use crate::Position;
use crate::diff_parser::LineChange;
use crate::language_parsers::LanguageParser;
use anyhow::{Context, anyhow};
use globset::GlobSet;
use ignore::Walk;
use serde_repr::Serialize_repr;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::ffi::OsString;
use std::ops::{Range, RangeInclusive};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use strum_macros::EnumString;

const UNNAMED_BLOCK_LABEL: &str = "(unnamed)";

/// Represents a `block` tag parsed from the source file comments.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Block {
    // Optional attributes in the `block` tag.
    pub(crate) attributes: HashMap<String, String>,
    // Block's start tag position range ("<" symbol to ">" symbol).
    pub(crate) start_tag_position_range: RangeInclusive<Position>,
    // Block's content substring range in the original source code.
    pub(crate) content_bytes_range: Range<usize>,
    // Block's content position range in the original source code (from the end of the comment with
    // the start tag to the beginning of the comment with the end tag).
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
    ///
    /// `new_line_positions` is used for locating a starting position of a line in the source code.
    fn content_intersects_with_any(&self, line_changes: &[LineChange]) -> bool {
        line_changes
            .binary_search_by(|line_change: &LineChange| {
                if Self::intersects_with_line_change(&self.content_position_range, line_change) {
                    Ordering::Equal
                } else if line_change.line < self.content_position_range.start.line {
                    Ordering::Less
                } else {
                    Ordering::Greater
                }
            })
            .is_ok()
    }

    /// Whether the `Block`'s start tag intersects with any of the **ordered** `line_changes`.
    ///
    /// `new_line_positions` is used for locating a starting position of a line in the source code.
    fn start_tag_intersects_with_any(&self, line_changes: &[LineChange]) -> bool {
        line_changes
            .binary_search_by(|line_change: &LineChange| {
                if Self::intersects_with_line_change_inclusive(
                    &self.start_tag_position_range,
                    line_change,
                ) {
                    Ordering::Equal
                } else if line_change.line < self.start_tag_position_range.start().line {
                    Ordering::Less
                } else {
                    Ordering::Greater
                }
            })
            .is_ok()
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

    /// Returns the block's severity.
    pub(crate) fn severity(&self) -> anyhow::Result<BlockSeverity> {
        self.attributes
            .get("severity")
            .map_or(Ok(BlockSeverity::Error), |s| {
                BlockSeverity::from_str(s.as_str())
                    .context("Failed to parse \"severity\" attribute")
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
    Error = 1,
    Warning = 2,
    Info = 3,
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
    pub(crate) fn to_serializable_report(&self) -> Vec<serde_json::Value> {
        let mut listings = Vec::new();
        for block in &self.blocks_with_context {
            listings.push(serde_json::json!({
                // <block affects="README.md:list-output-example">
                "name": block.block.name_display(),
                "line": block.block.start_tag_position_range.start().line,
                "column": block.block.start_tag_position_range.start().character,
                "is_content_modified": block.is_content_modified,
                "attributes": block.block.attributes,
                // </block>
            }));
        }
        // Sort by line number for deterministic output
        listings.sort_by_key(|b| b["line"].as_u64().unwrap_or(0));
        listings
    }
}

/// Represents a block with its corresponding validation context.
#[derive(Debug, Clone)]
pub struct BlockWithContext {
    pub(crate) block: Block,
    // Whether the block's tag is modified (computed from the input diff).
    pub(crate) _is_start_tag_modified: bool,
    // Whether the content of the block is modified (computed from the input diff).
    pub(crate) is_content_modified: bool,
}

/// Parses source files and returns only those blocks that intersect with the provided modified line ranges.
///
/// - `line_changes_by_file` maps file paths to sorted line changes.
/// - `should_scan_files` indicates whether all the files in filesystem should be scanned for blocks.
/// - `file_system` provides access to file contents within a root path.
/// - `parsers` maps file extensions to language-specific block parsers.
/// - `extra_file_extensions` allows remapping unknown extensions to supported ones (e.g., "cxx" -> "cpp").
///
/// Returns a map of file paths to the list of intersecting blocks found in that file.
pub fn parse_blocks(
    mut line_changes_by_file: HashMap<PathBuf, Vec<LineChange>>,
    should_scan_files: bool,
    file_system: &impl FileSystem,
    path_checker: &impl PathChecker,
    parsers: HashMap<OsString, LanguageParser>,
    extra_file_extensions: HashMap<OsString, OsString>,
) -> anyhow::Result<HashMap<PathBuf, FileBlocks>> {
    let mut blocks = HashMap::new();
    if should_scan_files {
        for result in file_system.walk() {
            match result {
                Ok(file_path) => {
                    if !path_checker.should_allow(&file_path)
                        || path_checker.should_ignore(&file_path)
                    {
                        continue;
                    }
                    let changes_owned = line_changes_by_file.remove(&file_path);
                    let line_changes = changes_owned.as_deref().unwrap_or(&[]);
                    let file_blocks_opt = parse_file(
                        file_path.as_path(),
                        line_changes,
                        BlocksFilter::All,
                        file_system,
                        &parsers,
                        &extra_file_extensions,
                    )?;
                    if let Some(file_blocks) = file_blocks_opt
                        && !file_blocks.is_empty()
                    {
                        blocks.insert(file_path, file_blocks);
                    }
                }
                Err(err) => {
                    return Err(anyhow!("Failed to walk directory: {err}"));
                }
            }
        }
    }
    // Parse remaining files in `line_changes_by_file` from the given diff input (if any).
    for (file_path, line_changes) in line_changes_by_file {
        if path_checker.should_ignore(&file_path) {
            // Not calling `path_checker.should_allow()` because all the files in the
            // `line_changes_by_file` are implicitly allowed.
            continue;
        }
        let file_blocks_opt = parse_file(
            file_path.as_path(),
            line_changes.as_slice(),
            BlocksFilter::ModifiedOnly,
            file_system,
            &parsers,
            &extra_file_extensions,
        )?;
        if let Some(file_blocks) = file_blocks_opt
            && !file_blocks.is_empty()
        {
            blocks.insert(file_path.clone(), file_blocks);
        }
    }
    Ok(blocks)
}

enum BlocksFilter {
    All,
    ModifiedOnly,
}

fn parse_file(
    file_path: &Path,
    line_changes: &[LineChange],
    blocks_filter: BlocksFilter,
    file_reader: &impl FileSystem,
    parsers: &HashMap<OsString, LanguageParser>,
    extra_file_extensions: &HashMap<OsString, OsString>,
) -> anyhow::Result<Option<FileBlocks>> {
    let parser = match parser_for_file_path(file_path, parsers, extra_file_extensions) {
        None => return Ok(None),
        Some(p) => p,
    };
    let source_code = file_reader.read_to_string(file_path)?;
    let blocks = parser
        .borrow_mut()
        .parse(&source_code)
        .context(format!("Failed to parse file {file_path:?}"))?;

    let blocks_with_context = blocks
        .into_iter()
        .filter_map(|block| {
            let is_content_modified = block.content_intersects_with_any(line_changes);
            let is_start_tag_modified = block.start_tag_intersects_with_any(line_changes);

            if matches!(blocks_filter, BlocksFilter::All)
                || is_content_modified
                || is_start_tag_modified
            {
                Some(BlockWithContext {
                    block,
                    _is_start_tag_modified: is_start_tag_modified,
                    is_content_modified,
                })
            } else {
                None
            }
        })
        .collect();

    Ok(Some(FileBlocks {
        file_content: source_code,
        blocks_with_context,
    }))
}

fn parser_for_file_path<'p>(
    file_path: &Path,
    parsers: &'p HashMap<OsString, LanguageParser>,
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
    parsers: &'p HashMap<OsString, LanguageParser>,
    extra_file_extensions: &HashMap<OsString, OsString>,
) -> Option<&'p LanguageParser> {
    let ext = if let Some(ext) = extra_file_extensions.get(extension) {
        ext
    } else {
        extension
    };
    parsers.get(ext)
}

pub trait FileSystem {
    /// Reads the entire contents of a file into a string.
    fn read_to_string(&self, path: &Path) -> anyhow::Result<String>;

    /// Walks the directory tree rooted at the file system's root path, returning an iterator over the paths of all files.
    fn walk(&self) -> impl Iterator<Item = anyhow::Result<PathBuf>>;
}

/// Checks whether a path should be allowed or ignored when parsing blocks from files.
pub trait PathChecker {
    /// Whether the given `path` should be explicitly allowed.
    fn should_allow(&self, path: &Path) -> bool;

    /// Whether the given `path` should be explicitly ignored.
    fn should_ignore(&self, path: &Path) -> bool;
}

pub struct FileSystemImpl {
    root_path: PathBuf,
}

impl FileSystemImpl {
    /// Creates a new filesystem-backed reader rooted at `root_path`.
    pub fn new(root_path: PathBuf) -> Self {
        Self { root_path }
    }
}

impl FileSystem for FileSystemImpl {
    fn read_to_string(&self, path: &Path) -> anyhow::Result<String> {
        std::fs::read_to_string(self.root_path.join(path))
            .context(format!("Failed to read file \"{}\"", path.display()))
    }

    fn walk(&self) -> impl Iterator<Item = anyhow::Result<PathBuf>> {
        // Clone root_path for the closure.
        let root_path = self.root_path.clone();
        Walk::new(&self.root_path).filter_map(move |entry| match entry {
            Ok(entry) => {
                let path = entry.path();
                if path.is_dir() {
                    return None;
                }
                // Return path relative to the root.
                let relative_path = path.strip_prefix(&root_path).unwrap_or(path);
                Some(Ok(relative_path.to_path_buf()))
            }
            Err(err) => Some(Err(anyhow::Error::from(err))),
        })
    }
}

/// Checks whether a path should be allowed or ignored.
pub struct PathCheckerImpl {
    glob_set: GlobSet,
    ignored_glob_set: GlobSet,
}

impl PathCheckerImpl {
    pub fn new(glob_set: GlobSet, ignored_glob_set: GlobSet) -> Self {
        Self {
            glob_set,
            ignored_glob_set,
        }
    }
}

impl PathChecker for PathCheckerImpl {
    fn should_allow(&self, path: &Path) -> bool {
        self.glob_set.is_match(path)
    }

    fn should_ignore(&self, path: &Path) -> bool {
        self.ignored_glob_set.is_match(path)
    }
}

#[cfg(test)]
mod block_severity_from_str_tests {
    use crate::Position;
    use crate::blocks::{Block, BlockSeverity};
    use std::collections::HashMap;

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
    use crate::language_parsers::language_parsers;
    use crate::test_utils::FakeFileSystem;
    use crate::test_utils::{self, FakePathChecker};
    use std::collections::HashSet;

    /// Creates a whole line change (either added or deleted line).
    fn line_change(line: usize) -> LineChange {
        LineChange { line, ranges: None }
    }

    #[test]
    fn with_nonempty_line_changes_no_scan_files_returns_only_blocks_with_modified_start_tag_or_content()
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
                PathBuf::from("a.rs"),
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
                PathBuf::from("b.rs"),
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
            line_changes,
            false,
            &file_system,
            &FakePathChecker::allow_all(),
            parsers,
            HashMap::new(),
        )?;

        assert_eq!(blocks_by_file.len(), 2);
        let blocks_a = &blocks_by_file[&PathBuf::from("a.rs")].blocks_with_context;
        assert_eq!(blocks_a.len(), 9);
        let first = &blocks_a[0];
        assert_eq!(first.block.name(), Some("first"));
        assert!(first._is_start_tag_modified);
        assert!(!first.is_content_modified);
        let second = &blocks_a[1];
        assert_eq!(second.block.name(), Some("second"));
        assert!(second._is_start_tag_modified);
        assert!(second.is_content_modified);
        let third = &blocks_a[2];
        assert_eq!(third.block.name(), Some("third"));
        assert!(!third._is_start_tag_modified);
        assert!(third.is_content_modified);
        let fourth = &blocks_a[3];
        assert_eq!(fourth.block.name(), Some("fourth"));
        assert!(!fourth._is_start_tag_modified);
        assert!(fourth.is_content_modified);
        let sixth = &blocks_a[4];
        assert_eq!(sixth.block.name(), Some("sixth"));
        assert!(sixth._is_start_tag_modified);
        assert!(!sixth.is_content_modified);
        let seventh = &blocks_a[5];
        assert_eq!(seventh.block.name(), Some("seventh"));
        assert!(seventh._is_start_tag_modified);
        assert!(!seventh.is_content_modified);
        let eighth = &blocks_a[6];
        assert_eq!(eighth.block.name(), Some("eighth"));
        assert!(!eighth._is_start_tag_modified);
        assert!(eighth.is_content_modified);
        let ninth = &blocks_a[7];
        assert_eq!(ninth.block.name(), Some("ninth"));
        assert!(!ninth._is_start_tag_modified);
        assert!(ninth.is_content_modified);
        let tenth = &blocks_a[8];
        assert_eq!(tenth.block.name(), Some("tenth"));
        assert!(!tenth._is_start_tag_modified);
        assert!(tenth.is_content_modified);
        let blocks_b = &blocks_by_file[&PathBuf::from("b.rs")].blocks_with_context;
        assert_eq!(blocks_b.len(), 1);
        assert_eq!(blocks_b[0].block.name(), Some("first"));

        Ok(())
    }

    #[test]
    fn with_nonempty_line_changes_with_scan_files_parses_modified_and_unmodified_blocks()
    -> anyhow::Result<()> {
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
                PathBuf::from("a.rs"),
                vec![LineChange {
                    line: 3, // Content line of the first block.
                    ranges: None,
                }],
            ),
            (
                PathBuf::from("b.rs"),
                vec![LineChange {
                    line: 3, // Content line of the first block.
                    ranges: None,
                }],
            ),
        ]);
        let blocks_by_file = parse_blocks(
            line_changes,
            true,
            &file_system,
            &FakePathChecker::allow_all(),
            parsers,
            HashMap::new(),
        )?;

        assert_eq!(
            blocks_by_file[&PathBuf::from("a.rs")]
                .blocks_with_context
                .iter()
                .map(|b| { (b.block.name().unwrap(), b.is_content_modified) })
                .collect::<Vec<(&str, bool)>>(),
            &[("first_from_a", true), ("second_from_a", false)]
        );
        assert_eq!(
            blocks_by_file[&PathBuf::from("b.rs")]
                .blocks_with_context
                .iter()
                .map(|b| { (b.block.name().unwrap(), b.is_content_modified) })
                .collect::<Vec<(&str, bool)>>(),
            &[("first_from_b", true), ("second_from_b", false)]
        );
        Ok(())
    }

    #[test]
    fn with_empty_line_changes_with_scan_files_parses_unmodified_blocks() -> anyhow::Result<()> {
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
            HashMap::new(),
            true,
            &file_system,
            &FakePathChecker::allow_all(),
            parsers,
            HashMap::new(),
        )?;

        assert_eq!(
            blocks_by_file[&PathBuf::from("a.rs")]
                .blocks_with_context
                .iter()
                .map(|b| { (b.block.name().unwrap(), b.is_content_modified) })
                .collect::<Vec<(&str, bool)>>(),
            &[("first_from_a", false), ("second_from_a", false)]
        );
        assert_eq!(
            blocks_by_file[&PathBuf::from("b.rs")]
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
            HashMap::new(),
            true,
            &file_system,
            &FakePathChecker::allow_all(),
            parsers,
            HashMap::new(),
        )?;

        let content_a = &blocks_by_file[&PathBuf::from("a.rs")].file_content;
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
            HashMap::new(),
            true,
            &file_system,
            &FakePathChecker::allow_all(),
            parsers,
            HashMap::from([("rust".into(), "rs".into())]),
        )?;

        assert_eq!(blocks_by_file.len(), 1);
        assert_eq!(
            blocks_by_file[&PathBuf::from("a.rust")]
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
            HashMap::new(),
            true,
            &FakeFileSystem::new(files),
            &FakePathChecker::allow_all(),
            HashMap::new(),
            HashMap::new(),
        )?;

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
            HashMap::new(),
            true,
            &file_system,
            &path_checker,
            language_parsers()?,
            HashMap::new(),
        )?;

        assert_eq!(blocks.len(), 1);
        assert!(blocks.contains_key(&PathBuf::from("allowed.rs")));
        assert!(!blocks.contains_key(&PathBuf::from("ignored.rs")));
        Ok(())
    }

    #[test]
    fn empty_input_returns_empty_result() -> anyhow::Result<()> {
        let line_changes = HashMap::default();
        let blocks = parse_blocks(
            line_changes,
            true,
            &FakeFileSystem::new(HashMap::default()),
            &FakePathChecker::allow_all(),
            HashMap::new(),
            HashMap::new(),
        )?;

        assert_eq!(blocks.len(), 0);
        Ok(())
    }
}

