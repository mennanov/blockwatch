//! The repository scan that turns source files into the blocks to validate.

use crate::blocks::{Block, BlockWithContext, FileBlocks};
use crate::diff_parser::{DiffChanges, LineChanges};
use crate::fs::{FileSystem, PathFilter};
use crate::language_parsers::{LanguageParsers, SharedLanguageParser};
use crate::repo_path::RepoPath;
use crate::virtual_blocks::{Locator, VirtualBlock};
use anyhow::{Context, anyhow, bail};
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Counts of the files a scan looked at.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ScanStats {
    /// Number of files that were read and parsed for blocks.
    pub(crate) files_scanned: usize,
    /// Number of files that were not parsed because their extension has no parser.
    pub(crate) files_skipped: usize,
}

/// The blocks found in each file, together with the counts of files the scan looked at.
#[derive(Debug, Default)]
pub(crate) struct ParsedBlocks {
    /// The blocks to validate, grouped by the file they were found in.
    pub(crate) blocks: HashMap<RepoPath, FileBlocks>,
    /// File counts for the run report; carried alongside the blocks because only the scan knows
    /// how many files it looked at but produced no blocks for.
    pub(crate) stats: ScanStats,
    /// The files that were read and parsed for blocks, in the order they were read.
    pub(crate) scanned_files: Vec<RepoPath>,
}

impl ParsedBlocks {
    /// Folds one file's parse result in. `None` means no parser claimed the file's extension,
    /// which counts as skipped rather than scanned.
    fn record(&mut self, file_path: RepoPath, file_blocks: Option<FileBlocks>) {
        match file_blocks {
            Some(file_blocks) => {
                self.stats.files_scanned += 1;
                self.scanned_files.push(file_path.clone());
                // A file that parsed cleanly but declares no blocks still counts as scanned; it is
                // simply nothing for the validators to work on.
                if !file_blocks.is_empty() {
                    self.blocks.insert(file_path, file_blocks);
                }
            }
            None => self.stats.files_skipped += 1,
        }
    }
}

/// How a run decides which files it reads, and which of their blocks it keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScanMode {
    /// Read every file in the repository, and keep every block. If a diff is supplied then the
    /// corresponding blocks are marked as modified.
    All,
    /// Read only the files from the supplied diff, keeping just the blocks that the diff modified.
    /// The file of a virtual block is read too when the diff modified the block's entry in the
    /// config file.
    OnlyChanged,
}

impl ScanMode {
    /// Whether a run in this mode keeps `block`.
    fn keeps(self, block: &BlockWithContext) -> bool {
        match self {
            Self::All => true,
            // A block with a modified start tag is considered modified because its rules
            // (attributes) are modified.
            Self::OnlyChanged => block.is_content_modified || block.is_start_tag_modified,
        }
    }
}

/// A pass over the repository that finds the blocks a run should consider.
///
/// A file is read only if `path_filter` selects it. A virtual block joins the blocks of the file it
/// wraps, when that file is read.
pub(crate) struct Scan<'a, F: FileSystem> {
    file_system: &'a F,
    path_filter: &'a PathFilter,
    /// Selects the language-specific block parser of a file.
    parsers: &'a LanguageParsers,
    /// The blocks the config file declares, in the order it declares them. They are also grouped
    /// in `virtual_blocks_by_file`. This list is kept because the scan reports the first block
    /// whose file is missing, and a hash map has no stable order.
    virtual_blocks: &'a [VirtualBlock],
    virtual_blocks_by_file: HashMap<&'a RepoPath, Vec<&'a VirtualBlock>>,
}

impl<'a, F: FileSystem> Scan<'a, F> {
    /// Prepares a scan that reads files through `file_system`. `virtual_blocks` are the blocks the
    /// config file declares.
    pub(crate) fn new(
        file_system: &'a F,
        path_filter: &'a PathFilter,
        parsers: &'a LanguageParsers,
        virtual_blocks: &'a [VirtualBlock],
    ) -> Self {
        let mut virtual_blocks_by_file: HashMap<&RepoPath, Vec<&VirtualBlock>> = HashMap::new();
        for virtual_block in virtual_blocks {
            virtual_blocks_by_file
                .entry(&virtual_block.file)
                .or_default()
                .push(virtual_block);
        }
        Self {
            file_system,
            path_filter,
            parsers,
            virtual_blocks,
            virtual_blocks_by_file,
        }
    }

    /// Parses source files into the blocks a run should consider, together with the counts of
    /// files it looked at.
    ///
    /// - `line_changes_by_file` maps file paths to sorted line changes.
    /// - `scan_mode` chooses which set of files is read; see [`ScanMode`].
    ///
    /// In either mode a file is read only if it passes the allow globs and is not ignored.
    ///
    /// Fails if `line_changes_by_file` is invalid, e.g. it refers to files that do not exist. Also
    /// fails if a virtual block's target does not resolve in a file that is read, or if its file
    /// passes the filters but does not exist. In [`ScanMode::OnlyChanged`], only a virtual block
    /// whose entry the diff modified fails for a missing file.
    pub(crate) fn parse_blocks(
        &self,
        line_changes_by_file: &DiffChanges,
        scan_mode: ScanMode,
    ) -> anyhow::Result<ParsedBlocks> {
        self.ensure_diff_has_valid_paths(line_changes_by_file)?;
        match scan_mode {
            ScanMode::All => {
                self.ensure_target_files_exist(self.virtual_blocks.iter())?;
                self.parse_all_files(line_changes_by_file)
            }
            ScanMode::OnlyChanged => {
                self.ensure_target_files_exist(
                    self.virtual_blocks
                        .iter()
                        .filter(|virtual_block| virtual_block.is_entry_modified),
                )?;
                self.parse_changed_files(line_changes_by_file)
            }
        }
    }

    /// The files of the walk that a run can check, in walk order: those that the path filter
    /// doesn't ignore and that have a parser or a virtual block. The allow globs are not applied,
    /// and no file is read. An error from the walk comes through as an `Err` item.
    pub(crate) fn checkable_files(&self) -> impl Iterator<Item = anyhow::Result<RepoPath>> {
        self.file_system
            .walk(None)
            .filter(move |file_path| match file_path {
                Ok(file_path) => {
                    !self.path_filter.is_ignored(file_path.as_path())
                        && (self.parsers.parser_for(file_path.as_path()).is_some()
                            || self.virtual_blocks_by_file.contains_key(file_path))
                }
                Err(_) => true,
            })
    }

    /// Rejects a virtual block whose file is selected by the path filter but does not exist.
    fn ensure_target_files_exist<'v>(
        &self,
        virtual_blocks: impl Iterator<Item = &'v VirtualBlock>,
    ) -> anyhow::Result<()> {
        for virtual_block in virtual_blocks {
            let file = &virtual_block.file;
            if self.path_filter.is_selected(file.as_path())
                && !self.file_system.exists(file.as_path())
            {
                return Err(virtual_block.unresolved(anyhow!("file does not exist")));
            }
        }
        Ok(())
    }

    /// Rejects a diff with no valid file paths.
    ///
    /// One invalid path is normal: deletions, generated files, paths outside the globs. But a diff
    /// where *no* path is valid is itself likely invalid: it marks no block as modified, so every
    /// rule that needs a diff would pass silently.
    fn ensure_diff_has_valid_paths(
        &self,
        line_changes_by_file: &DiffChanges,
    ) -> anyhow::Result<()> {
        let mut invalid_path: Option<&RepoPath> = None;
        for file_path in line_changes_by_file.files() {
            // Only paths this run would have opened count: past the filters, with a known
            // extension.
            if !self.path_filter.is_selected(file_path.as_path()) {
                continue;
            }
            if self.parsers.parser_for(file_path.as_path()).is_none() {
                continue;
            }
            if self.file_system.exists(file_path.as_path()) {
                return Ok(());
            }
            // Hash map order is not stable, so pick the path to report rather than take the first.
            invalid_path = Some(invalid_path.map_or(file_path, |lowest| lowest.min(file_path)));
        }
        match invalid_path {
            None => Ok(()),
            Some(file_path) => Err(anyhow!("{}", invalid_diff_target_message(file_path))),
        }
    }

    /// Parses every block in every file of the repository, marking the ones the line changes
    /// touched. Line changes for files the walk did not reach contribute nothing to the result;
    /// the walk defines the scope, and the diff only says which of the blocks it found had changed.
    fn parse_all_files(&self, line_changes_by_file: &DiffChanges) -> anyhow::Result<ParsedBlocks> {
        let mut parsed = ParsedBlocks::default();
        for repo_path_result in self.file_system.walk(None) {
            let file_path =
                repo_path_result.map_err(|err| anyhow!("failed to walk directory: {err}"))?;
            if !self.path_filter.is_selected(file_path.as_path()) {
                continue;
            }
            let file_blocks = parse_file(
                self.file_system,
                file_path.as_path(),
                line_changes_by_file.for_file(&file_path),
                ScanMode::All,
                self.parsers,
                self.virtual_blocks_of(&file_path),
            )?;
            parsed.record(file_path, file_blocks);
        }
        Ok(parsed)
    }

    /// Parses only the files in the diff, and the files of the virtual blocks whose entry it
    /// modified. Keeps the blocks whose start tag or content it modified.
    fn parse_changed_files(
        &self,
        line_changes_by_file: &DiffChanges,
    ) -> anyhow::Result<ParsedBlocks> {
        // A changed entry in the config file changes the rules of its block, as a changed tag
        // does. So the block is checked even when the diff does not touch its file.
        let entry_files = self
            .virtual_blocks_by_file
            .iter()
            .filter(|(_, virtual_blocks)| {
                virtual_blocks
                    .iter()
                    .any(|virtual_block| virtual_block.is_entry_modified)
            })
            .map(|(file_path, _)| *file_path);
        let files: HashSet<&RepoPath> = line_changes_by_file.files().chain(entry_files).collect();
        let mut parsed = ParsedBlocks::default();
        for file_path in files {
            if !self.path_filter.is_selected(file_path.as_path()) {
                continue;
            }
            let file_blocks = parse_file(
                self.file_system,
                file_path.as_path(),
                line_changes_by_file.for_file(file_path),
                ScanMode::OnlyChanged,
                self.parsers,
                self.virtual_blocks_of(file_path),
            )
            .map_err(|error| {
                // Only a file this run validates reaches a read: filtered-out paths are skipped
                // above, and one with no language parser and no virtual blocks returns before its
                // contents are read.
                if self.file_system.exists(file_path.as_path()) {
                    error
                } else {
                    error.context(invalid_diff_target_message(file_path))
                }
            })?;
            parsed.record(file_path.clone(), file_blocks);
        }
        Ok(parsed)
    }

    /// The virtual blocks that wrap `file`.
    fn virtual_blocks_of(&self, file: &RepoPath) -> &[&'a VirtualBlock] {
        self.virtual_blocks_by_file
            .get(file)
            .map_or(&[][..], Vec::as_slice)
    }
}

