use crate::Position;
use crate::diff_parser::{self, LineChange, LineChangeKind};
use crate::fs::{FileSystem, PathChecker};
use crate::language_parsers::{LanguageParsers, SharedLanguageParser};
use crate::repo_path::RepoPath;
use crate::virtual_blocks::{ConfigEntry, VirtualBlock};
use anyhow::{Context, anyhow, bail};
use serde_repr::Serialize_repr;
use std::cmp::Ordering;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::ops::Range;
use std::path::Path;
use std::str::FromStr;
use strum_macros::EnumString;

const UNNAMED_BLOCK_LABEL: &str = "(unnamed)";

/// A block: a part of a file that its attributes set rules for.
///
/// A pair of tags in the file's comments declares a block. So does a `[[block]]` entry of the
/// config file, around a symbol of the file. That is a virtual block.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Block {
    /// Optional attributes in the `block` tag, or in the `[[block]]` entry of a virtual block.
    /// Their names are what selects the validators that will check this block (`affects`,
    /// `keep-sorted`, …).
    pub(crate) attributes: HashMap<String, String>,
    /// Block's start tag position range, half-open: it starts at the `<` symbol and ends one column
    /// past the `>` symbol. A virtual block has no tag, so it has the first line of the symbol's
    /// definition.
    pub(crate) start_tag_position_range: Range<Position>,
    /// The block's content.
    pub(crate) content: Content,
    /// Where the block's attributes are written.
    pub(crate) declaration: Declaration,
}

/// The content of a [`Block`]: its text, and where the block's file writes it.
#[derive(Debug, PartialEq, Eq, Clone)]
pub(crate) struct Content {
    /// The content's text.
    text: ContentText,
    /// Where the content is written in the block's file. For a block with tags, it runs from the
    /// end of the comment with the start tag to the start of the comment with the end tag. For a
    /// virtual block, it is where the symbol's value or definition is written.
    positions: Range<Position>,
}

/// The text of a block's [`Content`].
#[derive(Debug, PartialEq, Eq, Clone)]
enum ContentText {
    /// The source text in this byte range: the lines between the block's tags, or the definition
    /// of the object, list or table that a virtual block wraps.
    Source(Range<usize>),
    /// The decoded value of the scalar that a virtual block wraps. Its quotes and escapes are
    /// gone, so it is not a part of the source text.
    Decoded(String),
}

/// Where the attributes of a [`Block`] are written.
#[derive(Debug, PartialEq, Eq, Clone)]
pub(crate) enum Declaration {
    /// In the block's start tag, at its `start_tag_position_range`.
    Tags,
    /// In a `[[block]]` entry of the config file.
    ConfigEntry(ConfigEntry),
}

/// What tells a block apart from the other blocks of its file.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct BlockKey {
    /// Where the block starts. Two blocks with tags never start at the same place.
    start: Position,
    /// The config line of a virtual block, or `None` for a block with tags. Two virtual blocks
    /// can start at the same place, such as TOML's `/a` and `/a/b` in `a.b = 1`, but they never
    /// share a config line.
    config_line: Option<usize>,
}

#[cfg(test)]
impl BlockKey {
    /// The line the block starts on.
    pub(crate) fn start_line(&self) -> usize {
        self.start.line
    }
}

impl PartialOrd for Block {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Block {
    fn cmp(&self, other: &Self) -> Ordering {
        self.start_tag_position_range
            .start
            .cmp(&other.start_tag_position_range.start)
    }
}

impl Content {
    /// Content whose text is the bytes in `byte_range` of the block's file. `positions` are where
    /// those bytes are.
    pub(crate) fn source(byte_range: Range<usize>, positions: Range<Position>) -> Self {
        Self {
            text: ContentText::Source(byte_range),
            positions,
        }
    }

    /// Content whose text is `text`, the decoded value of a scalar. `positions` are where the
    /// scalar is written in the block's file.
    pub(crate) fn decoded(text: String, positions: Range<Position>) -> Self {
        Self {
            text: ContentText::Decoded(text),
            positions,
        }
    }

    /// Returns the content's text. `source` must be the text of the block's file.
    pub(crate) fn text<'a>(&'a self, source: &'a str) -> &'a str {
        match &self.text {
            ContentText::Source(range) => &source[range.clone()],
            ContentText::Decoded(text) => text,
        }
    }

