// BlockWatch is a program, not a library. The library exists only for the binary and the fuzz
// targets, so nothing but `run` and `count_rust_blocks` may be `pub`. This lint catches any other.
#![warn(unreachable_pub)]

use serde::Serialize;
use std::ffi::OsString;

mod block_parser;
/// The `Block` type and the repository scan that turns source files into blocks to validate.
mod blocks;
/// The whole run: reads the flags and the input, runs the validators or `list`, and writes the
/// output.
mod cli;
/// Reads the settings from the config file, `blockwatch.toml`.
mod config;
/// Reads a unified diff into the per-file line changes that decide which blocks are checked.
mod diff_parser;
/// Command-line arguments and the accessors that turn them into globs, filters, and extension maps.
mod flags;
/// File access and path filtering behind traits, so tests can substitute fakes for the real disk.
mod fs;
/// One tree-sitter-backed comment parser per supported language, keyed by file extension.
mod language_parsers;
/// The paths and globs given on the command line, and the check that each selects a file.
mod path_arguments;
/// `RepoPath`: the single spelling of a repository-relative file path used as a map key.
mod repo_path;
/// Renders the end-of-run report describing what was scanned and checked.
mod report;
/// Renders the violations of a run as a SARIF log, the format code-scanning services read.
mod sarif;
/// Project settings: how they are validated, and how the flags and the config file are merged.
mod settings;
/// `SymbolPath`: a parsed and validated RFC 6901 symbol path.
mod symbol_path;
/// Derives the symbols of a file: the elements a symbol reference can address.
mod symbols;
mod tag_parser;
/// The rules enforced on blocks (`affects`, `keep-sorted`, …) and the machinery that runs them.
mod validators;
/// Unique violation address.
mod violation_address;
/// Blocks that the config file declares around a symbol, instead of tags in a comment.
mod virtual_blocks;

pub use cli::run;

/// Counts the blocks in `source`, which is Rust code. Returns an error if a tag is malformed or
/// not closed.
///
/// It is `pub` because a fuzz target is a separate crate, and a separate crate can only call `pub`
/// functions.
pub fn count_rust_blocks(source: &str) -> anyhow::Result<usize> {
    let parsers = language_parsers::language_parsers()?;
    parsers[&OsString::from("rs")]
        .lock()
        .expect("no active locks")
        .parse_blocks(source)
        .try_fold(0, |count, block| block.map(|_| count + 1))
}

/// A place in a source file.
#[derive(Serialize, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Position {
    /// 1-based line number.
    pub(crate) line: usize,
    /// 1-based character (column) number.
    pub(crate) character: usize,
}

impl Position {
    /// Creates a position from a 1-based `line` and `character`.
    ///
    /// Callers converting from a 0-based source (tree-sitter rows/columns, diff offsets) must add
    /// one first.
    pub(crate) fn new(line: usize, character: usize) -> Self {
        Self { line, character }
    }

    /// The position of `byte_offset` in `text`.
    ///
    /// Takes time proportional to `byte_offset`, since it counts every line break before it. It
    /// suits the few positions a message needs, not a position for every node of a file.
    ///
    /// # Panics
    /// Panics if `byte_offset` is past the end of `text` or not on a character boundary.
    pub(crate) fn from_byte_offset(text: &str, byte_offset: usize) -> Self {
        let line = text[..byte_offset].matches('\n').count() + 1;
        Self::new(line, character_column_at(text, byte_offset))
    }
}

/// The 1-based character column of `byte_offset` within its line in `text`.
pub(crate) fn character_column_at(text: &str, byte_offset: usize) -> usize {
    let line_start = text[..byte_offset].rfind('\n').map_or(0, |i| i + 1);
    text[line_start..byte_offset].chars().count() + 1
}

#[cfg(test)]
mod count_rust_blocks_tests {
    use super::count_rust_blocks;