fn invalid_diff_target_message(file_path: &RepoPath) -> String {
    format!("diff target \"{file_path}\" does not exist in the repository root")
}

/// Parses the blocks of one file, marking the ones `line_changes` touched and keeping those
/// `scan_mode` keeps. The blocks come in source order, the `virtual_blocks` of the file
/// among its tags. A file without a parser has no tags, only its `virtual_blocks`. Returns `None`
/// for a file with neither a parser nor `virtual_blocks`. For a file that is not valid UTF-8 but
/// has no blocks, returns no blocks and an empty text.
///
/// # Errors
/// Returns an error if the file can't be read, if it is not valid UTF-8 and has blocks, if a tag
/// is malformed or a name is used twice, or if a virtual block's target does not resolve in the
/// file.
pub(crate) fn parse_file(
    file_system: &impl FileSystem,
    file_path: &Path,
    line_changes: &LineChanges,
    scan_mode: ScanMode,
    parsers: &LanguageParsers,
    virtual_blocks: &[&VirtualBlock],
) -> anyhow::Result<Option<FileBlocks>> {
    let parser = parsers.parser_for(file_path);
    if parser.is_none() && virtual_blocks.is_empty() {
        return Ok(None);
    }
    let source_code = match String::from_utf8(file_system.read(file_path)?) {
        Ok(source_code) => source_code,
        // The replaced bytes would change what a rule compares, so a block here can't be checked.
        Err(error)
            if non_utf8_file_has_blocks(parser, file_path, error.as_bytes(), virtual_blocks) =>
        {
            bail!(
                "file \"{}\" is not valid UTF-8, so its blocks can't be read. Save it as \
                 UTF-8, or skip it with `ignore` in the config file or with `--ignore`",
                file_path.display()
            );
        }
        // No block refers into a file without blocks, so its text is not kept.
        Err(_) => {
            return Ok(Some(FileBlocks {
                file_content: String::new(),
                blocks_with_context: Vec::new(),
            }));
        }
    };
    parse_source(
        parser,
        file_path,
        source_code,
        line_changes,
        scan_mode,
        virtual_blocks,
    )
    .map(Some)
}

/// Whether a file that is not valid UTF-8 has blocks: one of its `virtual_blocks`, or a tag in
/// `bytes`, the contents of `file_path`. A tag that doesn't parse counts as a block. A file
/// without a `parser` has no tags.
fn non_utf8_file_has_blocks(
    parser: Option<&SharedLanguageParser>,
    file_path: &Path,
    bytes: &[u8],
    virtual_blocks: &[&VirtualBlock],
) -> bool {
    !virtual_blocks.is_empty()
        || parser.is_some_and(|parser| {
            // Tags are ASCII. So in an encoding such as Latin-1 they are still found after the
            // other bytes are replaced. In UTF-16 they are never found.
            let text = String::from_utf8_lossy(bytes);
            !tag_blocks(
                parser,
                file_path,
                &text,
                &LineChanges::default(),
                ScanMode::All,
                &mut HashMap::new(),
            )
            .is_ok_and(|tags| tags.is_empty())
        })
}

/// Parses the blocks of `source_code`, the text of `file_path`: its tags and its
/// `virtual_blocks`. Marks the ones `line_changes` touched and keeps those `scan_mode` keeps, in
/// source order. Without a `parser`, the file has no tags.
///
/// # Errors
/// Returns an error if a tag is malformed or a name is used twice, or if a virtual block's target
/// does not resolve in the file.
fn parse_source(
    parser: Option<&SharedLanguageParser>,
    file_path: &Path,
    source_code: String,
    line_changes: &LineChanges,
    scan_mode: ScanMode,
    virtual_blocks: &[&VirtualBlock],
) -> anyhow::Result<FileBlocks> {
    // Tracks where each name in this file is first declared, to reject duplicate blocks.
    let mut names_seen = HashMap::new();
    let mut blocks_with_context = match parser {
        Some(parser) => tag_blocks(
            parser,
            file_path,
            &source_code,
            line_changes,
            scan_mode,
            &mut names_seen,
        )?,
        // Tags are found in the file's comments, and only a parser knows how a comment is written.
        None => Vec::new(),
    };
    blocks_with_context.extend(virtual_blocks_in_file(
        virtual_blocks,
        parser,
        file_path,
        &source_code,
        line_changes,
        scan_mode,
        &mut names_seen,
    )?);
    // The tags come in source order. The virtual blocks join them there.
    blocks_with_context.sort_by(|a, b| a.block.cmp(&b.block));
    Ok(FileBlocks {
        file_content: source_code,
        blocks_with_context,
    })
}

/// The blocks that tags declare in `source`, the text of `file_path`, with what `line_changes`
/// touched in each. Keeps those `scan_mode` keeps, in source order, and records every block's name
/// in `names_seen`.
///
/// # Errors
/// Returns an error if a tag is malformed or not closed, has an unknown attribute or severity, or
/// has a name that `names_seen` already holds. The error shows the file.
fn tag_blocks(
    parser: &SharedLanguageParser,
    file_path: &Path,
    source: &str,
    line_changes: &LineChanges,
    scan_mode: ScanMode,
    names_seen: &mut HashMap<String, String>,
) -> anyhow::Result<Vec<BlockWithContext>> {
    // Tree-sitter takes most of the time a scan spends on a file, and most files hold no tags. A
    // file without the text that opens a start tag holds no block. The text that opens an end tag
    // is checked too, so that an end tag left behind when its start tag was deleted still fails
    // the run. An end tag with a space before or after its slash is missed in such a file.
    if !source.contains("<block") && !source.contains("</block") {
        return Ok(Vec::new());
    }
    // Blocks are filtered as the parser yields them, so only the ones this run will validate are
    // ever held. The parser's lock lives until the end of the statement, which is as long as the
    // iterator borrowing it does.
    parser
        .lock()
        .expect("a thread panicked while it held the parser")
        .parse_blocks(source)
        .map(|block| {
            let block = block?;
            reject_duplicate_name(&block, file_path, names_seen)?;
            Ok(BlockWithContext {
                is_content_modified: block.content.intersects_any(line_changes),
                is_start_tag_modified: line_changes.touches(&block.start_tag_position_range),
                block,
            })
        })
        .filter(|result| match result {
            Ok(block_with_context) => scan_mode.keeps(block_with_context),
            // An error is kept, so that collecting the blocks stops at it.
            Err(_) => true,
        })
        .collect::<anyhow::Result<Vec<_>>>()
        .with_context(|| format!("failed to parse file \"{}\"", file_path.display()))
}

/// The blocks that `virtual_blocks` declare in `source`, the text of `file_path`, with what
/// `line_changes` touched in each. Keeps those `scan_mode` keeps, in the order of
/// `virtual_blocks`, and records every block's name in `names_seen`.
///
/// # Errors
/// Returns an error if a block wraps a symbol and the file has no parser, no symbols or a syntax
/// error, if a target does not resolve, or if a block has a name that `names_seen` already holds.
fn virtual_blocks_in_file(
    virtual_blocks: &[&VirtualBlock],
    parser: Option<&SharedLanguageParser>,
    file_path: &Path,
    source: &str,
    line_changes: &LineChanges,
    scan_mode: ScanMode,
    names_seen: &mut HashMap<String, String>,
) -> anyhow::Result<Vec<BlockWithContext>> {
    // Only a block that wraps a symbol needs the file's symbols. A language without symbols must
    // not fail the other blocks.
    let first_symbol_block =
        virtual_blocks
            .iter()
            .find(|virtual_block| match virtual_block.locator {
                Locator::Symbol(_) => true,
                Locator::Pattern(_) | Locator::File => false,
            });
    // The file's symbols are the same for each of its virtual blocks. So is the error when they
    // can't be found, which then shows the first block that needs them.
    let symbols = match (first_symbol_block, parser) {
        (None, _) => Vec::new(),
        (Some(first), Some(parser)) => parser
            .lock()
            .expect("a thread panicked while it held the parser")
            .parse_symbols(source)
            .map_err(|error| first.unresolved(error))?,
        (Some(first), None) => {
            return Err(first.unresolved(anyhow!("file format is unsupported")));
        }
    };
    let mut blocks_with_context = Vec::new();
    for virtual_block in virtual_blocks {
        let block_with_context = virtual_block.resolve(&symbols, source, line_changes)?;
        reject_duplicate_name(&block_with_context.block, file_path, names_seen)?;
        if scan_mode.keeps(&block_with_context) {
            blocks_with_context.push(block_with_context);
        }
    }
    Ok(blocks_with_context)
}