    /// Maps a position inside the content onto the position in the block's file.
    ///
    /// `line_idx` is the 0-based index of a line of [`Self::text`],
    /// `column_offset` is the 0-based offset within that line.
    pub(crate) fn position(&self, line_idx: usize, column_offset: usize) -> Position {
        match self.text {
            ContentText::Source(_) => {
                let line_start_column = if line_idx == 0 {
                    self.positions.start.character
                } else {
                    1
                };
                Position::new(
                    self.positions.start.line + line_idx,
                    line_start_column + column_offset,
                )
            }
            // Quotes, escapes and joined lines make a decoded value differ from the source text,
            // so a position inside it has no exact place in the source. The start of the value is
            // the closest one.
            ContentText::Decoded(_) => self.positions.start.clone(),
        }
    }

    /// Whether any of the **ordered** `line_changes` touch where the content is written.
    fn intersects_any(&self, line_changes: &[LineChange]) -> bool {
        diff_parser::changes_on_lines(
            line_changes,
            self.positions.start.line,
            self.positions.end.line,
        )
        .any(|line_change| self.intersects(line_change))
    }

    /// Whether `line_change`, which has to fall on one of the lines the content spans, touches
    /// where the content is written.
    fn intersects(&self, line_change: &LineChange) -> bool {
        match &line_change.kind {
            LineChangeKind::Deleted => {
                let point = diff_parser::deletion_point(line_change.line);
                // The end is included even though the range excludes it: a gap sitting exactly at
                // the range's exclusive end is where the range's last character used to be. A
                // whole-line deletion never lands there, since its gap is always at column 1, so
                // this bound only bites if deletion points ever get finer-grained than a line.
                self.positions.start <= point && point <= self.positions.end
            }
            // An insertion moves the line breaks around the line, not just the characters on it,
            // so there is nothing to compare column by column. Every position on the line counts
            // as changed.
            LineChangeKind::Added => true,
            LineChangeKind::Modified(ranges) => {
                let start_col = if line_change.line == self.positions.start.line {
                    self.positions.start.character
                } else {
                    1
                };
                let end_col = if line_change.line < self.positions.end.line {
                    usize::MAX
                } else {
                    self.positions.end.character
                };

                diff_parser::intersects_columns(ranges, start_col, end_col)
            }
        }
    }
}

impl Block {
    /// Returns the optional value of the `name` attribute for this block.
    pub(crate) fn name(&self) -> Option<&str> {
        self.attributes.get("name").map(String::as_str)
    }

    /// Returns the block's name if present, otherwise a human-friendly placeholder label.
    pub(crate) fn name_display(&self) -> &str {
        self.name().unwrap_or(UNNAMED_BLOCK_LABEL)
    }

    /// Where the block's attributes are written, as messages show it: `line 3` for a block with
    /// tags, or `line 7 of "blockwatch.toml"` for a virtual block.
    pub(crate) fn declared_at(&self) -> String {
        match &self.declaration {
            // A message shows the block's file before this, so the line is enough.
            Declaration::Tags => format!("line {}", self.start_tag_position_range.start.line),
            Declaration::ConfigEntry(entry) => entry.to_string(),
        }
    }

