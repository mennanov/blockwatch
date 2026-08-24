use crate::Position;
use crate::diff_parser::LineChange;
use crate::fs::{FileSystem, PathChecker};
use crate::language_parsers::{LanguageParser, LanguageParsers};
use crate::repo_path::RepoPath;
use anyhow::{Context, anyhow, bail};
use serde_repr::Serialize_repr;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::ffi::OsString;
use std::ops::{Range, RangeInclusive};
use std::path::Path;
use std::str::FromStr;
use strum_macros::EnumString;

const UNNAMED_BLOCK_LABEL: &str = "(unnamed)";

/// Represents a `block` tag parsed from the source file comments.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Block {
    /// Optional attributes in the `block` tag. Their names are what selects the validators that
    /// will check this block (`affects`, `keep-sorted`, …).
    pub(crate) attributes: HashMap<String, String>,
    /// Block's start tag position range ("<" symbol to ">" symbol). Doubles as the block's identity
    /// in reports, since a block need not have a `name`.
    pub(crate) start_tag_position_range: RangeInclusive<Position>,
    /// Block's content substring range in the original source code.
    pub(crate) content_bytes_range: Range<usize>,
    /// Block's content position range in the original source code (from the end of the comment with
    /// the start tag to the beginning of the comment with the end tag). Compared against the diff's
    /// line changes to decide whether the block was touched.
    pub(crate) content_position_range: Range<Position>,
}

impl PartialOrd for Block {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Block {
    fn cmp(&self, other: &Self) -> Ordering {
        self.start_tag_position_range
            .start()
            .cmp(other.start_tag_position_range.start())
    }
}

impl Block {
    /// Creates a new `Block` with the given attributes and content indexes.
    pub(crate) fn new(
        attributes: HashMap<String, String>,
        start_tag_position_range: RangeInclusive<Position>,
        content_range: Range<usize>,
        content_position_range: Range<Position>,
    ) -> Self {
        Self {
            attributes,
            start_tag_position_range,
            content_bytes_range: content_range,
            content_position_range,
        }
    }

    /// Whether the `Block`'s content intersects with any of the **ordered** `line_changes`.
    fn content_intersects_with_any(&self, line_changes: &[LineChange]) -> bool {
        Self::changes_on_lines(
            line_changes,
            self.content_position_range.start.line,
            self.content_position_range.end.line,
        )
        .any(|line_change| {
            Self::intersects_with_line_change(&self.content_position_range, line_change)
        })
    }

    /// Whether the `Block`'s start tag intersects with any of the **ordered** `line_changes`.
    fn start_tag_intersects_with_any(&self, line_changes: &[LineChange]) -> bool {
        Self::changes_on_lines(
            line_changes,
            self.start_tag_position_range.start().line,
            self.start_tag_position_range.end().line,
        )
        .any(|line_change| {
            Self::intersects_with_line_change_inclusive(&self.start_tag_position_range, line_change)
        })
    }

    /// The **ordered** `line_changes` that fall on lines `first_line..=last_line`, in order.
    fn changes_on_lines(
        line_changes: &[LineChange],
        first_line: usize,
        last_line: usize,
    ) -> impl Iterator<Item = &LineChange> {
        let first = line_changes.partition_point(|line_change| line_change.line < first_line);
        line_changes[first..]
            .iter()
            .take_while(move |line_change| line_change.line <= last_line)
    }

    /// Whether the `position_range` intersects with the given `line_change`.
    fn intersects_with_line_change_inclusive(
        position_range: &RangeInclusive<Position>,
        line_change: &LineChange,
    ) -> bool {
        if line_change.line < position_range.start().line {
            return false;
        }
        if line_change.line > position_range.end().line {
            return false;
        }

        if let Some(ranges) = &line_change.ranges {
            let start_character = if line_change.line == position_range.start().line {
                position_range.start().character - 1 // LineChange.ranges are 0-based
            } else {
                0
            };
            let end_character = if line_change.line < position_range.end().line {
                usize::MAX
            } else {
                position_range.end().character - 1 // LineChange.ranges are 0-based
            };

            ranges
                .binary_search_by(|range| {
                    if range.end > start_character && range.start <= end_character {
                        // Intersection between [start_character, end_character]
                        // and half-open [range.start, range.end).
                        Ordering::Equal
                    } else if range.end <= start_character {
                        Ordering::Less
                    } else {
                        Ordering::Greater
                    }
                })
                .is_ok()
        } else {
            true
        }
    }

    /// Whether the `position_range` intersects with the given `line_change`.
    fn intersects_with_line_change(
        position_range: &Range<Position>,
        line_change: &LineChange,
    ) -> bool {
        if line_change.line < position_range.start.line {
            return false;
        }
        if line_change.line > position_range.end.line {
            return false;
        }

        if let Some(ranges) = &line_change.ranges {
            let start_character = if line_change.line == position_range.start.line {
                position_range.start.character - 1 // LineChange.ranges are 0-based
            } else {
                0
            };
            let end_character = if line_change.line < position_range.end.line {
                usize::MAX
            } else {
                position_range.end.character - 1 // LineChange.ranges are 0-based
            };

            ranges
                .binary_search_by(|range| {
                    if range.end > start_character && range.start < end_character {
                        // Intersection between closed-open [start_character, end_character)
                        // and closed-open [range.start, range.end).
                        Ordering::Equal
                    } else if range.end <= start_character {
                        Ordering::Less
                    } else {
                        Ordering::Greater
                    }
                })
                .is_ok()
        } else {
            true
        }
    }

    /// Returns the optional value of the `name` attribute for this block.
    pub(crate) fn name(&self) -> Option<&str> {
        self.attributes.get("name").map(String::as_str)
    }

