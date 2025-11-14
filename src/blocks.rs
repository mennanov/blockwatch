use crate::Position;
use crate::differ::LineChange;
use crate::parsers::BlocksParser;
use anyhow::Context;
use serde_repr::Serialize_repr;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::str::FromStr;
use strum_macros::EnumString;

const UNNAMED_BLOCK_LABEL: &str = "(unnamed)";

/// Represents a `block` tag parsed from the source file comments.
#[derive(Debug, PartialEq, Clone)]
pub struct Block {
    // Source line number with the `block` tag.
    pub(crate) starts_at_line: usize,
    // Source line number with the corresponding closed `block` tag.
    pub(crate) ends_at_line: usize,
    // Optional attributes in the `block` tag.
    pub(crate) attributes: HashMap<String, String>,
    // Block's start tag range in the original source code.
    pub(crate) start_tag_range: Range<usize>,
    // Block's content substring range in the original source code.
    content_range: Range<usize>,
}

impl Block {
    /// Creates a new `Block` with the given attributes and content indexes.
    pub(crate) fn new(
        starts_at_line: usize,
        ends_at_line: usize,
        attributes: HashMap<String, String>,
        start_tag_range: Range<usize>,
        content_range: Range<usize>,
    ) -> Self {
        Self {
            starts_at_line,
            ends_at_line,
            attributes,
            start_tag_range,
            content_range,
        }
    }