    /// What tells the block apart from the other blocks of its file.
    pub(crate) fn key(&self) -> BlockKey {
        BlockKey {
            start: self.start_tag_position_range.start.clone(),
            config_line: match &self.declaration {
                Declaration::Tags => None,
                Declaration::ConfigEntry(entry) => Some(entry.line),
            },
        }
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
// <block name="block-severity" affects="docs/validators/README.md:severity-levels">
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
// </block>

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
    /// The listing is already deterministic without a sorting pass: [`parse_file`] keeps the
    /// blocks in source order.
    pub(crate) fn to_serializable_report(&self) -> Vec<serde_json::Value> {
        self.blocks_with_context
            .iter()
            .map(|block| {
                // <block affects="docs/cli.md:list-output-example">
                let mut listing = serde_json::json!({
                    "name": block.block.name_display(),
                    "line": block.block.start_tag_position_range.start.line,
                    "column": block.block.start_tag_position_range.start.character,
                    "is_content_modified": block.is_content_modified,
                    "attributes": block.block.attributes,
                });
                match &block.block.declaration {
                    Declaration::Tags => {}
                    Declaration::ConfigEntry(entry) => listing["config_line"] = entry.line.into(),
                }
                // </block>
                listing
            })
            .collect()
    }
}

/// Represents a block with its corresponding validation context.
#[derive(Debug, Clone)]
pub struct BlockWithContext {
    /// The block itself, as parsed from the source comment or the config file.
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
    /// The file of a virtual block is read too when the diff modified the block's entry in the
    /// config file.
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
/// - `virtual_blocks` are the blocks the config file declares. Each joins the blocks of the file
///   it wraps, when that file is read.
///
/// In either mode a file is read only if it passes the allow globs and is not ignored.
///
/// Fails if `line_changes_by_file` is invalid, e.g. it refers to files that do not exist. Also
/// fails if a virtual block's target does not resolve in a file that is read, or if its file
/// passes the filters but does not exist. In [`ScanMode::OnlyChanged`], only a virtual block
/// whose entry the diff modified fails for a missing file.
pub fn parse_blocks(
    line_changes_by_file: &HashMap<RepoPath, Vec<LineChange>>,
    scan_mode: ScanMode,
    file_system: &impl FileSystem,
    path_checker: &impl PathChecker,
    parsers: &LanguageParsers,
    extra_file_extensions: &HashMap<OsString, OsString>,
    virtual_blocks: &[VirtualBlock],
) -> anyhow::Result<ParsedBlocks> {
    ensure_diff_has_valid_paths(
        line_changes_by_file,
        file_system,
        path_checker,
        parsers,
        extra_file_extensions,
    )?;
    let mut virtual_blocks_by_file: HashMap<&RepoPath, Vec<&VirtualBlock>> = HashMap::new();
    for virtual_block in virtual_blocks {
        virtual_blocks_by_file
            .entry(&virtual_block.file)
            .or_default()
            .push(virtual_block);
    }
    match scan_mode {
        ScanMode::All => {
            ensure_target_files_exist(virtual_blocks.iter(), file_system, path_checker)?;
            parse_all_files(
                line_changes_by_file,
                file_system,
                path_checker,
                parsers,
                extra_file_extensions,
                &virtual_blocks_by_file,
            )
        }
        ScanMode::OnlyChanged => {
            ensure_target_files_exist(
                virtual_blocks
                    .iter()
                    .filter(|virtual_block| virtual_block.is_entry_modified),
                file_system,
                path_checker,
            )?;
            parse_changed_files(
                line_changes_by_file,
                file_system,
                path_checker,
                parsers,
                extra_file_extensions,
                &virtual_blocks_by_file,
            )
        }
    }
}

/// Rejects a virtual block whose file is allowed by `path_checker` but does not exist.
fn ensure_target_files_exist<'v>(
    virtual_blocks: impl Iterator<Item = &'v VirtualBlock>,
    file_system: &impl FileSystem,
    path_checker: &impl PathChecker,
) -> anyhow::Result<()> {
    for virtual_block in virtual_blocks {
        let file = &virtual_block.file;
        if path_checker.should_allow(file)
            && !path_checker.should_ignore(file)
            && !file_system.exists(file.as_path())
        {
            return Err(virtual_block.unresolved(anyhow!("file does not exist")));
        }
    }
    Ok(())
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
    virtual_blocks_by_file: &HashMap<&RepoPath, Vec<&VirtualBlock>>,
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
            virtual_blocks_by_file
                .get(&file_path)
                .map_or(&[][..], Vec::as_slice),
        )?;
        record_parsed_file(&mut parsed, file_path, file_blocks);
    }
    Ok(parsed)
}