    /// Returns the block's name if present, otherwise a human-friendly placeholder label.
    pub(crate) fn name_display(&self) -> &str {
        self.name().unwrap_or(UNNAMED_BLOCK_LABEL)
    }

    /// Returns the block's content from the given `source`.
    pub(crate) fn content<'source>(&self, source: &'source str) -> &'source str {
        &source[self.content_bytes_range.clone()]
    }

    /// Maps a position inside the block's content onto the position in the original source.
    ///
    /// `content_line_idx` is the 0-based index of a line of [`Self::content`],
    /// `column_offset` is the 0-based offset within that line.
    pub(crate) fn content_position(
        &self,
        content_line_idx: usize,
        column_offset: usize,
    ) -> Position {
        let line_start_column = if content_line_idx == 0 {
            self.content_position_range.start.character
        } else {
            1
        };
        Position::new(
            self.content_position_range.start.line + content_line_idx,
            line_start_column + column_offset,
        )
    }

    /// Returns the block's severity.
    pub(crate) fn severity(&self) -> anyhow::Result<BlockSeverity> {
        self.attributes
            .get("severity")
            .map_or(Ok(BlockSeverity::Error), |s| {
                BlockSeverity::from_str(s.as_str())
                    .context(format!("Invalid \"severity\" attribute value \"{}\"", s))
            })
    }
}

/// Block's severity.
///
/// Mirrors [LSP DiagnosticSeverity](https://github.com/microsoft/vscode-languageserver-node/blob/3412a17149850f445bf35b4ad71148cfe5f8411e/types/src/main.ts#L614)
#[derive(Clone, Copy, Serialize_repr, EnumString, Debug, PartialEq)]
#[strum(ascii_case_insensitive)]
#[repr(u8)]
pub enum BlockSeverity {
    /// The default. A violation at this level makes the run exit non-zero, failing a hook or CI.
    Error = 1,
    /// Reported like an error but does not affect the exit code.
    Warning = 2,
    /// Reported for information only; does not affect the exit code.
    Info = 3,
    /// The weakest level, for suggestions, does not affect the exit code.
    Hint = 4,
}

/// Represents a source field with its corresponding modified blocks.
#[derive(Debug)]
pub struct FileBlocks {
    /// Source file contents.
    pub(crate) file_content: String,
    /// Blocks to be validated.
    pub(crate) blocks_with_context: Vec<BlockWithContext>,
}

impl FileBlocks {
    fn is_empty(&self) -> bool {
        self.blocks_with_context.is_empty()
    }

    /// Converts the file blocks to a serializable report.
    ///
    /// The listing is already deterministic without a sorting pass: the parser yields blocks in
    /// start-tag order, and filtering preserves it.
    pub(crate) fn to_serializable_report(&self) -> Vec<serde_json::Value> {
        self.blocks_with_context
            .iter()
            .map(|block| {
                serde_json::json!({
                    // <block affects="docs/cli.md:list-output-example">
                    "name": block.block.name_display(),
                    "line": block.block.start_tag_position_range.start().line,
                    "column": block.block.start_tag_position_range.start().character,
                    "is_content_modified": block.is_content_modified,
                    "attributes": block.block.attributes,
                    // </block>
                })
            })
            .collect()
    }
}

/// Represents a block with its corresponding validation context.
#[derive(Debug, Clone)]
pub struct BlockWithContext {
    /// The block itself, as parsed from the source comment.
    pub(crate) block: Block,
    /// Whether the block's start tag is modified (computed from the input diff).
    pub(crate) is_start_tag_modified: bool,
    /// Whether the content of the block is modified (computed from the input diff). Validators such
    /// as `affects` fire only for blocks whose content actually changed.
    pub(crate) is_content_modified: bool,
}

/// Counts of the files a scan looked at.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ScanStats {
    /// Number of files that were read and parsed for blocks.
    pub files_scanned: usize,
    /// Number of files that were not parsed because their extension has no parser.
    pub files_skipped: usize,
}

/// The blocks found in each file, together with the counts of files the scan looked at.
#[derive(Debug, Default)]
pub struct ParsedBlocks {
    /// The blocks to validate, grouped by the file they were found in.
    pub blocks: HashMap<RepoPath, FileBlocks>,
    /// File counts for the run report; carried alongside the blocks because only the scan knows
    /// how many files it looked at but produced no blocks for.
    pub stats: ScanStats,
}

/// How a run decides which files it reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanMode {
    /// Read every file in the repository. If a diff is supplied then the corresponding blocks are
    /// marked as modified.
    All,
    /// Read only the files from the supplied diff, keeping just the blocks that the diff modified.
    OnlyChanged,
}

/// Parses source files into the blocks a run should consider, together with the counts of files
/// it looked at.
///
/// - `line_changes_by_file` maps file paths to sorted line changes.
/// - `scan_mode` chooses which set of files is read; see [`ScanMode`].
/// - `file_system` provides access to file contents within a root path.
/// - `parsers` maps file extensions to language-specific block parsers.
/// - `extra_file_extensions` allows remapping unknown extensions to supported ones (e.g., "cxx" -> "cpp").
///
/// In either mode a file is read only if it passes the allow globs and is not ignored.
///
/// Fails if `line_changes_by_file` is invalid, e.g. it refers to files that do not exist.
pub fn parse_blocks(
    line_changes_by_file: &HashMap<RepoPath, Vec<LineChange>>,
    scan_mode: ScanMode,
    file_system: &impl FileSystem,
    path_checker: &impl PathChecker,
    parsers: &LanguageParsers,
    extra_file_extensions: &HashMap<OsString, OsString>,
) -> anyhow::Result<ParsedBlocks> {
    ensure_diff_has_valid_paths(
        line_changes_by_file,
        file_system,
        path_checker,
        parsers,
        extra_file_extensions,
    )?;
    match scan_mode {
        ScanMode::All => parse_all_files(
            line_changes_by_file,
            file_system,
            path_checker,
            parsers,
            extra_file_extensions,
        ),
        ScanMode::OnlyChanged => parse_changed_files(
            line_changes_by_file,
            file_system,
            path_checker,
            parsers,
            extra_file_extensions,
        ),
    }
}