#[cfg(test)]
mod supported_languages_tests {
    use std::{collections::HashMap, path::PathBuf};

    use crate::blocks::*;
    use crate::language_parsers::language_parsers;
    use crate::test_utils::{FakeFileSystem, FakePathChecker};

    // <block name="supported-extensions">
    #[test]
    fn all_language_extensions_are_supported() -> anyhow::Result<()> {
        let parsers = language_parsers()?;
        let files = HashMap::from([
            (
                "bash.bash".to_string(),
                "# <block>\necho \"hello\"\n# </block>".to_string(),
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
                "h.h".to_string(),
                "// <block>\nvoid foo();\n// </block>".to_string(),
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
                "php.php".to_string(),
                "<?php\n# <block>\necho 'hello';\n# </block>\n?>".to_string(),
            ),
            (
                "phtml.phtml".to_string(),
                "<?php\n# <block>\necho 'world';\n# </block>\n?>".to_string(),
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
                "sh.sh".to_string(),
                "# <block>\necho \"hello\"\n# </block>".to_string(),
            ),
            (
                "sql.sql".to_string(),
                "-- <block>\nSELECT * FROM users;\n-- </block>".to_string(),
            ),
            (
                "swift.swift".to_string(),
                "// <block>\nfunc main() {}\n// </block>".to_string(),
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
            HashMap::new(),
            true,
            &file_system,
            &FakePathChecker::allow_all(),
            parsers,
            HashMap::new(),
        )?;

        for file_name in files.keys() {
            assert!(
                !blocks_by_file
                    .get(&PathBuf::from(file_name))
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