    /// Whether the `Block` intersects with the given `line_change`.
    fn intersects_with_line_change(
        &self,
        range: &Range<usize>,
        line_change: &LineChange,
        new_line_positions: &[usize],
    ) -> bool {
        if line_change.line < self.starts_at_line || line_change.line > self.ends_at_line {
            // `line_change` is outside the block's start and end tags.
            return false;
        }
        let content_start_position = Position::from_byte_offset(range.start, new_line_positions);
        if line_change.line < content_start_position.line {
            // `line_change` is before the block's content start line.
            return false;
        }

        let content_end_position = Position::from_byte_offset(range.end - 1, new_line_positions);
        if line_change.line > content_end_position.line {
            // `line_change` is after the block's content end line.
            return false;
        }

        if let Some(ranges) = &line_change.ranges {
            let line_start_character = if line_change.line == content_start_position.line {
                content_start_position.character
            } else {
                0
            };
            let line_end_character = if line_change.line < content_end_position.line {
                usize::MAX
            } else {
                content_end_position.character
            };

            ranges
                .binary_search_by(|range| {
                    if range.end > line_start_character && range.start <= line_end_character {
                        // Intersection between [line_start_character, line_end_character]
                        // and half-open [range.start, range.end).
                        Ordering::Equal
                    } else if range.end <= line_start_character {
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

    /// Whether the `Block`'s content intersects with any of the **ordered** `line_changes`.
    ///
    /// `new_line_positions` is used for locating a starting position of a line in the source code.
    fn content_intersects_with_any(
        &self,
        line_changes: &[LineChange],
        new_line_positions: &[usize],
    ) -> bool {
        self.intersects_with_any(&self.content_range, line_changes, new_line_positions)
    }

    /// Whether the `Block`'s start tag intersects with any of the **ordered** `line_changes`.
    ///
    /// `new_line_positions` is used for locating a starting position of a line in the source code.
    fn start_tag_intersects_with_any(
        &self,
        line_changes: &[LineChange],
        new_line_positions: &[usize],
    ) -> bool {
        self.intersects_with_any(&self.start_tag_range, line_changes, new_line_positions)
    }

    /// Whether the `Block`'s `range` intersects with any of the given `line_changes`.
    fn intersects_with_any(
        &self,
        range: &Range<usize>,
        line_changes: &[LineChange],
        new_line_positions: &[usize],
    ) -> bool {
        line_changes
            .binary_search_by(|line_change: &LineChange| {
                if self.intersects_with_line_change(range, line_change, new_line_positions) {
                    Ordering::Equal
                } else if line_change.line < self.starts_at_line {
                    Ordering::Less
                } else {
                    Ordering::Greater
                }
            })
            .is_ok()
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
        &source[self.content_range.clone()]
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
    /// Newline positions in the `file_content`.
    /// Can be used to convert a byte offset to a line number and a character position in log(N).
    pub(crate) file_content_new_lines: Vec<usize>,
    /// Blocks to be validated.
    pub(crate) blocks_with_context: Vec<BlockWithContext>,
}

/// Represents a block with its corresponding validation context.
#[derive(Debug)]
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
/// - `file_reader` provides async access to file contents within a root path.
/// - `parsers` maps file extensions to language-specific block parsers.
/// - `extra_file_extensions` allows remapping unknown extensions to supported ones (e.g., "cxx" -> "cpp").
///
/// Returns a map of file paths to the list of intersecting blocks found in that file.
pub fn parse_blocks(
    line_changes_by_file: &HashMap<String, Vec<LineChange>>,
    file_reader: &impl FileReader,
    parsers: HashMap<String, Rc<Box<dyn BlocksParser>>>,
    extra_file_extensions: HashMap<String, String>,
) -> anyhow::Result<HashMap<String, FileBlocks>> {
    let mut blocks = HashMap::new();
    for (file_path, line_changes) in line_changes_by_file {
        let source_code = file_reader.read_to_string(Path::new(&file_path))?;
        let new_line_positions: Vec<usize> = source_code
            .match_indices('\n')
            .map(|(idx, _)| idx)
            .collect();
        let mut file_blocks = Vec::new();
        if let Some(mut ext) = file_name_extension(file_path) {
            ext = extra_file_extensions
                .get(ext)
                .map(|e| e.as_str())
                .unwrap_or(ext);
            if let Some(parser) = parsers.get(ext) {
                for block in parser
                    .parse(&source_code)
                    .context(format!("Failed to parse file \"{file_path}\""))?
                {
                    let is_content_modified =
                        block.content_intersects_with_any(line_changes, &new_line_positions);
                    let is_start_tag_modified =
                        block.start_tag_intersects_with_any(line_changes, &new_line_positions);
                    if !is_content_modified && !is_start_tag_modified {
                        // Skip untouched blocks.
                        continue;
                    }
                    file_blocks.push(BlockWithContext {
                        block,
                        _is_start_tag_modified: is_start_tag_modified,
                        is_content_modified,
                    });
                }
            }
        }
        if !file_blocks.is_empty() {
            blocks.insert(
                file_path.to_string(),
                FileBlocks {
                    file_content: source_code,
                    file_content_new_lines: new_line_positions,
                    blocks_with_context: file_blocks,
                },
            );
        }
    }
    Ok(blocks)
}

pub trait FileReader {
    /// Reads the entire contents of a file into a string.
    fn read_to_string(&self, path: &Path) -> anyhow::Result<String>;
}

fn file_name_extension(file_name: &str) -> Option<&str> {
    file_name.rsplit('.').next()
}

pub struct FsReader {
    root_path: PathBuf,
}

impl FsReader {
    /// Creates a new filesystem-backed reader rooted at `root_path`.
    pub fn new(root_path: PathBuf) -> Self {
        Self { root_path }
    }
}

impl FileReader for FsReader {
    fn read_to_string(&self, path: &Path) -> anyhow::Result<String> {
        std::fs::read_to_string(self.root_path.join(path))
            .context(format!("Failed to read file \"{}\"", path.display()))
    }
}

#[cfg(test)]
mod block_severity_from_str_tests {
    use crate::blocks::{Block, BlockSeverity};
    use std::collections::HashMap;

    pub(crate) fn new_empty_block_with_severity(severity: &str) -> Block {
        Block::new(
            0,
            0,
            HashMap::from([("severity".into(), severity.into())]),
            0..0,
            0..0,
        )
    }

    #[test]
    fn with_valid_value_returns_corresponding_severity() {
        let block = new_empty_block_with_severity("warning");

        assert_eq!(block.severity().unwrap(), BlockSeverity::Warning);
    }

    #[test]
    fn severity_is_case_insensitive() {
        let block = new_empty_block_with_severity("InFo");

        assert_eq!(block.severity().unwrap(), BlockSeverity::Info);
    }

    #[test]
    fn block_with_no_severity_returns_error_severity_by_default() {
        let block = Block::new(0, 0, HashMap::new(), 0..0, 0..0);

        assert_eq!(block.severity().unwrap(), BlockSeverity::Error);
    }

    #[test]
    fn with_invalid_value_returns_error() {
        let block = new_empty_block_with_severity("warn");

        assert!(block.severity().is_err());
    }
}

#[cfg(test)]
mod parse_blocks_tests {
    use crate::blocks::*;
    use crate::parsers::language_parsers;
    use crate::test_utils;

    struct FakeFileReader {
        files: HashMap<String, String>,
    }

    impl FakeFileReader {
        fn new(files: HashMap<String, String>) -> Self {
            Self { files }
        }
    }

    impl FileReader for FakeFileReader {
        fn read_to_string(&self, path: &Path) -> anyhow::Result<String> {
            Ok(self.files[&path.display().to_string()].clone())
        }
    }

    /// Creates a whole line change (either added or deleted line).
    fn line_change(line: usize) -> LineChange {
        LineChange { line, ranges: None }
    }

    #[test]
    fn returns_modified_blocks_from_multiple_files() -> anyhow::Result<()> {
        let file_reader = FakeFileReader::new(HashMap::from([
            (
                "a.rs".to_string(),
                r#"
        // <block name="first">
        fn a() {}
        // </block>
        // <block name="second">
        fn b() {
            println!("hello");
            println!("world");
        }
        // </block>
        "#
                .to_string(),
            ),
            (
                "b.rs".to_string(),
                r#"
        // <block name="outer">
        fn outer() {
            // <block name="inner">
            println!("hello");
            println!("world");
            // </block>
        }
        // </block>
        "#
                .to_string(),
            ),
            (
                "c.rs".to_string(),
                r#"
        // <block name="target">
        fn c() {}
        // </block>
        fn d() {
            println!("hello");
        }
        "#
                .to_string(),
            ),
        ]));
        let modified_ranges = HashMap::from([
            (
                "a.rs".to_string(),
                vec![line_change(3), line_change(7), line_change(8)],
            ), // Both blocks are modified.
            ("b.rs".to_string(), vec![line_change(5), line_change(6)]), // The inner block is modified.
            ("c.rs".to_string(), vec![line_change(6), line_change(7)]), // No block is modified.
        ]);
        let parsers = language_parsers()?;

        let file_blocks_by_file =
            parse_blocks(&modified_ranges, &file_reader, parsers, HashMap::new())?;

        assert_eq!(file_blocks_by_file.len(), 2);
        let blocks_a = &file_blocks_by_file["a.rs"].blocks_with_context;
        assert_eq!(blocks_a.len(), 2);
        assert_eq!(blocks_a[0].block.name(), Some("first"));
        assert_eq!(blocks_a[1].block.name(), Some("second"));
        let blocks_b = &file_blocks_by_file["b.rs"].blocks_with_context;
        assert_eq!(blocks_b.len(), 2);
        assert_eq!(blocks_b[0].block.name(), Some("outer"));
        assert_eq!(blocks_b[1].block.name(), Some("inner"));
        Ok(())
    }

    #[test]
    fn returns_blocks_with_modified_start_tag_or_contents_only() -> anyhow::Result<()> {
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
        // <block name="nineth">
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
        let file_reader = FakeFileReader::new(HashMap::from([
            ("a.rs".to_string(), content_a.to_string()),
            ("b.rs".to_string(), content_b.to_string()),
        ]));
        let line_changes = HashMap::from([
            (
                "a.rs".to_string(),
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
                        // "nineth" block.
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
                "b.rs".to_string(),
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

        let blocks_by_file = parse_blocks(&line_changes, &file_reader, parsers, HashMap::new())?;

        assert_eq!(blocks_by_file.len(), 2);
        let blocks_a = &blocks_by_file["a.rs"].blocks_with_context;
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
        let nineth = &blocks_a[7];
        assert_eq!(nineth.block.name(), Some("nineth"));
        assert!(!nineth._is_start_tag_modified);
        assert!(nineth.is_content_modified);
        let tenth = &blocks_a[8];
        assert_eq!(tenth.block.name(), Some("tenth"));
        assert!(!tenth._is_start_tag_modified);
        assert!(tenth.is_content_modified);
        let blocks_b = &blocks_by_file["b.rs"].blocks_with_context;
        assert_eq!(blocks_b.len(), 1);
        assert_eq!(blocks_b[0].block.name(), Some("first"));

        Ok(())
    }

    #[test]
    fn returns_file_contents_correctly() -> anyhow::Result<()> {
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
        let file_reader = FakeFileReader::new(HashMap::from([(
            "a.rs".to_string(),
            file_a_contents.to_string(),
        )]));
        let modified_ranges = HashMap::from([
            (
                "a.rs".to_string(),
                vec![line_change(3), line_change(7), line_change(8)],
            ), // Both blocks are modified.
        ]);
        let parsers = language_parsers()?;

        let blocks_by_file = parse_blocks(&modified_ranges, &file_reader, parsers, HashMap::new())?;

        let content_a = &blocks_by_file["a.rs"].file_content;
        assert_eq!(content_a, file_a_contents);
        Ok(())
    }

    #[test]
    fn uses_remapped_extensions() -> anyhow::Result<()> {
        let file_reader = FakeFileReader::new(HashMap::from([(
            "a.rust".to_string(),
            r#"
        // <block name="first">
        fn a() {}
        // </block>"#
                .to_string(),
        )]));
        let modified_ranges = HashMap::from([("a.rust".to_string(), vec![line_change(3)])]);
        let parsers = language_parsers()?;

        let blocks_by_file = parse_blocks(
            &modified_ranges,
            &file_reader,
            parsers,
            HashMap::from([("rust".to_string(), "rs".to_string())]),
        )?;

        assert_eq!(blocks_by_file.len(), 1);
        assert_eq!(blocks_by_file["a.rust"].blocks_with_context.len(), 1);
        Ok(())
    }

    #[test]
    fn skips_unknown_files() -> anyhow::Result<()> {
        let files = HashMap::from([("test.unknown".to_string(), "test content".to_string())]);
        let modified_ranges = HashMap::from([(
            "test.unknown".to_string(),
            vec![line_change(1), line_change(2)],
        )]);

        let blocks = parse_blocks(
            &modified_ranges,
            &FakeFileReader::new(files),
            HashMap::new(),
            HashMap::new(),
        )?;

        assert_eq!(blocks.len(), 0);
        Ok(())
    }

    #[test]
    fn empty_input_returns_ok() -> anyhow::Result<()> {
        let blocks = parse_blocks(
            &HashMap::default(),
            &FakeFileReader::new(HashMap::default()),
            HashMap::new(),
            HashMap::new(),
        )?;

        assert_eq!(blocks.len(), 0);
        Ok(())
    }
}