/// Rejects a diff with no valid file paths.
///
/// One invalid path is normal: deletions, generated files, paths outside the globs. But a diff
/// where *no* path is valid is itself likely invalid: it marks no block as modified, so every rule
/// that needs a diff would pass silently.
fn ensure_diff_has_valid_paths(
    line_changes_by_file: &HashMap<RepoPath, Vec<LineChange>>,
    file_system: &impl FileSystem,
    path_checker: &impl PathChecker,
    parsers: &LanguageParsers,
    extra_file_extensions: &HashMap<OsString, OsString>,
) -> anyhow::Result<()> {
    let mut invalid_path: Option<&RepoPath> = None;
    for file_path in line_changes_by_file.keys() {
        // Only paths this run would have opened count: past the filters, with a known extension.
        if !path_checker.should_allow(file_path) || path_checker.should_ignore(file_path) {
            continue;
        }
        if parser_for_file_path(file_path.as_path(), parsers, extra_file_extensions).is_none() {
            continue;
        }
        if file_system.exists(file_path.as_path()) {
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

fn invalid_diff_target_message(file_path: &RepoPath) -> String {
    format!("diff target \"{file_path}\" does not exist in the repository root.")
}

/// Parses every block in every file of the repository, marking the ones the line changes touched.
/// Line changes for files the walk did not reach contribute nothing to the result; the walk
/// defines the scope, and the diff only says which of the blocks it found had changed.
fn parse_all_files(
    line_changes_by_file: &HashMap<RepoPath, Vec<LineChange>>,
    file_system: &impl FileSystem,
    path_checker: &impl PathChecker,
    parsers: &LanguageParsers,
    extra_file_extensions: &HashMap<OsString, OsString>,
) -> anyhow::Result<ParsedBlocks> {
    let mut parsed = ParsedBlocks::default();
    for repo_path_result in file_system.walk() {
        let file_path =
            repo_path_result.map_err(|err| anyhow!("Failed to walk directory: {err}"))?;
        if !path_checker.should_allow(&file_path) || path_checker.should_ignore(&file_path) {
            continue;
        }
        let line_changes = line_changes_by_file
            .get(&file_path)
            .map_or(&[][..], Vec::as_slice);
        let file_blocks = parse_file(
            file_system,
            file_path.as_path(),
            line_changes,
            every_block,
            parsers,
            extra_file_extensions,
        )?;
        record_parsed_file(&mut parsed, file_path, file_blocks);
    }
    Ok(parsed)
}

/// Parses only the files in the diff, keeping the blocks whose start tag or content it modified.
fn parse_changed_files(
    line_changes_by_file: &HashMap<RepoPath, Vec<LineChange>>,
    file_system: &impl FileSystem,
    path_checker: &impl PathChecker,
    parsers: &LanguageParsers,
    extra_file_extensions: &HashMap<OsString, OsString>,
) -> anyhow::Result<ParsedBlocks> {
    let mut parsed = ParsedBlocks::default();
    for (file_path, line_changes) in line_changes_by_file {
        if !path_checker.should_allow(file_path) || path_checker.should_ignore(file_path) {
            continue;
        }
        let file_blocks = parse_file(
            file_system,
            file_path.as_path(),
            line_changes,
            modified_blocks,
            parsers,
            extra_file_extensions,
        )
        .map_err(|error| {
            // Only a file this run validates reaches a read: filtered-out paths are skipped above,
            // and one with no language parser returns before its contents are read.
            if file_system.exists(file_path.as_path()) {
                error
            } else {
                error.context(invalid_diff_target_message(file_path))
            }
        })?;
        record_parsed_file(&mut parsed, file_path.clone(), file_blocks);
    }
    Ok(parsed)
}

/// Folds one file's parse result into the accumulator. `None` means no parser claimed the file's
/// extension, which counts as skipped rather than scanned.
fn record_parsed_file(
    parsed: &mut ParsedBlocks,
    file_path: RepoPath,
    file_blocks: Option<FileBlocks>,
) {
    match file_blocks {
        Some(file_blocks) => {
            parsed.stats.files_scanned += 1;
            // A file that parsed cleanly but declares no blocks still counts as scanned; it is
            // simply nothing for the validators to work on.
            if !file_blocks.is_empty() {
                parsed.blocks.insert(file_path, file_blocks);
            }
        }
        None => parsed.stats.files_skipped += 1,
    }
}

/// Keeps every block the file declares, whether or not a diff touched it.
pub fn every_block(_block: &BlockWithContext) -> bool {
    true
}

/// Keeps only the blocks a diff touched, by their content or by their start tag.
pub fn modified_blocks(block: &BlockWithContext) -> bool {
    // A block with a modified start tag is considered modified because its rules (attributes) are
    // modified.
    block.is_content_modified || block.is_start_tag_modified
}

/// Parses the blocks of one file, marking the ones `line_changes` touched and keeping those
/// `block_predicate` selects. Returns `None` for unsupported file extensions.
pub fn parse_file(
    file_system: &impl FileSystem,
    file_path: &Path,
    line_changes: &[LineChange],
    block_predicate: impl Fn(&BlockWithContext) -> bool,
    parsers: &LanguageParsers,
    extra_file_extensions: &HashMap<OsString, OsString>,
) -> anyhow::Result<Option<FileBlocks>> {
    let parser = match parser_for_file_path(file_path, parsers, extra_file_extensions) {
        None => return Ok(None),
        Some(p) => p,
    };
    let source_code = file_system.read_to_string(file_path)?;
    // Tracks each named block's first position in this file, to reject duplicate blocks.
    let mut names_seen: HashMap<String, Position> = HashMap::new();
    // Blocks are filtered as the parser yields them, so only the ones this run will validate are
    // ever held. The parser's lock lives until the end of the statement, which is as long as the
    // iterator borrowing it does.
    let blocks_with_context = parser
        .lock()
        .expect("no active locks")
        .parse(&source_code)
        .filter_map(|block| {
            let block = match block {
                Ok(block) => block,
                Err(error) => return Some(Err(error)),
            };
            if let Err(err) = validate_block_syntax(&block, file_path) {
                return Some(Err(err));
            }
            if let Err(err) = reject_duplicate_name(&block, file_path, &mut names_seen) {
                return Some(Err(err));
            }
            let block_with_context = BlockWithContext {
                is_content_modified: block.content_intersects_with_any(line_changes),
                is_start_tag_modified: block.start_tag_intersects_with_any(line_changes),
                block,
            };
            block_predicate(&block_with_context).then_some(Ok(block_with_context))
        })
        .collect::<anyhow::Result<Vec<_>>>()
        .context(format!("Failed to parse file {file_path:?}"))?;

    Ok(Some(FileBlocks {
        file_content: source_code,
        blocks_with_context,
    }))
}

const RECOGNIZED_ATTRIBUTES: &[&str] = &[
    // <block keep-sorted>
    "affects",
    "check-ai",
    "check-ai-pattern",
    "check-lua",
    "check-lua-pattern",
    "check-lua-timeout",
    "keep-sorted",
    "keep-sorted-format",
    "keep-sorted-pattern",
    "keep-unique",
    "keep-unique-pattern",
    "line-count",
    "line-pattern",
    "name",
    "same-as",
    "same-as-format",
    "same-as-mode",
    "same-as-pattern",
    "severity",
    // </block>
];

/// Validates syntax for the given `block` and `file_path`.
fn validate_block_syntax(block: &Block, file_path: &Path) -> anyhow::Result<()> {
    for attr in block.attributes.keys() {
        if !RECOGNIZED_ATTRIBUTES.contains(&attr.as_str()) {
            bail!(
                "Block {}:{} at line {}, column {} contains unrecognized attribute `{}`",
                file_path.display(),
                block.name_display(),
                block.start_tag_position_range.start().line,
                block.start_tag_position_range.start().character,
                attr,
            );
        }
    }
    // Validate the `severity` attribute value.
    block.severity().map(|_| ()).context(format!(
        "Block {}:{} at line {}, column {} contains unrecognized severity value",
        file_path.display(),
        block.name_display(),
        block.start_tag_position_range.start().line,
        block.start_tag_position_range.start().character,
    ))
}

/// Rejects a block whose `name` was already used earlier in the same file, recording it in
/// `names_seen` otherwise. A block with no `name` is not a reference target and is ignored.
///
/// Two blocks sharing a name make every `affects`/`same-as` reference to it ambiguous, silently
/// binding to whichever block the parser happens to reach first.
fn reject_duplicate_name(
    block: &Block,
    file_path: &Path,
    names_seen: &mut HashMap<String, Position>,
) -> anyhow::Result<()> {
    let Some(name) = block.name() else {
        return Ok(());
    };
    let position = block.start_tag_position_range.start();
    match names_seen.entry(name.to_string()) {
        Entry::Occupied(entry) => {
            bail!(
                "Block {}:{} at line {}, column {} duplicates the name of the block at line {}, column {}",
                file_path.display(),
                name,
                position.line,
                position.character,
                entry.get().line,
                entry.get().character,
            )
        }
        Entry::Vacant(entry) => {
            entry.insert(position.clone());
            Ok(())
        }
    }
}

fn parser_for_file_path<'p>(
    file_path: &Path,
    parsers: &'p LanguageParsers,
    extra_file_extensions: &HashMap<OsString, OsString>,
) -> Option<&'p LanguageParser> {
    let file_name = file_path.file_name()?.to_str()?;

    for (i, _) in file_name.match_indices('.').rev() {
        let extension = &file_name[i + 1..];
        let ext_os = OsString::from(extension);

        if let Some(parser) = try_parser_for_extension(&ext_os, parsers, extra_file_extensions) {
            return Some(parser);
        }
    }

    try_parser_for_extension(&OsString::from(file_name), parsers, extra_file_extensions)
}

fn try_parser_for_extension<'p>(
    extension: &OsString,
    parsers: &'p LanguageParsers,
    extra_file_extensions: &HashMap<OsString, OsString>,
) -> Option<&'p LanguageParser> {
    let ext = if let Some(ext) = extra_file_extensions.get(extension) {
        ext
    } else {
        extension
    };
    parsers.get(ext)
}