/// Rejects a block whose `name` was already used earlier in the same file. Otherwise records in
/// `names_seen` where the block is declared. A block with no `name` is not a reference target
/// and is ignored.
///
/// Two blocks sharing a name make every `affects`/`same-as` reference to it ambiguous, silently
/// binding to whichever block the parser happens to reach first.
fn reject_duplicate_name(
    block: &Block,
    file_path: &Path,
    names_seen: &mut HashMap<String, String>,
) -> anyhow::Result<()> {
    let Some(name) = block.name() else {
        return Ok(());
    };
    match names_seen.entry(name.to_string()) {
        Entry::Occupied(entry) => {
            bail!(
                "block {}:{} at {} duplicates the name of the block at {}",
                file_path.display(),
                name,
                block.declared_at(),
                entry.get(),
            )
        }
        Entry::Vacant(entry) => {
            entry.insert(block.declared_at());
            Ok(())
        }
    }
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
mod parse_blocks_tests {
    use super::*;
    use crate::blocks::*;
    use crate::diff_parser::{LineChange, LineChangeKind};
    use crate::fs::PathFilter;
    use crate::fs::test_utils::FakeFileSystem;
    use crate::language_parsers::language_parsers;
    use crate::test_utils::{self, diff_changes};
    use std::collections::HashSet;
    use std::ops::Range;

    /// A newly added line.
    fn added(line: usize) -> LineChange {
        LineChange {
            line,
            kind: LineChangeKind::Added,
        }
    }

    /// A modified line with `length` chars modified.
    fn modified(line: usize, length: usize) -> LineChange {
        LineChange {
            line,
            kind: LineChangeKind::Modified(vec![0..length]),
        }
    }

    /// A modified line with character range `range` (0-based) modified.
    fn modified_range(line: usize, range: Range<usize>) -> LineChange {
        LineChange {
            line,
            kind: LineChangeKind::Modified(vec![range]),
        }
    }

    /// A deletion anchored at `line`, the line that now occupies the gap the removed lines left.
    fn deleted(line: usize) -> LineChange {
        LineChange {
            line,
            kind: LineChangeKind::Deleted,
        }
    }

    #[test]
    fn parse_blocks_counts_scanned_and_skipped_files() -> anyhow::Result<()> {
        let file_system = FakeFileSystem::new(HashMap::from([
            (
                "with_blocks.py".to_string(),
                "# <block keep-sorted=\"asc\">\n'a'\n# </block>\n".to_string(),
            ),
            ("without_blocks.py".to_string(), "x = 1\n".to_string()),
            ("notes.unknown".to_string(), "not a language\n".to_string()),
        ]));
        let parsers = language_parsers()?;

        let parsed = Scan::new(&file_system, &PathFilter::allow_all(), &parsers, &[])
            .parse_blocks(&DiffChanges::default(), ScanMode::All)?;

        // The file without blocks is not kept, but it was parsed, so it counts as scanned. The
        // file with an unknown extension counts as skipped.
        assert_eq!(parsed.blocks.len(), 1);
        assert_eq!(parsed.stats.files_scanned, 2);
        assert_eq!(parsed.stats.files_skipped, 1);
        Ok(())
    }

    #[test]
    fn ignored_file_and_file_without_parser_checkable_files_skips_them() -> anyhow::Result<()> {
        let file_system = FakeFileSystem::new(HashMap::from([
            ("kept.py".to_string(), String::new()),
            ("ignored.py".to_string(), String::new()),
            ("notes.unknown".to_string(), String::new()),
        ]));
        let path_filter = PathFilter::with_ignored_paths(HashSet::from(["ignored.py".to_string()]));

        let files = Scan::new(&file_system, &path_filter, &language_parsers()?, &[])
            .checkable_files()
            .collect::<anyhow::Result<Vec<_>>>()?;

        assert_eq!(files, [RepoPath::from_reference("kept.py")?]);
        Ok(())
    }

    #[test]
    fn walk_error_checkable_files_returns_it() -> anyhow::Result<()> {
        // The fake walk fails on a path that escapes the repository.
        let file_system = FakeFileSystem::new(HashMap::from([(
            "../outside.py".to_string(),
            String::new(),
        )]));

        let files = Scan::new(
            &file_system,
            &PathFilter::allow_all(),
            &language_parsers()?,
            &[],
        )
        .checkable_files()
        .collect::<anyhow::Result<Vec<_>>>();

        assert!(files.is_err(), "the walk error was dropped: {files:?}");
        Ok(())
    }

    #[test]
    fn diff_targets_mode_returns_only_blocks_with_modified_start_tag_or_content()
    -> anyhow::Result<()> {
        let content_a = r#"
        /* <block name="first"> */ let foo = "bar"; // </block>
        /* <block name="second"> */ let foo = "baz"; // </block>
        /* <block name="third"> */ third block /* </block> */
        /* <block name="fourth"> */ fourth block // </block>
        /* <block name="fifth"> */ let foo="boo"; // </block>
        /* <block
            name="sixth"
            keep-sorted="asc"> */ block six // </block>
        /* <block name="seventh"
            keep-sorted="asc"> */ block seven
        // </block>
        /* <block name="eighth"
            keep-sorted="asc"> */ block eight
        // </block>
        // <block name="ninth">
        block nine
        // </block>
        // <block name="tenth">
        block ten // </block>
        // <block name="eleventh">
        block eleven // </block>
        // <block name="twelfth">
        twelve /*
        Some comment.
        </block> */
        "#;
        let content_b = "/* <block name=\"first\"> */let foo = \"bar\"; // </block>";
        let file_system = FakeFileSystem::new(HashMap::from([
            ("a.rs".to_string(), content_a.to_string()),
            ("b.rs".to_string(), content_b.to_string()),
        ]));
        let line_changes = diff_changes([
            (
                RepoPath::from_reference("a.rs")?,
                vec![
                    added(1), // No blocks on this line.
                    LineChange {
                        // "first" block.
                        line: 2,
                        kind: LineChangeKind::Modified(vec![
                            test_utils::substr_range(
                                content_a.lines().nth(1).unwrap(),
                                "/* <block ",
                            ),
                            test_utils::substr_range(
                                content_a.lines().nth(1).unwrap(),
                                "name=\"first\"> */",
                            ),
                        ]), // The start tag is modified, not the contents.
                    },
                    LineChange {
                        // "second" block.
                        line: 3,
                        kind: LineChangeKind::Modified(vec![
                            test_utils::substr_range(
                                content_a.lines().nth(2).unwrap(),
                                "/* <block name=\"second\"> */ let foo ",
                            ), /* tag and contents*/
                            test_utils::substr_range(
                                content_a.lines().nth(2).unwrap(),
                                " = \"baz\"; ",
                            ), /* contents only */
                        ]),
                    },
                    LineChange {
                        // "third" block.
                        line: 4,
                        kind: LineChangeKind::Modified(vec![test_utils::substr_range(
                            content_a.lines().nth(3).unwrap(),
                            " third block ",
                        )]), // Only the content is modified.
                    },
                    LineChange {
                        // "fourth" block.
                        line: 5,
                        kind: LineChangeKind::Modified(vec![test_utils::substr_range(
                            content_a.lines().nth(4).unwrap(),
                            " fourth block // </block>",
                        )]), // The content and end tag are modified.
                    },
                    LineChange {
                        // "fifth" block.
                        line: 6,
                        kind: LineChangeKind::Modified(vec![test_utils::substr_range(
                            content_a.lines().nth(5).unwrap(),
                            " </block>",
                        )]), // Only the end tag is modified.
                    },
                    LineChange {
                        // "sixth" block.
                        line: 8,
                        kind: LineChangeKind::Modified(vec![test_utils::substr_range(
                            content_a.lines().nth(7).unwrap(),
                            "name=\"sixth\"",
                        )]), // Only the start tag is modified.
                    },
                    LineChange {
                        // "seventh" block.
                        line: 11,
                        kind: LineChangeKind::Modified(vec![test_utils::substr_range(
                            content_a.lines().nth(10).unwrap(),
                            "keep-sorted=\"asc\"> */",
                        )]), // Only the start tag is modified.
                    },
                    LineChange {
                        // "eighth" block.
                        line: 14,
                        kind: LineChangeKind::Modified(vec![test_utils::substr_range(
                            content_a.lines().nth(13).unwrap(),
                            " block eight",
                        )]), // Only the content on the same line as start tag is modified.
                    },
                    LineChange {
                        // "ninth" block.
                        line: 17,
                        kind: LineChangeKind::Modified(vec![test_utils::substr_range(
                            content_a.lines().nth(16).unwrap(),
                            "block nine",
                        )]), // Only the content on a line between start and end tags is modified.
                    },
                    LineChange {
                        // "tenth" block.
                        line: 20,
                        kind: LineChangeKind::Modified(vec![test_utils::substr_range(
                            content_a.lines().nth(19).unwrap(),
                            "block ten ",
                        )]), // Only the content on the same line as end tag is modified.
                    },
                    LineChange {
                        // "eleventh" block.
                        line: 22,
                        kind: LineChangeKind::Modified(vec![test_utils::substr_range(
                            content_a.lines().nth(21).unwrap(),
                            " </block>",
                        )]), // End tag is modified.
                    },
                    LineChange {
                        // "twelfth" block.
                        line: 25,
                        kind: LineChangeKind::Modified(vec![test_utils::substr_range(
                            content_a.lines().nth(24).unwrap(),
                            "Some comment.",
                        )]), // Multiline end tag is modified.
                    },
                ],
            ),
            (
                RepoPath::from_reference("b.rs")?,
                vec![LineChange {
                    line: 1,
                    kind: LineChangeKind::Modified(vec![test_utils::substr_range(
                        content_b.lines().next().unwrap(),
                        "let foo = \"bar\"; ",
                    )]), // Block's content is modified in a single line file.
                }],
            ),
        ]);
        let parsers = language_parsers()?;

        let blocks_by_file = Scan::new(&file_system, &PathFilter::allow_all(), &parsers, &[])
            .parse_blocks(&line_changes, ScanMode::OnlyChanged)?
            .blocks;

        assert_eq!(blocks_by_file.len(), 2);
        let blocks_a = &blocks_by_file[&RepoPath::from_reference("a.rs")?].blocks_with_context;
        assert_eq!(blocks_a.len(), 9);
        let first = &blocks_a[0];
        assert_eq!(first.block.name(), Some("first"));
        assert!(first.is_start_tag_modified);
        assert!(!first.is_content_modified);
        let second = &blocks_a[1];
        assert_eq!(second.block.name(), Some("second"));
        assert!(second.is_start_tag_modified);
        assert!(second.is_content_modified);
        let third = &blocks_a[2];
        assert_eq!(third.block.name(), Some("third"));
        assert!(!third.is_start_tag_modified);
        assert!(third.is_content_modified);
        let fourth = &blocks_a[3];
        assert_eq!(fourth.block.name(), Some("fourth"));
        assert!(!fourth.is_start_tag_modified);
        assert!(fourth.is_content_modified);
        let sixth = &blocks_a[4];
        assert_eq!(sixth.block.name(), Some("sixth"));
        assert!(sixth.is_start_tag_modified);
        assert!(!sixth.is_content_modified);
        let seventh = &blocks_a[5];
        assert_eq!(seventh.block.name(), Some("seventh"));
        assert!(seventh.is_start_tag_modified);
        assert!(!seventh.is_content_modified);
        let eighth = &blocks_a[6];
        assert_eq!(eighth.block.name(), Some("eighth"));
        assert!(!eighth.is_start_tag_modified);
        assert!(eighth.is_content_modified);
        let ninth = &blocks_a[7];
        assert_eq!(ninth.block.name(), Some("ninth"));
        assert!(!ninth.is_start_tag_modified);
        assert!(ninth.is_content_modified);
        let tenth = &blocks_a[8];
        assert_eq!(tenth.block.name(), Some("tenth"));
        assert!(!tenth.is_start_tag_modified);
        assert!(tenth.is_content_modified);
        let blocks_b = &blocks_by_file[&RepoPath::from_reference("b.rs")?].blocks_with_context;
        assert_eq!(blocks_b.len(), 1);
        assert_eq!(blocks_b[0].block.name(), Some("first"));

        Ok(())
    }

    /// Parses a single block from `content` with `line_changes` applied.
    ///
    /// Scans in [`ScanMode::All`] so that a block the changes did not touch is returned too,
    /// rather than filtered out before its flags can be read.
    fn block_with_context(
        content: &str,
        line_changes: Vec<LineChange>,
    ) -> anyhow::Result<BlockWithContext> {
        let file_system =
            FakeFileSystem::new(HashMap::from([("a.rs".to_string(), content.to_string())]));
        let line_changes = diff_changes([(RepoPath::from_reference("a.rs")?, line_changes)]);

        let mut blocks_by_file = Scan::new(
            &file_system,
            &PathFilter::allow_all(),
            &language_parsers()?,
            &[],
        )
        .parse_blocks(&line_changes, ScanMode::All)?
        .blocks;

        let mut file_blocks = blocks_by_file
            .remove(&RepoPath::from_reference("a.rs")?)
            .expect("a.rs holds a block");
        assert_eq!(file_blocks.blocks_with_context.len(), 1);
        Ok(file_blocks.blocks_with_context.remove(0))
    }

    #[test]
    fn modified_start_tag_line_without_content_leaves_the_content_unmodified() -> anyhow::Result<()>
    {
        // The start tag's comment ends its line, so the content begins on the next one. Rewriting
        // every character of the line still reaches nothing the content owns.
        let tag_line = "// <block name=\"first\">";
        let block = block_with_context(
            &format!("{tag_line}\none\n// </block>\n"),
            vec![modified(1, tag_line.chars().count())],
        )?;

        assert!(block.is_start_tag_modified);
        assert!(!block.is_content_modified);
        Ok(())
    }

    #[test]
    fn modified_start_tag_line_carrying_content_modifies_the_content() -> anyhow::Result<()> {
        let tag_line = "/* <block name=\"first\"> */ one";
        let block = block_with_context(
            &format!("{tag_line}\n// </block>\n"),
            vec![modified(1, tag_line.chars().count())],
        )?;

        assert!(block.is_start_tag_modified);
        assert!(block.is_content_modified);
        Ok(())
    }

    #[test]
    fn added_start_tag_line_modifies_the_content() -> anyhow::Result<()> {
        // An inserted line brings a new line break too, and that break is the first thing the
        // content owns, so the block's content did change.
        let block = block_with_context(
            "// <block name=\"first\">\none\n// </block>\n",
            vec![added(1)],
        )?;

        assert!(block.is_start_tag_modified);
        assert!(block.is_content_modified);
        Ok(())
    }

    #[test]
    fn deleted_lines_above_the_start_tag_leave_the_block_unmodified() -> anyhow::Result<()> {
        // A deletion is anchored at the line that took the removed lines' place, so it lands on
        // column 1 of the start tag's line. The removed text sat above the block, and the block
        // starts further right on that line, so neither the tag nor the content lost anything.
        let block = block_with_context(
            "// <block name=\"first\">\none\n// </block>\n",
            vec![deleted(1)],
        )?;

        assert!(!block.is_start_tag_modified);
        assert!(!block.is_content_modified);
        Ok(())
    }

    #[test]
    fn deleted_lines_above_a_start_tag_at_column_one_leave_the_block_unmodified()
    -> anyhow::Result<()> {
        // The tag opens a continuation line of a block comment, so it starts at column 1 and the
        // deletion's gap lands exactly on the tag's first character. The gap still sits before
        // that character, so the tag kept every character it had.
        let block = block_with_context(
            "/*\n<block name=\"first\">\n*/\none\n// </block>\n",
            vec![deleted(2)],
        )?;

        assert!(!block.is_start_tag_modified);
        assert!(!block.is_content_modified);
        Ok(())
    }

    #[test]
    fn deleted_line_inside_a_multi_line_start_tag_modifies_the_start_tag() -> anyhow::Result<()> {
        // The gap sits between two lines the tag spans, so the tag itself lost a line. The content
        // begins after the tag's comment ends, which the gap never reaches.
        let block = block_with_context(
            "/* <block\n   name=\"first\"> */\none\n/* </block> */\n",
            vec![deleted(2)],
        )?;

        assert!(block.is_start_tag_modified);
        assert!(!block.is_content_modified);
        Ok(())
    }

    #[test]
    fn deleting_every_content_line_modifies_the_content() -> anyhow::Result<()> {
        // Nothing is left between the tags, so the deletion is anchored at the end tag's line,
        // which is where the content range ends. The content lost every line it had.
        let block =
            block_with_context("// <block name=\"first\">\n// </block>\n", vec![deleted(2)])?;

        assert!(!block.is_start_tag_modified);
        assert!(block.is_content_modified);
        Ok(())
    }

    #[test]
    fn modified_column_before_start_tag_leaves_the_start_tag_unmodified() -> anyhow::Result<()> {
        // The start tag begins at 0-based index 7 (column 8). Index 6 (column 7) is the space
        // before the tag, which sits outside the start tag range.
        let code = "    // <block name=\"first\">\none\n// </block>\n";
        let block = block_with_context(code, vec![modified_range(1, 6..7)])?;

        assert!(!block.is_start_tag_modified);
        assert!(!block.is_content_modified);
        Ok(())
    }

    #[test]
    fn modified_first_column_of_start_tag_modifies_the_start_tag() -> anyhow::Result<()> {
        // The start tag begins at 0-based index 7 (column 8). Index 7 is the '<' character,
        // which sits at the inclusive start of the start tag range.
        let code = "    // <block name=\"first\">\none\n// </block>\n";
        let block = block_with_context(code, vec![modified_range(1, 7..8)])?;

        assert!(block.is_start_tag_modified);
        assert!(!block.is_content_modified);
        Ok(())
    }

    #[test]
    fn modified_column_after_multiline_start_tag_leaves_the_start_tag_unmodified()
    -> anyhow::Result<()> {
        // The start tag's closing delimiter '>' is at 0-based index 15 (column 16), making column 17
        // the exclusive end. Index 16 (column 17) is the space after '>', which sits outside the tag.
        let code = "/* <block\n   name=\"first\"> */\none\n/* </block> */\n";
        let block = block_with_context(code, vec![modified_range(2, 16..17)])?;

        assert!(!block.is_start_tag_modified);
        assert!(!block.is_content_modified);
        Ok(())
    }

    #[test]
    fn modified_last_column_of_multiline_start_tag_modifies_the_start_tag() -> anyhow::Result<()> {
        // The start tag's closing delimiter '>' is at 0-based index 15 (column 16). That character
        // sits immediately before the exclusive end, inside the tag.
        let code = "/* <block\n   name=\"first\"> */\none\n/* </block> */\n";
        let block = block_with_context(code, vec![modified_range(2, 15..16)])?;

        assert!(block.is_start_tag_modified);
        assert!(!block.is_content_modified);
        Ok(())
    }

    #[test]
    fn modified_start_tag_comment_before_inline_content_leaves_the_content_unmodified()
    -> anyhow::Result<()> {
        // The start tag comment '/* ... */' ends at index 26 (column 27), where content begins.
        // Index 25 (column 26) is the closing '/' of '*/', outside the content range.
        let code = "/* <block name=\"first\"> */ one\n// </block>\n";
        let block = block_with_context(code, vec![modified_range(1, 25..26)])?;

        assert!(!block.is_start_tag_modified);
        assert!(!block.is_content_modified);
        Ok(())
    }

    #[test]
    fn modified_first_column_of_inline_content_modifies_the_content() -> anyhow::Result<()> {
        // Content begins at 0-based index 26 (column 27). That character sits at the inclusive
        // start of the content range.
        let code = "/* <block name=\"first\"> */ one\n// </block>\n";
        let block = block_with_context(code, vec![modified_range(1, 26..27)])?;

        assert!(!block.is_start_tag_modified);
        assert!(block.is_content_modified);
        Ok(())
    }

    #[test]
    fn modified_end_tag_comment_after_inline_content_leaves_the_content_unmodified()
    -> anyhow::Result<()> {
        // Content ends at 0-based index 4 (column 5), where the closing comment begins.
        // Index 4 is the opening '/' of '/*', outside the content range.
        let code = "/* <block name=\"first\"> */\none /* </block> */\n";
        let block = block_with_context(code, vec![modified_range(2, 4..5)])?;

        assert!(!block.is_start_tag_modified);
        assert!(!block.is_content_modified);
        Ok(())
    }

    #[test]
    fn modified_last_column_of_inline_content_modifies_the_content() -> anyhow::Result<()> {
        // Content ends at 0-based index 4 (column 5). Index 3 (column 4) is the space after 'one',
        // which sits immediately before the exclusive end, inside content.
        let code = "/* <block name=\"first\"> */\none /* </block> */\n";
        let block = block_with_context(code, vec![modified_range(2, 3..4)])?;

        assert!(!block.is_start_tag_modified);
        assert!(block.is_content_modified);
        Ok(())
    }

    #[test]
    fn modified_column_one_on_continuation_line_of_start_tag_modifies_the_start_tag()
    -> anyhow::Result<()> {
        // Line 2 is a continuation line of a multi-line start tag. A modification at column 1
        // of line 2 touches the start tag since continuation lines start at column 1.
        let code = "/* <block\nname=\"first\"> */\none\n/* </block> */\n";
        let block = block_with_context(code, vec![modified_range(2, 0..1)])?;

        assert!(block.is_start_tag_modified);
        assert!(!block.is_content_modified);
        Ok(())
    }

    #[test]
    fn modified_column_one_on_continuation_line_of_content_modifies_the_content()
    -> anyhow::Result<()> {
        // The content starts on line 1 after an inline tag. A modification at column 1 of line 2
        // touches the content since continuation lines start at column 1.
        let code = "/* <block name=\"first\"> */ first\nsecond\n/* </block> */\n";
        let block = block_with_context(code, vec![modified_range(2, 0..1)])?;

        assert!(!block.is_start_tag_modified);
        assert!(block.is_content_modified);
        Ok(())
    }

    #[test]
    fn all_mode_with_line_changes_parses_modified_and_unmodified_blocks() -> anyhow::Result<()> {
        let file_system = FakeFileSystem::new(HashMap::from([
            (
                "a.rs".to_string(),
                r#"
        // <block name="first_from_a">
        fn a() {}
        // </block>
        // <block name="second_from_a">
        fn b() {
            println!("hello");
        }
        // </block>
        "#
                .to_string(),
            ),
            (
                "b.rs".to_string(),
                r#"
        // <block name="first_from_b">
        fn a() {}
        // </block>
        // <block name="second_from_b">
        fn b() {
            println!("hello");
        }
        // </block>
        "#
                .to_string(),
            ),
        ]));
        let parsers = language_parsers()?;

        let line_changes = diff_changes([
            (
                RepoPath::from_reference("a.rs")?,
                vec![LineChange {
                    line: 3, // Content line of the first block.
                    kind: LineChangeKind::Added,
                }],
            ),
            (
                RepoPath::from_reference("b.rs")?,
                vec![LineChange {
                    line: 3, // Content line of the first block.
                    kind: LineChangeKind::Added,
                }],
            ),
        ]);
        let blocks_by_file = Scan::new(&file_system, &PathFilter::allow_all(), &parsers, &[])
            .parse_blocks(&line_changes, ScanMode::All)?
            .blocks;

        assert_eq!(
            blocks_by_file[&RepoPath::from_reference("a.rs")?]
                .blocks_with_context
                .iter()
                .map(|b| { (b.block.name().unwrap(), b.is_content_modified) })
                .collect::<Vec<(&str, bool)>>(),
            &[("first_from_a", true), ("second_from_a", false)]
        );
        assert_eq!(
            blocks_by_file[&RepoPath::from_reference("b.rs")?]
                .blocks_with_context
                .iter()
                .map(|b| { (b.block.name().unwrap(), b.is_content_modified) })
                .collect::<Vec<(&str, bool)>>(),
            &[("first_from_b", true), ("second_from_b", false)]
        );
        Ok(())
    }

    #[test]
    fn all_mode_without_line_changes_parses_unmodified_blocks() -> anyhow::Result<()> {
        let file_system = FakeFileSystem::new(HashMap::from([
            (
                "a.rs".to_string(),
                r#"
        // <block name="first_from_a">
        fn a() {}
        // </block>
        // <block name="second_from_a">
        fn b() {
            println!("hello");
        }
        // </block>
        "#
                .to_string(),
            ),
            (
                "b.rs".to_string(),
                r#"
        // <block name="first_from_b">
        fn a() {}
        // </block>
        // <block name="second_from_b">
        fn b() {
            println!("hello");
        }
        // </block>
        "#
                .to_string(),
            ),
        ]));
        let parsers = language_parsers()?;

        let blocks_by_file = Scan::new(&file_system, &PathFilter::allow_all(), &parsers, &[])
            .parse_blocks(&DiffChanges::default(), ScanMode::All)?
            .blocks;

        assert_eq!(
            blocks_by_file[&RepoPath::from_reference("a.rs")?]
                .blocks_with_context
                .iter()
                .map(|b| { (b.block.name().unwrap(), b.is_content_modified) })
                .collect::<Vec<(&str, bool)>>(),
            &[("first_from_a", false), ("second_from_a", false)]
        );
        assert_eq!(
            blocks_by_file[&RepoPath::from_reference("b.rs")?]
                .blocks_with_context
                .iter()
                .map(|b| { (b.block.name().unwrap(), b.is_content_modified) })
                .collect::<Vec<(&str, bool)>>(),
            &[("first_from_b", false), ("second_from_b", false)]
        );
        Ok(())
    }

    #[test]
    fn parsed_blocks_contain_original_file_content() -> anyhow::Result<()> {
        let file_a_contents = r#"
        // <block name="first">
        fn a() {}
        // </block>
        // <block name="second">
        fn b() {
            println!("hello");
            println!("world");
        }
        // </block>
        "#;
        let file_system = FakeFileSystem::new(HashMap::from([(
            "a.rs".to_string(),
            file_a_contents.to_string(),
        )]));
        let parsers = language_parsers()?;

        let blocks_by_file = Scan::new(&file_system, &PathFilter::allow_all(), &parsers, &[])
            .parse_blocks(&DiffChanges::default(), ScanMode::All)?
            .blocks;

        let content_a = &blocks_by_file[&RepoPath::from_reference("a.rs")?].file_content;
        assert_eq!(content_a, file_a_contents);
        Ok(())
    }

    #[test]
    fn with_remapped_extension_returns_parsed_blocks() -> anyhow::Result<()> {
        let file_system = FakeFileSystem::new(HashMap::from([(
            "a.rust".to_string(),
            r#"
        // <block name="first">
        fn a() {}
        // </block>"#
                .to_string(),
        )]));
        let parsers = language_parsers()?
            .with_extension_mappings(HashMap::from([("rust".into(), "rs".into())]));

        let blocks_by_file = Scan::new(&file_system, &PathFilter::allow_all(), &parsers, &[])
            .parse_blocks(&DiffChanges::default(), ScanMode::All)?
            .blocks;

        assert_eq!(blocks_by_file.len(), 1);
        assert_eq!(
            blocks_by_file[&RepoPath::from_reference("a.rust")?]
                .blocks_with_context
                .len(),
            1
        );
        Ok(())
    }

    #[test]
    fn with_unknown_extension_returns_empty_result() -> anyhow::Result<()> {
        let files = HashMap::from([("test.unknown".to_string(), "test content".to_string())]);

        let blocks = Scan::new(
            &FakeFileSystem::new(files),
            &PathFilter::allow_all(),
            &LanguageParsers::default(),
            &[],
        )
        .parse_blocks(&DiffChanges::default(), ScanMode::All)?
        .blocks;

        assert_eq!(blocks.len(), 0);
        Ok(())
    }

    #[test]
    fn with_allowed_and_ignored_paths_returns_block_from_allowed_paths_only() -> anyhow::Result<()>
    {
        let file_system = FakeFileSystem::new(HashMap::from([
            (
                "allowed.rs".to_string(),
                r#"
        // <block name="allowed">
        fn allowed() {}
        // </block>
        "#
                .to_string(),
            ),
            (
                "ignored.rs".to_string(),
                r#"
        // <block name="ignored">
        fn ignored() {}
        // </block>
        "#
                .to_string(),
            ),
        ]));
        let path_filter = PathFilter::with_ignored_paths(HashSet::from(["ignored.rs".to_string()]));

        let blocks = Scan::new(&file_system, &path_filter, &language_parsers()?, &[])
            .parse_blocks(&DiffChanges::default(), ScanMode::All)?
            .blocks;

        assert_eq!(blocks.len(), 1);
        assert!(blocks.contains_key(&RepoPath::from_reference("allowed.rs")?));
        assert!(!blocks.contains_key(&RepoPath::from_reference("ignored.rs")?));
        Ok(())
    }

    #[test]
    fn diff_target_outside_the_allowed_globs_is_not_parsed() -> anyhow::Result<()> {
        let file_system = FakeFileSystem::new(HashMap::from([
            (
                "src/allowed.rs".to_string(),
                "// <block name=\"allowed\">\nfn allowed() {}\n// </block>\n".to_string(),
            ),
            (
                "vendor/denied.rs".to_string(),
                "// <block name=\"denied\">\nfn denied() {}\n// </block>\n".to_string(),
            ),
        ]));
        let line_changes = diff_changes([
            (RepoPath::from_reference("src/allowed.rs")?, vec![added(2)]),
            (
                RepoPath::from_reference("vendor/denied.rs")?,
                vec![added(2)],
            ),
        ]);

        let blocks = Scan::new(
            &file_system,
            &PathFilter::allow_only("src/**"),
            &language_parsers()?,
            &[],
        )
        .parse_blocks(&line_changes, ScanMode::OnlyChanged)?
        .blocks;

        assert_eq!(blocks.len(), 1);
        assert!(blocks.contains_key(&RepoPath::from_reference("src/allowed.rs")?));
        Ok(())
    }

    #[test]
    fn all_mode_ignores_diff_entries_for_files_it_did_not_reach() -> anyhow::Result<()> {
        // In `All` mode the repository alone decides which files are read; the diff only marks
        // which of the blocks it found had changed.
        let file_system = FakeFileSystem::new(HashMap::from([(
            "present.rs".to_string(),
            "// <block name=\"present\">\nfn present() {}\n// </block>\n".to_string(),
        )]));
        let line_changes = diff_changes([
            (RepoPath::from_reference("present.rs")?, vec![added(1)]),
            (RepoPath::from_reference("absent.rs")?, vec![added(1)]),
        ]);

        let parsed = Scan::new(
            &file_system,
            &PathFilter::allow_all(),
            &language_parsers()?,
            &[],
        )
        .parse_blocks(&line_changes, ScanMode::All)?;

        assert_eq!(parsed.blocks.len(), 1);
        assert!(
            parsed
                .blocks
                .contains_key(&RepoPath::from_reference("present.rs")?)
        );
        assert_eq!(parsed.stats.files_scanned, 1);
        Ok(())
    }

    #[test]
    fn diff_with_a_missing_file_reports_the_likely_cause() -> anyhow::Result<()> {
        // What `diff.relative=true` produces: a well-formed path that matches no file.
        let line_changes = diff_changes([(
            RepoPath::from_reference("rules.py")?,
            vec![LineChange {
                line: 1,
                kind: LineChangeKind::Added,
            }],
        )]);
        let error = Scan::new(
            &FakeFileSystem::new(HashMap::from([("src/rules.py".to_string(), String::new())])),
            &PathFilter::allow_all(),
            &language_parsers()?,
            &[],
        )
        .parse_blocks(&line_changes, ScanMode::OnlyChanged)
        .unwrap_err();
        let message = format!("{error:#}");
        assert!(
            message.contains("does not exist in the repository root"),
            "unexpected error: {message}"
        );
        Ok(())
    }

    #[test]
    fn diff_with_only_missing_files_reports_the_likely_cause_in_all_mode() -> anyhow::Result<()> {
        // A diff where no path is valid marks no block as modified, so every rule that needs a
        // diff would go quiet, and the run would report success, which is undesirable.
        let line_changes = diff_changes([(RepoPath::from_reference("rules.py")?, vec![added(1)])]);
        let error = Scan::new(
            &FakeFileSystem::new(HashMap::from([("src/rules.py".to_string(), String::new())])),
            &PathFilter::allow_all(),
            &language_parsers()?,
            &[],
        )
        .parse_blocks(&line_changes, ScanMode::All)
        .unwrap_err();
        let message = format!("{error:#}");
        assert!(
            message.contains("does not exist in the repository root"),
            "unexpected error: {message}"
        );
        Ok(())
    }

    #[test]
    fn diff_with_several_missing_files_reports_the_lowest_path() -> anyhow::Result<()> {
        // The same diff must still give the same error on every run.
        let line_changes = diff_changes([
            (RepoPath::from_reference("c.py")?, vec![added(1)]),
            (RepoPath::from_reference("a.py")?, vec![added(1)]),
            (RepoPath::from_reference("b.py")?, vec![added(1)]),
        ]);
        let error = Scan::new(
            &FakeFileSystem::new(HashMap::new()),
            &PathFilter::allow_all(),
            &language_parsers()?,
            &[],
        )
        .parse_blocks(&line_changes, ScanMode::All)
        .unwrap_err();

        assert_eq!(
            format!("{error:#}"),
            "diff target \"a.py\" does not exist in the repository root"
        );
        Ok(())
    }

    #[test]
    fn only_changed_diff_with_a_missing_file_beside_a_present_one_reports_the_likely_cause()
    -> anyhow::Result<()> {
        // The present file makes the diff look valid, so the missing one is found only when the
        // scan reads it.
        let line_changes = diff_changes([
            (RepoPath::from_reference("present.py")?, vec![added(1)]),
            (RepoPath::from_reference("gone.py")?, vec![added(1)]),
        ]);
        let error = Scan::new(
            &FakeFileSystem::new(HashMap::from([("present.py".to_string(), String::new())])),
            &PathFilter::allow_all(),
            &language_parsers()?,
            &[],
        )
        .parse_blocks(&line_changes, ScanMode::OnlyChanged)
        .unwrap_err();

        let message = format!("{error:#}");
        assert!(
            message.contains("diff target \"gone.py\" does not exist in the repository root"),
            "unexpected error: {message}"
        );
        Ok(())
    }

    #[test]
    fn only_changed_diff_with_a_present_file_that_fails_to_parse_does_not_say_it_is_missing()
    -> anyhow::Result<()> {
        // The path is right, so blaming it would send the user to the wrong place.
        let line_changes = diff_changes([(RepoPath::from_reference("broken.py")?, vec![added(1)])]);
        let error = Scan::new(
            &FakeFileSystem::new(HashMap::from([(
                "broken.py".to_string(),
                "# <block name=\"open\">\nx = 1\n".to_string(),
            )])),
            &PathFilter::allow_all(),
            &language_parsers()?,
            &[],
        )
        .parse_blocks(&line_changes, ScanMode::OnlyChanged)
        .unwrap_err();

        let message = format!("{error:#}");
        assert!(
            message.contains("failed to parse file"),
            "unexpected error: {message}"
        );
        assert!(
            !message.contains("does not exist in the repository root"),
            "unexpected error: {message}"
        );
        Ok(())
    }

    #[test]
    fn diff_with_only_files_outside_the_globs_is_not_a_broken_diff() -> anyhow::Result<()> {
        // Globs narrow a run, so a diff with no path inside them is what the caller asked for.
        let line_changes =
            diff_changes([(RepoPath::from_reference("docs/gone.py")?, vec![added(1)])]);
        let blocks = Scan::new(
            &FakeFileSystem::new(HashMap::from([(
                "src/present.py".to_string(),
                "# <block name=\"present\">\nx = 1\n# </block>\n".to_string(),
            )])),
            &PathFilter::allow_only("src/**"),
            &language_parsers()?,
            &[],
        )
        .parse_blocks(&line_changes, ScanMode::All)?
        .blocks;

        assert!(blocks.contains_key(&RepoPath::from_reference("src/present.py")?));
        Ok(())
    }

    #[test]
    fn diff_with_a_missing_ignored_file_is_skipped() -> anyhow::Result<()> {
        // An ignored path contributes no blocks whether or not it exists, so it must not be able
        // to fail the run.
        let line_changes = diff_changes([(
            RepoPath::from_reference("vendor/gone.py")?,
            vec![LineChange {
                line: 1,
                kind: LineChangeKind::Added,
            }],
        )]);
        let blocks = Scan::new(
            &FakeFileSystem::new(HashMap::new()),
            &PathFilter::with_ignored_paths(HashSet::from(["vendor/gone.py".to_string()])),
            &language_parsers()?,
            &[],
        )
        .parse_blocks(&line_changes, ScanMode::OnlyChanged)?
        .blocks;
        assert!(blocks.is_empty());
        Ok(())
    }

    #[test]
    fn diff_with_a_missing_unparseable_file_is_skipped() -> anyhow::Result<()> {
        // Likewise for a file whose extension maps to no language: a diff routinely carries binary
        // assets and lockfiles that are absent from a partial checkout.
        let line_changes = diff_changes([(
            RepoPath::from_reference("assets/logo.png")?,
            vec![LineChange {
                line: 1,
                kind: LineChangeKind::Added,
            }],
        )]);
        let blocks = Scan::new(
            &FakeFileSystem::new(HashMap::new()),
            &PathFilter::allow_all(),
            &language_parsers()?,
            &[],
        )
        .parse_blocks(&line_changes, ScanMode::OnlyChanged)?
        .blocks;
        assert!(blocks.is_empty());
        Ok(())
    }

    #[test]
    fn empty_input_returns_empty_result() -> anyhow::Result<()> {
        let line_changes = DiffChanges::default();
        let blocks = Scan::new(
            &FakeFileSystem::new(HashMap::default()),
            &PathFilter::allow_all(),
            &LanguageParsers::default(),
            &[],
        )
        .parse_blocks(&line_changes, ScanMode::All)?
        .blocks;

        assert_eq!(blocks.len(), 0);
        Ok(())
    }

    #[test]
    fn parse_file_with_every_block_returns_all_blocks() -> anyhow::Result<()> {
        let file_system = FakeFileSystem::new(HashMap::from([(
            "a.py".to_string(),
            "# <block name=\"x\">\n1\n# </block>\n# <block name=\"y\">\n2\n# </block>".to_string(),
        )]));
        let parsers = language_parsers()?;
        let file_blocks = parse_file(
            &file_system,
            Path::new("a.py"),
            &LineChanges::default(),
            ScanMode::All,
            &parsers,
            &[],
        )?
        .expect("python is supported");
        assert_eq!(file_blocks.blocks_with_context.len(), 2);
        Ok(())
    }

    #[test]
    fn unknown_attribute_in_block_fails_with_error() -> anyhow::Result<()> {
        let file_system = FakeFileSystem::new(HashMap::from([(
            "a.py".to_string(),
            "# <block name=\"x\" unknown-attr=\"value\">\n1\n# </block>".to_string(),
        )]));
        let parsers = language_parsers()?;
        let file_blocks = parse_file(
            &file_system,
            Path::new("a.py"),
            &LineChanges::default(),
            ScanMode::All,
            &parsers,
            &[],
        );

        assert!(file_blocks.is_err());
        assert_eq!(
            file_blocks.unwrap_err().source().unwrap().to_string(),
            "block x at line 1 contains unrecognized attribute `unknown-attr`"
        );
        Ok(())
    }

    #[test]
    fn unknown_severity_value_in_block_fails_with_error() -> anyhow::Result<()> {
        let file_system = FakeFileSystem::new(HashMap::from([(
            "a.py".to_string(),
            "# <block severity=\"invalid-severity\">\n1\n# </block>".to_string(),
        )]));
        let parsers = language_parsers()?;
        let file_blocks = parse_file(
            &file_system,
            Path::new("a.py"),
            &LineChanges::default(),
            ScanMode::All,
            &parsers,
            &[],
        );

        assert!(file_blocks.is_err());
        assert_eq!(
            file_blocks.unwrap_err().source().unwrap().to_string(),
            "block (unnamed) at line 1 contains unrecognized severity value `invalid-severity`"
        );
        Ok(())
    }

    #[test]
    fn duplicate_block_name_in_the_same_file_fails_with_error() -> anyhow::Result<()> {
        let file_system = FakeFileSystem::new(HashMap::from([(
            "a.py".to_string(),
            "# <block name=\"x\">\n1\n# </block>\n# <block name=\"x\">\n2\n# </block>".to_string(),
        )]));
        let parsers = language_parsers()?;
        let file_blocks = parse_file(
            &file_system,
            Path::new("a.py"),
            &LineChanges::default(),
            ScanMode::All,
            &parsers,
            &[],
        );

        assert!(file_blocks.is_err());
        assert_eq!(
            file_blocks.unwrap_err().source().unwrap().to_string(),
            "block a.py:x at line 4 duplicates the name of the block at line 1"
        );
        Ok(())
    }

    #[test]
    fn end_tag_without_a_start_tag_fails_with_error() -> anyhow::Result<()> {
        // A file without a start tag must still be parsed, so that its end tag fails the run.
        let file_system = FakeFileSystem::new(HashMap::from([(
            "a.py".to_string(),
            "1\n# </block>".to_string(),
        )]));
        let parsers = language_parsers()?;
        let file_blocks = parse_file(
            &file_system,
            Path::new("a.py"),
            &LineChanges::default(),
            ScanMode::All,
            &parsers,
            &[],
        );

        assert!(file_blocks.is_err());
        assert_eq!(
            file_blocks.unwrap_err().source().unwrap().to_string(),
            "unexpected closed block at line 2, column 3. If this is not meant as a tag, put it in backticks"
        );
        Ok(())
    }

    #[test]
    fn start_tag_without_an_end_tag_fails_with_error() -> anyhow::Result<()> {
        // A file without an end tag must still be parsed, so that its start tag fails the run.
        let file_system = FakeFileSystem::new(HashMap::from([(
            "a.py".to_string(),
            "# <block>\n1".to_string(),
        )]));
        let parsers = language_parsers()?;
        let file_blocks = parse_file(
            &file_system,
            Path::new("a.py"),
            &LineChanges::default(),
            ScanMode::All,
            &parsers,
            &[],
        );

        assert!(file_blocks.is_err());
        assert_eq!(
            file_blocks.unwrap_err().source().unwrap().to_string(),
            "block at line 1, column 3 is not closed. If this is not meant as a tag, put it in backticks"
        );
        Ok(())
    }

    #[test]
    fn same_block_name_in_different_files_does_not_fail() -> anyhow::Result<()> {
        let file_system = FakeFileSystem::new(HashMap::from([
            (
                "a.py".to_string(),
                "# <block name=\"x\">\n1\n# </block>".to_string(),
            ),
            (
                "b.py".to_string(),
                "# <block name=\"x\">\n2\n# </block>".to_string(),
            ),
        ]));
        let parsers = language_parsers()?;

        let blocks = Scan::new(&file_system, &PathFilter::allow_all(), &parsers, &[])
            .parse_blocks(&DiffChanges::default(), ScanMode::All)?
            .blocks;

        assert_eq!(blocks.len(), 2);
        Ok(())
    }

    /// The blocks of a run over `files` whose config file declares `virtual_blocks`, with no
    /// diff.
    fn parse_with_virtual_blocks(
        files: &[(&str, &str)],
        path_filter: &PathFilter,
        virtual_blocks: &[VirtualBlock],
    ) -> anyhow::Result<HashMap<RepoPath, FileBlocks>> {
        let files = files
            .iter()
            .map(|(path, content)| (path.to_string(), content.to_string()))
            .collect();
        Ok(Scan::new(
            &FakeFileSystem::new(files),
            path_filter,
            &language_parsers()?,
            virtual_blocks,
        )
        .parse_blocks(&DiffChanges::default(), ScanMode::All)?
        .blocks)
    }

    #[test]
    fn virtual_block_parse_blocks_lists_it_in_source_order_with_its_config_line()
    -> anyhow::Result<()> {
        let source = "{\n  \"a\": 1,\n  // <block name=\"tag\">\n  \"b\": 2\n  // </block>\n}\n";
        let virtual_blocks = [test_utils::virtual_block(
            "a.json#/a",
            &[("name", "virtual")],
        )?];

        let blocks = parse_with_virtual_blocks(
            &[("a.json", source)],
            &PathFilter::allow_all(),
            &virtual_blocks,
        )?;

        let mut listed = Vec::new();
        for block in &blocks[&RepoPath::from_reference("a.json")?].blocks_with_context {
            let listing = serde_json::to_value(BlockListing::from(block))?;
            listed.push((listing["name"].clone(), listing.get("config_line").cloned()));
        }
        assert_eq!(
            listed,
            vec![
                (serde_json::json!("virtual"), Some(serde_json::json!(7))),
                (serde_json::json!("tag"), None),
            ]
        );
        Ok(())
    }

    #[test]
    fn only_changed_parse_blocks_keeps_the_virtual_blocks_the_diff_touched() -> anyhow::Result<()> {
        let file_system = FakeFileSystem::new(HashMap::from([(
            "a.json".to_string(),
            "{\n  \"a\": 1,\n  \"b\": 2\n}\n".to_string(),
        )]));
        // The diff does not touch `gone.json`, so it is not read, and that it is missing is not an
        // error.
        let virtual_blocks = [
            test_utils::virtual_block("a.json#/a", &[("name", "a")])?,
            test_utils::virtual_block("a.json#/b", &[("name", "b")])?,
            test_utils::virtual_block("gone.json#/x", &[])?,
        ];
        let line_changes = diff_changes([(RepoPath::from_reference("a.json")?, vec![added(3)])]);

        let blocks = Scan::new(
            &file_system,
            &PathFilter::allow_all(),
            &language_parsers()?,
            &virtual_blocks,
        )
        .parse_blocks(&line_changes, ScanMode::OnlyChanged)?
        .blocks;

        let names: Vec<Option<&str>> = blocks[&RepoPath::from_reference("a.json")?]
            .blocks_with_context
            .iter()
            .map(|block_with_context| block_with_context.block.name())
            .collect();
        assert_eq!(names, vec![Some("b")]);
        Ok(())
    }

    #[test]
    fn only_changed_parse_blocks_reads_the_file_of_a_modified_entry() -> anyhow::Result<()> {
        let source = r#"{
  "a": 1,
  // <block name="tag">
  "b": 2,
  // </block>
  "c": 3
}
"#;
        let file_system = FakeFileSystem::new(HashMap::from([
            ("a.json".to_string(), source.to_string()),
            ("b.json".to_string(), source.to_string()),
        ]));
        let modified_entry = |target: &str| -> anyhow::Result<VirtualBlock> {
            let mut virtual_block = test_utils::virtual_block(target, &[("name", "a")])?;
            virtual_block.is_entry_modified = true;
            Ok(virtual_block)
        };
        // The diff touches only the tag in `b.json`. `vendor/gone.json` is outside the globs, so
        // it is not read even though its entry is modified.
        let virtual_blocks = [
            modified_entry("a.json#/a")?,
            test_utils::virtual_block("a.json#/c", &[("name", "c")])?,
            modified_entry("b.json#/a")?,
            modified_entry("vendor/gone.json#/x")?,
        ];
        let line_changes = diff_changes([(RepoPath::from_reference("b.json")?, vec![added(4)])]);

        let parsed = Scan::new(
            &file_system,
            &PathFilter::allow_only("{a,b}.json"),
            &language_parsers()?,
            &virtual_blocks,
        )
        .parse_blocks(&line_changes, ScanMode::OnlyChanged)?;

        let names = |file: &str| -> anyhow::Result<Vec<Option<&str>>> {
            Ok(parsed.blocks[&RepoPath::from_reference(file)?]
                .blocks_with_context
                .iter()
                .map(|block_with_context| block_with_context.block.name())
                .collect())
        };
        assert_eq!(names("a.json")?, vec![Some("a")]);
        assert_eq!(names("b.json")?, vec![Some("a"), Some("tag")]);
        // Each file is read once, even when both the diff and an entry lead to it.
        assert_eq!(parsed.stats.files_scanned, 2);
        Ok(())
    }

    #[test]
    fn only_changed_with_a_modified_entry_for_a_missing_file_parse_blocks_returns_error()
    -> anyhow::Result<()> {
        let mut virtual_block = test_utils::virtual_block("gone.json#/x", &[])?;
        virtual_block.is_entry_modified = true;

        let error = Scan::new(
            &FakeFileSystem::new(HashMap::new()),
            &PathFilter::allow_all(),
            &language_parsers()?,
            &[virtual_block],
        )
        .parse_blocks(&DiffChanges::default(), ScanMode::OnlyChanged)
        .unwrap_err();

        assert_eq!(
            format!("{error:#}"),
            "invalid block at line 7 of \"blockwatch.toml\": target gone.json#/x does not \
             resolve: file does not exist"
        );
        Ok(())
    }

    #[test]
    fn target_file_outside_the_globs_parse_blocks_skips_its_virtual_blocks() -> anyhow::Result<()> {
        // Neither target resolves: one symbol is missing, and the other file is.
        let virtual_blocks = [
            test_utils::virtual_block("vendor/a.json#/missing", &[])?,
            test_utils::virtual_block("vendor/gone.json#/x", &[])?,
        ];

        let blocks = parse_with_virtual_blocks(
            &[("vendor/a.json", "{}")],
            &PathFilter::allow_only("src/**"),
            &virtual_blocks,
        )?;

        assert!(blocks.is_empty());
        Ok(())
    }

    #[test]
    fn ignored_target_file_parse_blocks_skips_its_virtual_blocks() -> anyhow::Result<()> {
        // Neither target resolves: one symbol is missing, and the other file is.
        let virtual_blocks = [
            test_utils::virtual_block("vendor/a.json#/missing", &[])?,
            test_utils::virtual_block("vendor/gone.json#/x", &[])?,
        ];

        let blocks = parse_with_virtual_blocks(
            &[("vendor/a.json", "{}")],
            &PathFilter::with_ignored_paths(HashSet::from([
                "vendor/a.json".to_string(),
                "vendor/gone.json".to_string(),
            ])),
            &virtual_blocks,
        )?;

        assert!(blocks.is_empty());
        Ok(())
    }

    #[test]
    fn missing_target_file_parse_blocks_returns_error() -> anyhow::Result<()> {
        let virtual_blocks = [test_utils::virtual_block("gone.json#/x", &[])?];

        let error =
            parse_with_virtual_blocks(&[], &PathFilter::allow_all(), &virtual_blocks).unwrap_err();

        assert_eq!(
            format!("{error:#}"),
            "invalid block at line 7 of \"blockwatch.toml\": target gone.json#/x does not \
             resolve: file does not exist"
        );
        Ok(())
    }

    #[test]
    fn target_file_without_a_parser_parse_blocks_reads_its_whole_file_block() -> anyhow::Result<()>
    {
        let virtual_blocks = [test_utils::virtual_block(
            "notes.txt",
            &[("name", "notes")],
        )?];

        let blocks = parse_with_virtual_blocks(
            &[("notes.txt", "x\n")],
            &PathFilter::allow_all(),
            &virtual_blocks,
        )?;

        let names: Vec<Option<&str>> = blocks[&RepoPath::from_reference("notes.txt")?]
            .blocks_with_context
            .iter()
            .map(|block_with_context| block_with_context.block.name())
            .collect();
        assert_eq!(names, [Some("notes")]);
        Ok(())
    }

    #[test]
    fn target_file_without_symbols_parse_blocks_reads_its_whole_file_block() -> anyhow::Result<()> {
        let virtual_blocks = [test_utils::virtual_block("a.py", &[("name", "a")])?];

        let blocks = parse_with_virtual_blocks(
            &[("a.py", "x = 1\n")],
            &PathFilter::allow_all(),
            &virtual_blocks,
        )?;

        assert_eq!(
            blocks[&RepoPath::from_reference("a.py")?].blocks_with_context[0]
                .block
                .name(),
            Some("a")
        );
        Ok(())
    }

    #[test]
    fn virtual_block_in_a_file_without_a_parser_non_utf8_file_has_blocks_returns_true()
    -> anyhow::Result<()> {
        let virtual_block = test_utils::virtual_block("notes.txt", &[])?;

        // `é` in Latin-1 is the single byte 0xE9, which is not valid UTF-8.
        assert!(non_utf8_file_has_blocks(
            None,
            Path::new("notes.txt"),
            b"caf\xe9",
            &[&virtual_block]
        ));
        Ok(())
    }

    #[test]
    fn target_file_without_a_parser_parse_blocks_returns_error() -> anyhow::Result<()> {
        // The whole-file block needs no parser, so the error shows the symbol's block.
        let virtual_blocks = [
            test_utils::virtual_block("notes.txt", &[])?,
            test_utils::virtual_block("notes.txt#/x", &[])?,
        ];

        let error = parse_with_virtual_blocks(
            &[("notes.txt", "x")],
            &PathFilter::allow_all(),
            &virtual_blocks,
        )
        .unwrap_err();

        assert_eq!(
            format!("{error:#}"),
            "invalid block at line 7 of \"blockwatch.toml\": target notes.txt#/x does not \
             resolve: file format is unsupported"
        );
        Ok(())
    }

    #[test]
    fn target_file_without_symbols_parse_blocks_returns_error() -> anyhow::Result<()> {
        // The whole-file block needs no symbols, so the error shows the symbol's block.
        let virtual_blocks = [
            test_utils::virtual_block("a.py", &[])?,
            test_utils::virtual_block("a.py#/x", &[])?,
        ];

        let error = parse_with_virtual_blocks(
            &[("a.py", "x = 1\n")],
            &PathFilter::allow_all(),
            &virtual_blocks,
        )
        .unwrap_err();

        assert_eq!(
            format!("{error:#}"),
            "invalid block at line 7 of \"blockwatch.toml\": target a.py#/x does not resolve: \
             symbols are not supported for this language"
        );
        Ok(())
    }

    #[test]
    fn virtual_block_with_the_name_of_a_tag_parse_blocks_returns_error() -> anyhow::Result<()> {
        let source = "{\n  \"a\": 1,\n  // <block name=\"n\">\n  \"b\": 2\n  // </block>\n}\n";
        let virtual_blocks = [test_utils::virtual_block("a.json#/a", &[("name", "n")])?];

        let error = parse_with_virtual_blocks(
            &[("a.json", source)],
            &PathFilter::allow_all(),
            &virtual_blocks,
        )
        .unwrap_err();

        assert_eq!(
            format!("{error:#}"),
            "block a.json:n at line 7 of \"blockwatch.toml\" duplicates the name of the block at \
             line 3"
        );
        Ok(())
    }
}