    #[test]
    fn source_with_two_blocks_returns_two() -> anyhow::Result<()> {
        let source = "// <block>\nlet a = 1;\n// </block>\n// <block>\nlet b = 2;\n// </block>\n";
        assert_eq!(count_rust_blocks(source)?, 2);
        Ok(())
    }

    #[test]
    fn source_with_an_unclosed_block_returns_an_error() {
        assert!(count_rust_blocks("// <block>\nlet a = 1;\n").is_err());
    }
}

#[cfg(test)]
mod test_utils {
    use crate::blocks::{ScanMode, parse_blocks};
    use crate::diff_parser::{LineChange, LineChangeKind};
    use crate::fs::test_utils::{FakeFileSystem, FakePathChecker};
    use crate::language_parsers;
    use crate::repo_path::RepoPath;
    use crate::symbol_path::SymbolPath;
    use crate::validators::ValidationContext;
    use crate::virtual_blocks::{ConfigEntry, VirtualBlock};
    use std::collections::HashMap;
    use std::ops::Range;
    use std::path::PathBuf;
    use std::sync::Arc;

    /// Finds the byte range of the first occurrence of a substring within a string.
    ///
    /// # Arguments
    /// * `input` - The string to search in
    /// * `substr` - The substring to find
    pub(crate) fn substr_range(input: &str, substr: &str) -> Range<usize> {
        let pos = input.find(substr).unwrap();
        pos..(pos + substr.len())
    }

    /// Creates a [`ValidationContext`] for the given `file_name` with `contents` with all lines
    /// modified.
    pub(crate) fn validation_context(file_name: &str, contents: &str) -> Arc<ValidationContext> {
        let line_changes: Vec<LineChange> = contents
            .lines()
            .enumerate()
            .map(|(line, _)| LineChange {
                line: line + 1,
                kind: LineChangeKind::Added,
            })
            .collect();
        build_validation_context(file_name, contents, line_changes)
    }

    /// Creates a [`ValidationContext`] for the given `file_name` with `contents` and specified
    /// `line_changes`, rooted at the current directory.
    pub(crate) fn validation_context_with_changes(
        file_name: &str,
        contents: &str,
        line_changes: Vec<LineChange>,
    ) -> Arc<ValidationContext> {
        build_validation_context(file_name, contents, line_changes)
    }

    fn build_validation_context(
        file_name: &str,
        contents: &str,
        line_changes: Vec<LineChange>,
    ) -> Arc<ValidationContext> {
        let file_system = FakeFileSystem::new(HashMap::from([(
            file_name.to_string(),
            contents.to_string(),
        )]));
        let line_changes_by_file =
            HashMap::from([(RepoPath::from_reference(file_name).unwrap(), line_changes)]);
        let parsers = language_parsers::language_parsers().unwrap();
        Arc::new(ValidationContext::new(
            parse_blocks(
                &line_changes_by_file,
                ScanMode::OnlyChanged,
                &file_system,
                &FakePathChecker::allow_all(),
                &parsers,
                &HashMap::new(),
                &[],
            )
            .unwrap()
            .blocks,
            parsers,
            line_changes_by_file,
            HashMap::new(),
            Vec::new(),
        ))
    }

    /// A virtual block around `target`, such as `a.json#/key`, with `attributes`. It is declared
    /// at line 7 of `blockwatch.toml`.
    pub(crate) fn virtual_block(
        target: &str,
        attributes: &[(&str, &str)],
    ) -> anyhow::Result<VirtualBlock> {
        let (file, path) = target.split_once('#').expect("the target has a `#`");
        Ok(VirtualBlock {
            file: RepoPath::from_reference(file)?,
            path: SymbolPath::parse(path)?,
            attributes: attributes
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect(),
            entry: ConfigEntry {
                file: PathBuf::from("blockwatch.toml"),
                line: 7,
            },
            is_entry_modified: false,
        })
    }
}
