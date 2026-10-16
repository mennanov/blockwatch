// blockwatch is a program, not a library. The library exists only for the binary and the fuzz
// targets, so nothing but `run` and `count_rust_blocks` may be `pub`. This lint catches any other.
#![warn(unreachable_pub)]
// Slicing a `str` panics when an index falls inside a multi-byte character, such as `é`. So each
// slice outside tests needs an `expect` that says why its indexes are on character boundaries.
#![cfg_attr(not(test), warn(clippy::string_slice))]

use serde::Serialize;
use std::ffi::OsStr;
use std::ops::Range;

mod block_parser;
/// The `Block` type and the other types that describe the blocks of a file.
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
/// `PathsGlob`: a glob that lists the repository paths it matches.
mod paths_glob;
/// `RepoPath`: the single spelling of a repository-relative file path used as a map key.
mod repo_path;
/// Renders the end-of-run report describing what was scanned and checked.
mod report;
/// Renders the violations of a run as a SARIF log, the format code-scanning services read.
mod sarif;
/// The repository scan that turns source files into the blocks to validate.
mod scan;
/// Project settings: how they are validated, and how the flags and the config file are merged.
mod settings;
/// `SymbolPath`: a parsed and validated symbol path. It follows RFC 6901 and adds filters, such as
/// `~[name=blue]`.
mod symbol_path;
/// Derives the symbols of a file: the elements a symbol reference can address.
mod symbols;
mod tag_parser;
/// The reference to a block, a symbol or a file that `affects`, `check-lua` and `same-as` write in
/// an attribute, and the target it points at.
mod target_reference;
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
    let rust_parser = parsers
        .for_extension(OsStr::new("rs"))
        .expect("Rust has a parser");
    rust_parser
        .lock()
        .expect("a thread panicked while it held the parser")
        .parse_blocks(source)
        .try_fold(0, |count, block| block.map(|_| count + 1))
}

/// A place in a source file.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
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
}

/// Finds the [`Position`] of a byte offset in a text.
///
/// It finds where the lines start once. Counting the line breaks before each offset would scan
/// the text from its start every time, which is slow for a file with many comments.
#[derive(Debug)]
pub(crate) struct LineIndex<'a> {
    text: &'a str,
    /// The byte offset where each line of `text` starts, in order.
    line_starts: Vec<usize>,
}

impl<'a> LineIndex<'a> {
    pub(crate) fn new(text: &'a str) -> Self {
        let line_starts = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        Self { text, line_starts }
    }

    /// The position of `byte_offset` in the text.
    ///
    /// # Panics
    /// Panics if `byte_offset` is past the end of the text or not on a character boundary.
    pub(crate) fn position(&self, byte_offset: usize) -> Position {
        let line = self
            .line_starts
            .partition_point(|&start| start <= byte_offset);
        Position::new(line, character_column_at(self.text, byte_offset))
    }

    /// The positions of the start and the end of `byte_range`.
    ///
    /// # Panics
    /// Panics if an end of `byte_range` is past the end of the text or not on a character
    /// boundary.
    pub(crate) fn range(&self, byte_range: &Range<usize>) -> Range<Position> {
        self.position(byte_range.start)..self.position(byte_range.end)
    }
}

/// The 1-based character column of `byte_offset` within its line in `text`.
pub(crate) fn character_column_at(text: &str, byte_offset: usize) -> usize {
    #[expect(
        clippy::string_slice,
        reason = "callers give an offset on a character boundary"
    )]
    let line_start = text[..byte_offset].rfind('\n').map_or(0, |i| i + 1);
    #[expect(
        clippy::string_slice,
        reason = "`line_start` is right after a `\\n` or at 0"
    )]
    let before = &text[line_start..byte_offset];
    before.chars().count() + 1
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
    use crate::blocks::BlockSeverity;
    use crate::diff_parser::{DiffChanges, LineChange, LineChangeKind};
    use crate::fs::PathFilter;
    use crate::fs::test_utils::FakeFileSystem;
    use crate::language_parsers;
    use crate::repo_path::RepoPath;
    use crate::scan::{Scan, ScanMode};
    use crate::symbol_path::SymbolPath;
    use crate::validators::ValidationContext;
    use crate::virtual_blocks::{ConfigEntry, Locator, VirtualBlock};
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

    /// The diff's changes to each of `files`.
    pub(crate) fn diff_changes(
        files: impl IntoIterator<Item = (RepoPath, Vec<LineChange>)>,
    ) -> DiffChanges {
        files
            .into_iter()
            .map(|(file, line_changes)| (file, line_changes.into_iter().collect()))
            .collect()
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
            diff_changes([(RepoPath::from_reference(file_name).unwrap(), line_changes)]);
        let parsers = language_parsers::language_parsers().unwrap();
        Arc::new(ValidationContext::new(
            Scan::new(&file_system, &PathFilter::allow_all(), &parsers, &[])
                .parse_blocks(&line_changes_by_file, ScanMode::OnlyChanged)
                .unwrap()
                .blocks,
            parsers,
            line_changes_by_file,
            Vec::new(),
        ))
    }

    /// A virtual block around `target`, with `attributes`. It is declared at line 7 of
    /// `blockwatch.toml`. A target such as `a.json#/key` wraps a symbol, and one such as `a.txt`
    /// wraps the whole file.
    pub(crate) fn virtual_block(
        target: &str,
        attributes: &[(&str, &str)],
    ) -> anyhow::Result<VirtualBlock> {
        let (file, locator) = match target.split_once('#') {
            Some((file, path)) => (file, Locator::Symbol(SymbolPath::parse(path)?)),
            None => (target, Locator::File),
        };
        let attributes = attributes
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect();
        Ok(VirtualBlock {
            file: RepoPath::from_reference(file)?,
            locator,
            severity: BlockSeverity::from_attributes(&attributes)?,
            attributes,
            entry: ConfigEntry {
                file: PathBuf::from("blockwatch.toml"),
                line: 7,
            },
            is_entry_modified: false,
        })
    }
}