#[cfg(test)]
mod block_severity_from_str_tests {
    use crate::Position;
    use crate::blocks::{Block, BlockSeverity};
    use std::collections::HashMap;

    /// Builds a contentless block carrying only a `severity` attribute to test how that attribute
    /// is parsed.
    pub(crate) fn new_empty_block_with_severity(severity: &str) -> Block {
        Block::new(
            HashMap::from([("severity".into(), severity.into())]),
            Position::new(0, 0)..=Position::new(0, 0),
            0..0,
            Position::new(0, 0)..Position::new(0, 0),
        )
    }

    #[test]
    fn block_with_valid_severity_attribute_returns_correct_severity() {
        let block = new_empty_block_with_severity("warning");

        assert_eq!(block.severity().unwrap(), BlockSeverity::Warning);
    }

    #[test]
    fn block_with_mixed_case_severity_attribute_returns_correct_severity() {
        let block = new_empty_block_with_severity("InFo");

        assert_eq!(block.severity().unwrap(), BlockSeverity::Info);
    }

    #[test]
    fn block_without_severity_attribute_returns_error_severity() {
        let block = Block::new(
            HashMap::new(),
            Position::new(0, 0)..=Position::new(0, 0),
            0..0,
            Position::new(0, 0)..Position::new(0, 0),
        );

        assert_eq!(block.severity().unwrap(), BlockSeverity::Error);
    }