/// Parses only the files in the diff, and the files of the virtual blocks whose entry it modified.
/// Keeps the blocks whose start tag or content it modified.
fn parse_changed_files(
    line_changes_by_file: &HashMap<RepoPath, Vec<LineChange>>,
    file_system: &impl FileSystem,
    path_checker: &impl PathChecker,
    parsers: &LanguageParsers,
    extra_file_extensions: &HashMap<OsString, OsString>,
    virtual_blocks_by_file: &HashMap<&RepoPath, Vec<&VirtualBlock>>,
) -> anyhow::Result<ParsedBlocks> {
    // A changed entry in the config file changes the rules of its block, as a changed tag does. So
    // the block is checked even when the diff does not touch its file.
    let entry_files = virtual_blocks_by_file
        .iter()
        .filter(|(_, virtual_blocks)| {
            virtual_blocks
                .iter()
                .any(|virtual_block| virtual_block.is_entry_modified)
        })
        .map(|(file_path, _)| *file_path);
    let files: HashSet<&RepoPath> = line_changes_by_file.keys().chain(entry_files).collect();
    let mut parsed = ParsedBlocks::default();
    for file_path in files {
        if !path_checker.should_allow(file_path) || path_checker.should_ignore(file_path) {
            continue;
        }
        let line_changes = line_changes_by_file
            .get(file_path)
            .map_or(&[][..], Vec::as_slice);
        let file_blocks = parse_file(
            file_system,
            file_path.as_path(),
            line_changes,
            modified_blocks,
            parsers,
            extra_file_extensions,
            virtual_blocks_by_file
                .get(file_path)
                .map_or(&[][..], Vec::as_slice),
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
/// `block_predicate` selects. The blocks come in source order, the `virtual_blocks` of the file
/// among its tags. Returns `None` for unsupported file extensions.
///
/// # Errors
/// Returns an error if a tag is malformed or a name is used twice, or if a virtual block's target
/// does not resolve in the file.
pub fn parse_file(
    file_system: &impl FileSystem,
    file_path: &Path,
    line_changes: &[LineChange],
    block_predicate: impl Fn(&BlockWithContext) -> bool,
    parsers: &LanguageParsers,
    extra_file_extensions: &HashMap<OsString, OsString>,
    virtual_blocks: &[&VirtualBlock],
) -> anyhow::Result<Option<FileBlocks>> {
    let parser = match (
        parser_for_file_path(file_path, parsers, extra_file_extensions),
        virtual_blocks,
    ) {
        (Some(parser), _) => parser,
        (None, []) => return Ok(None),
        // A virtual block needs the symbols of its file, and a file without a parser has none.
        (None, [virtual_block, ..]) => {
            return Err(virtual_block.unresolved(anyhow!("file format is unsupported")));
        }
    };
    let source_code = file_system.read_to_string(file_path)?;
    // Tracks where each name in this file is first declared, to reject duplicate blocks.
    let mut names_seen = HashMap::new();
    let mut blocks_with_context = tag_blocks(
        parser,
        file_path,
        &source_code,
        line_changes,
        &block_predicate,
        &mut names_seen,
    )?;
    blocks_with_context.extend(virtual_blocks_in_file(
        virtual_blocks,
        parser,
        file_path,
        &source_code,
        line_changes,
        &block_predicate,
        &mut names_seen,
    )?);
    // The tags come in source order. The virtual blocks join them there.
    blocks_with_context.sort_by(|a, b| a.block.cmp(&b.block));
    Ok(Some(FileBlocks {
        file_content: source_code,
        blocks_with_context,
    }))
}

/// The blocks that tags declare in `source`, the text of `file_path`, with what `line_changes`
/// touched in each. Keeps those `block_predicate` selects, in source order, and records every
/// block's name in `names_seen`.
///
/// # Errors
/// Returns an error if a tag is malformed or not closed, has an unknown attribute or severity, or
/// has a name that `names_seen` already holds. The error shows the file.
fn tag_blocks(
    parser: &SharedLanguageParser,
    file_path: &Path,
    source: &str,
    line_changes: &[LineChange],
    block_predicate: &impl Fn(&BlockWithContext) -> bool,
    names_seen: &mut HashMap<String, String>,
) -> anyhow::Result<Vec<BlockWithContext>> {
    // Blocks are filtered as the parser yields them, so only the ones this run will validate are
    // ever held. The parser's lock lives until the end of the statement, which is as long as the
    // iterator borrowing it does.
    parser
        .lock()
        .expect("no active locks")
        .parse_blocks(source)
        .map(|block| {
            let block = block?;
            validate_block_syntax(&block, file_path)?;
            reject_duplicate_name(&block, file_path, names_seen)?;
            Ok(BlockWithContext {
                is_content_modified: block.content.intersects_any(line_changes),
                is_start_tag_modified: diff_parser::range_intersects_any(
                    &block.start_tag_position_range,
                    line_changes,
                ),
                block,
            })
        })
        .filter(|result| match result {
            Ok(block_with_context) => block_predicate(block_with_context),
            // An error is kept, so that collecting the blocks stops at it.
            Err(_) => true,
        })
        .collect::<anyhow::Result<Vec<_>>>()
        .with_context(|| format!("Failed to parse file {file_path:?}"))
}

/// The blocks that `virtual_blocks` declare in `source`, the text of `file_path`, with what
/// `line_changes` touched in each. Keeps those `block_predicate` selects, in the order of
/// `virtual_blocks`, and records every block's name in `names_seen`.
///
/// # Errors
/// Returns an error if the file has no symbols or a syntax error, if a target does not resolve,
/// or if a block has a name that `names_seen` already holds.
fn virtual_blocks_in_file(
    virtual_blocks: &[&VirtualBlock],
    parser: &SharedLanguageParser,
    file_path: &Path,
    source: &str,
    line_changes: &[LineChange],
    block_predicate: &impl Fn(&BlockWithContext) -> bool,
    names_seen: &mut HashMap<String, String>,
) -> anyhow::Result<Vec<BlockWithContext>> {
    // Without virtual blocks the file's symbols are not needed, and a language that has none
    // must not fail.
    let [first, ..] = virtual_blocks else {
        return Ok(Vec::new());
    };
    // The file's symbols are the same for each of its virtual blocks. So is the error when they
    // can't be found, which then shows the first block.
    let symbols = parser
        .lock()
        .expect("no active locks")
        .parse_symbols(source)
        .map_err(|error| first.unresolved(error))?;
    let mut blocks_with_context = Vec::new();
    for virtual_block in virtual_blocks {
        let block_with_context = virtual_block.resolve(&symbols, source, line_changes)?;
        reject_duplicate_name(&block_with_context.block, file_path, names_seen)?;
        if block_predicate(&block_with_context) {
            blocks_with_context.push(block_with_context);
        }
    }
    Ok(blocks_with_context)
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

/// Checks a block's attributes: each one must be recognized, and `severity` must have a valid
/// value.
///
/// # Errors
/// Returns an error that quotes an attribute or a value that is not valid, such as
/// ``unrecognized attribute `keep-sortd` ``.
pub(crate) fn validate_attributes(attributes: &HashMap<String, String>) -> anyhow::Result<()> {
    if let Some(name) = attributes
        .keys()
        .find(|name| !RECOGNIZED_ATTRIBUTES.contains(&name.as_str()))
    {
        bail!("unrecognized attribute `{name}`");
    }
    if let Some(value) = attributes.get("severity")
        && BlockSeverity::from_str(value).is_err()
    {
        bail!("unrecognized severity value `{value}`");
    }
    Ok(())
}

/// Checks the attributes of `block`, which is in `file_path`, with [`validate_attributes`].
///
/// # Errors
/// Returns an error that shows the block and what is not valid.
fn validate_block_syntax(block: &Block, file_path: &Path) -> anyhow::Result<()> {
    validate_attributes(&block.attributes).map_err(|error| {
        anyhow!(
            "Block {}:{} at {} contains {error}",
            file_path.display(),
            block.name_display(),
            block.declared_at(),
        )
    })
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
                "Block {}:{} at {} duplicates the name of the block at {}",
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

/// Resolves the language parser for `file_path` considering configured extension remappings.
///
/// Returns `None` if the file extension or filename is not supported by any registered parser.
pub(crate) fn parser_for_file_path<'p>(
    file_path: &Path,
    parsers: &'p LanguageParsers,
    extra_file_extensions: &HashMap<OsString, OsString>,
) -> Option<&'p SharedLanguageParser> {
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
) -> Option<&'p SharedLanguageParser> {
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
    use crate::blocks::{Block, BlockSeverity, Content, Declaration};
    use std::collections::HashMap;

    /// Builds a contentless block carrying only a `severity` attribute to test how that attribute
    /// is parsed.
    pub(crate) fn new_empty_block_with_severity(severity: &str) -> Block {
        Block {
            attributes: HashMap::from([("severity".into(), severity.into())]),
            start_tag_position_range: Position::new(0, 0)..Position::new(0, 0),
            content: Content::source(0..0, Position::new(0, 0)..Position::new(0, 0)),
            declaration: Declaration::Tags,
        }
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
        let block = Block {
            attributes: HashMap::new(),
            start_tag_position_range: Position::new(0, 0)..Position::new(0, 0),
            content: Content::source(0..0, Position::new(0, 0)..Position::new(0, 0)),
            declaration: Declaration::Tags,
        };

        assert_eq!(block.severity().unwrap(), BlockSeverity::Error);
    }

    #[test]
    fn block_with_invalid_severity_attribute_returns_error() {
        let block = new_empty_block_with_severity("warn");

        assert!(block.severity().is_err());
    }
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
mod parse_blocks_tests {
    use crate::blocks::*;
    use crate::fs::test_utils::{FakeFileSystem, FakePathChecker};
    use crate::language_parsers::language_parsers;
    use crate::test_utils::{self};
    use std::collections::HashSet;

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

        let parsed = parse_blocks(
            &HashMap::new(),
            ScanMode::All,
            &file_system,
            &FakePathChecker::allow_all(),
            &parsers,
            &HashMap::new(),
            &[],
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

        let blocks_by_file = parse_blocks(
            &line_changes,
            ScanMode::OnlyChanged,
            &file_system,
            &FakePathChecker::allow_all(),
            &parsers,
            &HashMap::new(),
            &[],
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
        let line_changes = HashMap::from([(RepoPath::from_reference("a.rs")?, line_changes)]);

        let mut blocks_by_file = parse_blocks(
            &line_changes,
            ScanMode::All,
            &file_system,
            &FakePathChecker::allow_all(),
            &language_parsers()?,
            &HashMap::new(),
            &[],
        )?
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

        let line_changes = HashMap::from([
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
        let blocks_by_file = parse_blocks(
            &line_changes,
            ScanMode::All,
            &file_system,
            &FakePathChecker::allow_all(),
            &parsers,
            &HashMap::new(),
            &[],
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
            &[],
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
            &[],
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
            &[],
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
            &[],
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
            &[],
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
            (RepoPath::from_reference("src/allowed.rs")?, vec![added(2)]),
            (
                RepoPath::from_reference("vendor/denied.rs")?,
                vec![added(2)],
            ),
        ]);

        let blocks = parse_blocks(
            &line_changes,
            ScanMode::OnlyChanged,
            &file_system,
            &FakePathChecker::allow_only("src/**"),
            &language_parsers()?,
            &HashMap::new(),
            &[],
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
            (RepoPath::from_reference("present.rs")?, vec![added(1)]),
            (RepoPath::from_reference("absent.rs")?, vec![added(1)]),
        ]);

        let parsed = parse_blocks(
            &line_changes,
            ScanMode::All,
            &file_system,
            &FakePathChecker::allow_all(),
            &language_parsers()?,
            &HashMap::new(),
            &[],
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
                kind: LineChangeKind::Added,
            }],
        )]);
        let error = parse_blocks(
            &line_changes,
            ScanMode::OnlyChanged,
            &FakeFileSystem::new(HashMap::from([("src/rules.py".to_string(), String::new())])),
            &FakePathChecker::allow_all(),
            &language_parsers()?,
            &HashMap::new(),
            &[],
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
        let line_changes = HashMap::from([(RepoPath::from_reference("rules.py")?, vec![added(1)])]);
        let error = parse_blocks(
            &line_changes,
            ScanMode::All,
            &FakeFileSystem::new(HashMap::from([("src/rules.py".to_string(), String::new())])),
            &FakePathChecker::allow_all(),
            &language_parsers()?,
            &HashMap::new(),
            &[],
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
        let line_changes =
            HashMap::from([(RepoPath::from_reference("docs/gone.py")?, vec![added(1)])]);
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
            &[],
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
                kind: LineChangeKind::Added,
            }],
        )]);
        let blocks = parse_blocks(
            &line_changes,
            ScanMode::OnlyChanged,
            &FakeFileSystem::new(HashMap::new()),
            &FakePathChecker::with_ignored_paths(HashSet::from(["vendor/gone.py".to_string()])),
            &language_parsers()?,
            &HashMap::new(),
            &[],
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
                kind: LineChangeKind::Added,
            }],
        )]);
        let blocks = parse_blocks(
            &line_changes,
            ScanMode::OnlyChanged,
            &FakeFileSystem::new(HashMap::new()),
            &FakePathChecker::allow_all(),
            &language_parsers()?,
            &HashMap::new(),
            &[],
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
            &[],
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
            &[],
            every_block,
            &parsers,
            &HashMap::new(),
            &[],
        );

        assert!(file_blocks.is_err());
        assert_eq!(
            file_blocks.unwrap_err().source().unwrap().to_string(),
            "Block a.py:x at line 1 contains unrecognized attribute `unknown-attr`"
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
            &[],
        );

        assert!(file_blocks.is_err());
        assert_eq!(
            file_blocks.unwrap_err().source().unwrap().to_string(),
            "Block a.py:(unnamed) at line 1 contains unrecognized severity value `invalid-severity`"
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
            &[],
        );

        assert!(file_blocks.is_err());
        assert_eq!(
            file_blocks.unwrap_err().source().unwrap().to_string(),
            "Block a.py:x at line 4 duplicates the name of the block at line 1"
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
            &[],
        )?
        .blocks;

        assert_eq!(blocks.len(), 2);
        Ok(())
    }

    /// The blocks of a run over `files` whose config file declares `virtual_blocks`, with no
    /// diff.
    fn parse_with_virtual_blocks(
        files: &[(&str, &str)],
        path_checker: &FakePathChecker,
        virtual_blocks: &[VirtualBlock],
    ) -> anyhow::Result<HashMap<RepoPath, FileBlocks>> {
        let files = files
            .iter()
            .map(|(path, content)| (path.to_string(), content.to_string()))
            .collect();
        Ok(parse_blocks(
            &HashMap::new(),
            ScanMode::All,
            &FakeFileSystem::new(files),
            path_checker,
            &language_parsers()?,
            &HashMap::new(),
            virtual_blocks,
        )?
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
            &FakePathChecker::allow_all(),
            &virtual_blocks,
        )?;

        let listed: Vec<(serde_json::Value, Option<serde_json::Value>)> = blocks
            [&RepoPath::from_reference("a.json")?]
            .to_serializable_report()
            .into_iter()
            .map(|listing| (listing["name"].clone(), listing.get("config_line").cloned()))
            .collect();
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
        let line_changes = HashMap::from([(RepoPath::from_reference("a.json")?, vec![added(3)])]);

        let blocks = parse_blocks(
            &line_changes,
            ScanMode::OnlyChanged,
            &file_system,
            &FakePathChecker::allow_all(),
            &language_parsers()?,
            &HashMap::new(),
            &virtual_blocks,
        )?
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
        let line_changes = HashMap::from([(RepoPath::from_reference("b.json")?, vec![added(4)])]);

        let parsed = parse_blocks(
            &line_changes,
            ScanMode::OnlyChanged,
            &file_system,
            &FakePathChecker::allow_only("{a,b}.json"),
            &language_parsers()?,
            &HashMap::new(),
            &virtual_blocks,
        )?;

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

        let error = parse_blocks(
            &HashMap::new(),
            ScanMode::OnlyChanged,
            &FakeFileSystem::new(HashMap::new()),
            &FakePathChecker::allow_all(),
            &language_parsers()?,
            &HashMap::new(),
            &[virtual_block],
        )
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
            &FakePathChecker::allow_only("src/**"),
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
            &FakePathChecker::with_ignored_paths(HashSet::from([
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

        let error = parse_with_virtual_blocks(&[], &FakePathChecker::allow_all(), &virtual_blocks)
            .unwrap_err();

        assert_eq!(
            format!("{error:#}"),
            "invalid block at line 7 of \"blockwatch.toml\": target gone.json#/x does not \
             resolve: file does not exist"
        );
        Ok(())
    }

    #[test]
    fn target_file_without_a_parser_parse_blocks_returns_error() -> anyhow::Result<()> {
        let virtual_blocks = [test_utils::virtual_block("notes.txt#/x", &[])?];

        let error = parse_with_virtual_blocks(
            &[("notes.txt", "x")],
            &FakePathChecker::allow_all(),
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
        let virtual_blocks = [test_utils::virtual_block("a.py#/x", &[])?];

        let error = parse_with_virtual_blocks(
            &[("a.py", "x = 1\n")],
            &FakePathChecker::allow_all(),
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
            &FakePathChecker::allow_all(),
            &virtual_blocks,
        )
        .unwrap_err();

        assert_eq!(
            format!("{error:#}"),
            "Block a.json:n at line 7 of \"blockwatch.toml\" duplicates the name of the block at \
             line 3"
        );
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
                "json.json".to_string(),
                "// <block>\n{\"key\": 1}\n// </block>".to_string(),
            ),
            (
                "jsonc.jsonc".to_string(),
                "// <block>\n{\"key\": 1}\n// </block>".to_string(),
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
            &[],
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