    #[test]
    fn block_with_invalid_severity_attribute_returns_error() {
        let block = new_empty_block_with_severity("warn");

        assert!(block.severity().is_err());
    }
}

#[cfg(test)]
mod parse_blocks_tests {
    use crate::blocks::*;
    use crate::fs::test_utils::{FakeFileSystem, FakePathChecker};
    use crate::language_parsers::language_parsers;
    use crate::test_utils::{self};
    use std::collections::HashSet;

    /// Creates a whole line change (either added or deleted line).
    fn line_change(line: usize) -> LineChange {
        LineChange { line, ranges: None }
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

        let parsed = parse_blocks(
            &HashMap::new(),
            ScanMode::All,
            &file_system,
            &FakePathChecker::allow_all(),
            &parsers,
            &HashMap::new(),
        )?;

        // The file without blocks is not kept, but it was parsed, so it counts as scanned. The
        // file with an unknown extension counts as skipped.
        assert_eq!(parsed.blocks.len(), 1);
        assert_eq!(parsed.stats.files_scanned, 2);
        assert_eq!(parsed.stats.files_skipped, 1);
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
        let line_changes = HashMap::from([
            (
                RepoPath::from_reference("a.rs")?,
                vec![
                    line_change(1), // No blocks on this line.
                    LineChange {
                        // "first" block.
                        line: 2,
                        ranges: Some(vec![
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
                        ranges: Some(vec![
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
                        ranges: Some(vec![test_utils::substr_range(
                            content_a.lines().nth(3).unwrap(),
                            " third block ",
                        )]), // Only the content is modified.
                    },
                    LineChange {
                        // "fourth" block.
                        line: 5,
                        ranges: Some(vec![test_utils::substr_range(
                            content_a.lines().nth(4).unwrap(),
                            " fourth block // </block>",
                        )]), // The content and end tag are modified.
                    },
                    LineChange {
                        // "fifth" block.
                        line: 6,
                        ranges: Some(vec![test_utils::substr_range(
                            content_a.lines().nth(5).unwrap(),
                            " </block>",
                        )]), // Only the end tag is modified.
                    },
                    LineChange {
                        // "sixth" block.
                        line: 8,
                        ranges: Some(vec![test_utils::substr_range(
                            content_a.lines().nth(7).unwrap(),
                            "name=\"sixth\"",
                        )]), // Only the start tag is modified.
                    },
                    LineChange {
                        // "seventh" block.
                        line: 11,
                        ranges: Some(vec![test_utils::substr_range(
                            content_a.lines().nth(10).unwrap(),
                            "keep-sorted=\"asc\"> */",
                        )]), // Only the start tag is modified.
                    },
                    LineChange {
                        // "eighth" block.
                        line: 14,
                        ranges: Some(vec![test_utils::substr_range(
                            content_a.lines().nth(13).unwrap(),
                            " block eight",
                        )]), // Only the content on the same line as start tag is modified.
                    },
                    LineChange {
                        // "ninth" block.
                        line: 17,
                        ranges: Some(vec![test_utils::substr_range(
                            content_a.lines().nth(16).unwrap(),
                            "block nine",
                        )]), // Only the content on a line between start and end tags is modified.
                    },
                    LineChange {
                        // "tenth" block.
                        line: 20,
                        ranges: Some(vec![test_utils::substr_range(
                            content_a.lines().nth(19).unwrap(),
                            "block ten ",
                        )]), // Only the content on the same line as end tag is modified.
                    },
                    LineChange {
                        // "eleventh" block.
                        line: 22,
                        ranges: Some(vec![test_utils::substr_range(
                            content_a.lines().nth(21).unwrap(),
                            " </block>",
                        )]), // End tag is modified.
                    },
                    LineChange {
                        // "twelfth" block.
                        line: 25,
                        ranges: Some(vec![test_utils::substr_range(
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
                    ranges: Some(vec![test_utils::substr_range(
                        content_b.lines().next().unwrap(),
                        "let foo = \"bar\"; ",
                    )]), // Block's content is modified in a single line file.
                }],
            ),
        ]);
        let parsers = language_parsers()?;

        let blocks_by_file = parse_blocks(
            &line_changes,
            ScanMode::OnlyChanged,
            &file_system,
            &FakePathChecker::allow_all(),
            &parsers,
            &HashMap::new(),
        )?
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

        let line_changes = HashMap::from([
            (
                RepoPath::from_reference("a.rs")?,
                vec![LineChange {
                    line: 3, // Content line of the first block.
                    ranges: None,
                }],
            ),
            (
                RepoPath::from_reference("b.rs")?,
                vec![LineChange {
                    line: 3, // Content line of the first block.
                    ranges: None,
                }],
            ),
        ]);
        let blocks_by_file = parse_blocks(
            &line_changes,
            ScanMode::All,
            &file_system,
            &FakePathChecker::allow_all(),
            &parsers,
            &HashMap::new(),
        )?
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

        let blocks_by_file = parse_blocks(
            &HashMap::new(),
            ScanMode::All,
            &file_system,
            &FakePathChecker::allow_all(),
            &parsers,
            &HashMap::new(),
        )?
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

        let blocks_by_file = parse_blocks(
            &HashMap::new(),
            ScanMode::All,
            &file_system,
            &FakePathChecker::allow_all(),
            &parsers,
            &HashMap::new(),
        )?
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
        let parsers = language_parsers()?;

        let blocks_by_file = parse_blocks(
            &HashMap::new(),
            ScanMode::All,
            &file_system,
            &FakePathChecker::allow_all(),
            &parsers,
            &HashMap::from([("rust".into(), "rs".into())]),
        )?
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

        let blocks = parse_blocks(
            &HashMap::new(),
            ScanMode::All,
            &FakeFileSystem::new(files),
            &FakePathChecker::allow_all(),
            &HashMap::new(),
            &HashMap::new(),
        )?
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
        let path_checker =
            FakePathChecker::with_ignored_paths(HashSet::from(["ignored.rs".to_string()]));

        let blocks = parse_blocks(
            &HashMap::new(),
            ScanMode::All,
            &file_system,
            &path_checker,
            &language_parsers()?,
            &HashMap::new(),
        )?
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
        let line_changes = HashMap::from([
            (
                RepoPath::from_reference("src/allowed.rs")?,
                vec![line_change(2)],
            ),
            (
                RepoPath::from_reference("vendor/denied.rs")?,
                vec![line_change(2)],
            ),
        ]);

        let blocks = parse_blocks(
            &line_changes,
            ScanMode::OnlyChanged,
            &file_system,
            &FakePathChecker::allow_only("src/**"),
            &language_parsers()?,
            &HashMap::new(),
        )?
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
        let line_changes = HashMap::from([
            (
                RepoPath::from_reference("present.rs")?,
                vec![line_change(1)],
            ),
            (RepoPath::from_reference("absent.rs")?, vec![line_change(1)]),
        ]);

        let parsed = parse_blocks(
            &line_changes,
            ScanMode::All,
            &file_system,
            &FakePathChecker::allow_all(),
            &language_parsers()?,
            &HashMap::new(),
        )?;

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
        let line_changes = HashMap::from([(
            RepoPath::from_reference("rules.py")?,
            vec![LineChange {
                line: 1,
                ranges: None,
            }],
        )]);
        let error = parse_blocks(
            &line_changes,
            ScanMode::OnlyChanged,
            &FakeFileSystem::new(HashMap::from([("src/rules.py".to_string(), String::new())])),
            &FakePathChecker::allow_all(),
            &language_parsers()?,
            &HashMap::new(),
        )
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
        let line_changes =
            HashMap::from([(RepoPath::from_reference("rules.py")?, vec![line_change(1)])]);
        let error = parse_blocks(
            &line_changes,
            ScanMode::All,
            &FakeFileSystem::new(HashMap::from([("src/rules.py".to_string(), String::new())])),
            &FakePathChecker::allow_all(),
            &language_parsers()?,
            &HashMap::new(),
        )
        .unwrap_err();
        let message = format!("{error:#}");
        assert!(
            message.contains("does not exist in the repository root"),
            "unexpected error: {message}"
        );
        Ok(())
    }

    #[test]
    fn diff_with_only_files_outside_the_globs_is_not_a_broken_diff() -> anyhow::Result<()> {
        // Globs narrow a run, so a diff with no path inside them is what the caller asked for.
        let line_changes = HashMap::from([(
            RepoPath::from_reference("docs/gone.py")?,
            vec![line_change(1)],
        )]);
        let blocks = parse_blocks(
            &line_changes,
            ScanMode::All,
            &FakeFileSystem::new(HashMap::from([(
                "src/present.py".to_string(),
                "# <block name=\"present\">\nx = 1\n# </block>\n".to_string(),
            )])),
            &FakePathChecker::allow_only("src/**"),
            &language_parsers()?,
            &HashMap::new(),
        )?
        .blocks;

        assert!(blocks.contains_key(&RepoPath::from_reference("src/present.py")?));
        Ok(())
    }

    #[test]
    fn diff_with_a_missing_ignored_file_is_skipped() -> anyhow::Result<()> {
        // An ignored path contributes no blocks whether or not it exists, so it must not be able
        // to fail the run.
        let line_changes = HashMap::from([(
            RepoPath::from_reference("vendor/gone.py")?,
            vec![LineChange {
                line: 1,
                ranges: None,
            }],
        )]);
        let blocks = parse_blocks(
            &line_changes,
            ScanMode::OnlyChanged,
            &FakeFileSystem::new(HashMap::new()),
            &FakePathChecker::with_ignored_paths(HashSet::from(["vendor/gone.py".to_string()])),
            &language_parsers()?,
            &HashMap::new(),
        )?
        .blocks;
        assert!(blocks.is_empty());
        Ok(())
    }

    #[test]
    fn diff_with_a_missing_unparseable_file_is_skipped() -> anyhow::Result<()> {
        // Likewise for a file whose extension maps to no language: a diff routinely carries binary
        // assets and lockfiles that are absent from a partial checkout.
        let line_changes = HashMap::from([(
            RepoPath::from_reference("assets/logo.png")?,
            vec![LineChange {
                line: 1,
                ranges: None,
            }],
        )]);
        let blocks = parse_blocks(
            &line_changes,
            ScanMode::OnlyChanged,
            &FakeFileSystem::new(HashMap::new()),
            &FakePathChecker::allow_all(),
            &language_parsers()?,
            &HashMap::new(),
        )?
        .blocks;
        assert!(blocks.is_empty());
        Ok(())
    }

    #[test]
    fn empty_input_returns_empty_result() -> anyhow::Result<()> {
        let line_changes = HashMap::default();
        let blocks = parse_blocks(
            &line_changes,
            ScanMode::All,
            &FakeFileSystem::new(HashMap::default()),
            &FakePathChecker::allow_all(),
            &HashMap::new(),
            &HashMap::new(),
        )?
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
            &[],
            every_block,
            &parsers,
            &HashMap::new(),
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
            &[],
            every_block,
            &parsers,
            &HashMap::new(),
        );

        assert!(file_blocks.is_err());
        assert_eq!(
            file_blocks.unwrap_err().source().unwrap().to_string(),
            "Block a.py:x at line 1, column 3 contains unrecognized attribute `unknown-attr`"
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
            &[],
            every_block,
            &parsers,
            &HashMap::new(),
        );

        assert!(file_blocks.is_err());
        assert_eq!(
            file_blocks.unwrap_err().source().unwrap().to_string(),
            "Block a.py:(unnamed) at line 1, column 3 contains unrecognized severity value"
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
            &[],
            every_block,
            &parsers,
            &HashMap::new(),
        );

        assert!(file_blocks.is_err());
        assert_eq!(
            file_blocks.unwrap_err().source().unwrap().to_string(),
            "Block a.py:x at line 4, column 3 duplicates the name of the block at line 1, column 3"
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

        let blocks = parse_blocks(
            &HashMap::new(),
            ScanMode::All,
            &file_system,
            &FakePathChecker::allow_all(),
            &parsers,
            &HashMap::new(),
        )?
        .blocks;

        assert_eq!(blocks.len(), 2);
        Ok(())
    }
}

#[cfg(test)]
mod supported_languages_tests {
    use std::collections::HashMap;

    use crate::blocks::*;
    use crate::fs::test_utils::{FakeFileSystem, FakePathChecker};
    use crate::language_parsers::language_parsers;

    // <block name="supported-extensions">
    #[test]
    fn all_language_extensions_are_supported() -> anyhow::Result<()> {
        let parsers = language_parsers()?;
        let files = HashMap::from([
            (
                "BUILD".to_string(),
                "# <block>\ncc_library(name = \"foo\")\n# </block>".to_string(),
            ),
            (
                "MODULE.bazel".to_string(),
                "# <block>\nmodule(name = \"m\")\n# </block>".to_string(),
            ),
            (
                "WORKSPACE".to_string(),
                "# <block>\nworkspace(name = \"w\")\n# </block>".to_string(),
            ),
            (
                "WORKSPACE.bzlmod".to_string(),
                "# <block>\n# migration stub\n# </block>".to_string(),
            ),
            (
                "CMakeLists.txt".to_string(),
                "# <block>\nadd_library(foo foo.c)\n# </block>".to_string(),
            ),
            (
                "cmake.cmake".to_string(),
                "#[[ <block> ]]\nset(X 1)\n# </block>".to_string(),
            ),
            (
                "bash.bash".to_string(),
                "# <block>\necho \"hello\"\n# </block>".to_string(),
            ),
            (
                "bzl.bzl".to_string(),
                "# <block>\ndef my_macro():\n    pass\n# </block>".to_string(),
            ),
            (
                "c.c".to_string(),
                "/* <block> */\nint main() { return 0; }\n/* </block> */".to_string(),
            ),
            (
                "cc.cpp".to_string(),
                "// <block>\nint main() { return 0; }\n// </block>".to_string(),
            ),
            (
                "cpp.cpp".to_string(),
                "// <block>\nint main() { return 0; }\n// </block>".to_string(),
            ),
            (
                "cs.cs".to_string(),
                "// <block>\nclass Program { }\n// </block>".to_string(),
            ),
            (
                "css.css".to_string(),
                "/* <block> */\nbody { margin: 0; }\n/* </block> */".to_string(),
            ),
            (
                "Containerfile".to_string(),
                "# <block>\nFROM fedora\n# </block>".to_string(),
            ),
            (
                "containerfile".to_string(),
                "# <block>\nFROM centos\n# </block>".to_string(),
            ),
            (
                "Dockerfile".to_string(),
                "# <block>\nFROM alpine\n# </block>".to_string(),
            ),
            (
                "app.dockerfile".to_string(),
                "# <block>\nFROM debian\n# </block>".to_string(),
            ),
            (
                "dart.dart".to_string(),
                "// <block>\nvoid main() {}\n// </block>".to_string(),
            ),
            (
                "ex.ex".to_string(),
                "# <block>\ndefmodule Foo do\nend\n# </block>".to_string(),
            ),
            (
                "exs.exs".to_string(),
                "# <block>\nIO.puts(:hello)\n# </block>".to_string(),
            ),
            (
                "go.go".to_string(),
                "// <block>\nfunc main() {}\n// </block>".to_string(),
            ),
            (
                "go.mod".to_string(),
                "// <block>\nmodule example.com/m\n// </block>".to_string(),
            ),
            (
                "go.sum".to_string(),
                "// <block>\nexample.com/dep v1.0.0 h1:abc\n// </block>".to_string(),
            ),
            (
                "go.work".to_string(),
                "// <block>\nuse ./mod\n// </block>".to_string(),
            ),
            (
                "gql.gql".to_string(),
                "# <block>\ntype Mutation {\n  noop: Boolean\n}\n# </block>".to_string(),
            ),
            (
                "gradle.gradle".to_string(),
                "// <block>\nversion = '1.0'\n// </block>".to_string(),
            ),
            (
                "graphql.graphql".to_string(),
                "# <block>\ntype Query {\n  hello: String\n}\n# </block>".to_string(),
            ),
            (
                "groovy.groovy".to_string(),
                "// <block>\ndef x = 1\n// </block>".to_string(),
            ),
            (
                "h.h".to_string(),
                "// <block>\nvoid foo();\n// </block>".to_string(),
            ),
            (
                "hcl.hcl".to_string(),
                "// <block>\nregion = \"eu-west-1\"\n// </block>".to_string(),
            ),
            (
                "htm.htm".to_string(),
                "<!-- <block> -->\n<div>Content</div>\n<!-- </block> -->".to_string(),
            ),
            (
                "html.html".to_string(),
                "<!-- <block> -->\n<p>Hello</p>\n<!-- </block> -->".to_string(),
            ),
            (
                "java.java".to_string(),
                "// <block>\nclass App {}\n// </block>".to_string(),
            ),
            (
                "Jenkinsfile".to_string(),
                "// <block>\npipeline { }\n// </block>".to_string(),
            ),
            (
                "jenkinsfile".to_string(),
                "// <block>\nnode { }\n// </block>".to_string(),
            ),
            (
                "js.js".to_string(),
                "// <block>\nconst x = 1;\n// </block>".to_string(),
            ),
            (
                "jsx.jsx".to_string(),
                "// <block>\nconst Comp = () => <div/>;\n// </block>".to_string(),
            ),
            (
                "kt.kt".to_string(),
                "// <block>\nfun main() {}\n// </block>".to_string(),
            ),
            (
                "kts.kts".to_string(),
                "// <block>\nplugins { }\n// </block>".to_string(),
            ),
            (
                "lua.lua".to_string(),
                "-- <block>\nlocal x = 1\n-- </block>".to_string(),
            ),
            (
                "makefile".to_string(),
                "# <block>\nall:\n\t@echo \"hello\"\n# </block>".to_string(),
            ),
            (
                "Makefile".to_string(),
                "# <block>\nall:\n\t@echo \"hello\"\n# </block>".to_string(),
            ),
            (
                "markdown.markdown".to_string(),
                "<div>\n<!-- <block> -->\n# Title\n<!-- </block> -->\n</div>".to_string(),
            ),
            (
                "md.md".to_string(),
                "<div>\n<!-- <block> -->\n## Heading\n<!-- </block> -->\n</div>".to_string(),
            ),
            (
                "mk.mk".to_string(),
                "# <block>\nall:\n\t@echo \"hello\"\n# </block>".to_string(),
            ),
            (
                "nix.nix".to_string(),
                "# <block>\n{ pkgs = null; }\n# </block>".to_string(),
            ),
            (
                "php.php".to_string(),
                "<?php\n# <block>\necho 'hello';\n# </block>\n?>".to_string(),
            ),
            (
                "phtml.phtml".to_string(),
                "<?php\n# <block>\necho 'world';\n# </block>\n?>".to_string(),
            ),
            (
                "proto.proto".to_string(),
                "// <block>\nsyntax = \"proto3\";\n// </block>".to_string(),
            ),
            (
                "py.py".to_string(),
                "# <block>\ndef main():\n    pass\n# </block>".to_string(),
            ),
            (
                "pyi.pyi".to_string(),
                "# <block>\ndef foo() -> None: pass\n# </block>".to_string(),
            ),
            (
                "rb.rb".to_string(),
                "# <block>\ndef hello\n  puts 'world'\nend\n# </block>".to_string(),
            ),
            (
                "rs.rs".to_string(),
                r#"/* <block> */fn a() {}/* </block> */"#.to_string(),
            ),
            (
                "sbt.sbt".to_string(),
                "// <block>\nname := \"app\"\n// </block>".to_string(),
            ),
            (
                "scala.scala".to_string(),
                "// <block>\nval x = 1\n// </block>".to_string(),
            ),
            (
                "sh.sh".to_string(),
                "# <block>\necho \"hello\"\n# </block>".to_string(),
            ),
            (
                "sql.sql".to_string(),
                "-- <block>\nSELECT * FROM users;\n-- </block>".to_string(),
            ),
            (
                "star.star".to_string(),
                "# <block>\nx = 42\n# </block>".to_string(),
            ),
            (
                "swift.swift".to_string(),
                "// <block>\nfunc main() {}\n// </block>".to_string(),
            ),
            (
                "tf.tf".to_string(),
                "# <block>\nregion = \"us-east-1\"\n# </block>".to_string(),
            ),
            (
                "tfvars.tfvars".to_string(),
                "# <block>\nregion = \"us-west-2\"\n# </block>".to_string(),
            ),
            (
                "toml.toml".to_string(),
                "# <block>\nname = \"test\"\n# </block>".to_string(),
            ),
            (
                "ts.ts".to_string(),
                "// <block>\nconst x: number = 1;\n// </block>".to_string(),
            ),
            (
                "tsx.tsx".to_string(),
                "// <block>\nconst C = () => <div/>;\n// </block>".to_string(),
            ),
            (
                "typescript.d.ts".to_string(),
                "// <block>\ndeclare const x: number;\n// </block>".to_string(),
            ),
            (
                "xml.xml".to_string(),
                "<!-- <block> -->\n<root/>\n<!-- </block> -->".to_string(),
            ),
            (
                "yaml.yaml".to_string(),
                "# <block>\nkey: value\n# </block>".to_string(),
            ),
            (
                "yml.yml".to_string(),
                "# <block>\nname: test\n# </block>".to_string(),
            ),
        ]);
        let file_system = FakeFileSystem::new(files.clone());

        let blocks_by_file = parse_blocks(
            &HashMap::new(),
            ScanMode::All,
            &file_system,
            &FakePathChecker::allow_all(),
            &parsers,
            &HashMap::new(),
        )?
        .blocks;

        for file_name in files.keys() {
            assert!(
                !blocks_by_file
                    .get(&RepoPath::from_reference(file_name)?)
                    .unwrap_or_else(|| panic!("No blocks found for file {file_name}"))
                    .blocks_with_context
                    .is_empty(),
                "File {file_name} should have blocks",
            );
        }
        Ok(())
    }
    // </block>
}
