use crate::parsers::BlocksParser;
use anyhow::Context;
use serde::Serialize;
use serde_repr::Serialize_repr;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::str::FromStr;
use std::sync::Arc;
use strum_macros::EnumString;

const UNNAMED_BLOCK_LABEL: &str = "(unnamed)";

/// Represents a `block` tag parsed from the source file comments.
#[derive(Debug, PartialEq, Clone, Serialize)]
pub struct Block {
    // Source line number with the `block` tag.
    pub(crate) starts_at_line: usize,
    // Source line number with the corresponding closed `block` tag.
    pub(crate) ends_at_line: usize,
    // Optional attributes in the `block` tag.
    pub(crate) attributes: HashMap<String, String>,
    // Block's content substring range in the original source code.
    #[serde(skip_serializing)]
    content_range: Range<usize>,
}

impl Block {
    /// Creates a new `Block` with the given attributes and content indexes.
    pub(crate) fn new(
        starts_at_line: usize,
        ends_at_line: usize,
        attributes: HashMap<String, String>,
        content_range: Range<usize>,
    ) -> Self {
        Self {
            starts_at_line,
            ends_at_line,
            attributes,
            content_range,
        }
    }

    /// Whether the `Block` intersects with the given closed-closed interval of `start` and `end`.
    pub(crate) fn intersects_with(&self, start: usize, end: usize) -> bool {
        self.ends_at_line >= start && end >= self.starts_at_line
    }

    /// Whether the `Block` intersects with any of the **ordered** `ranges`.
    pub(crate) fn intersects_with_any(&self, ranges: &[(usize, usize)]) -> bool {
        let idx = ranges.binary_search_by(|(start, end)| {
            if self.intersects_with(*start, *end) {
                Ordering::Equal
            } else if *end < self.starts_at_line {
                Ordering::Less
            } else {
                Ordering::Greater
            }
        });
        idx.is_ok()
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
    pub(crate) file_contents: String,
    pub(crate) blocks: Vec<Arc<Block>>,
}

/// Parses source files and returns only those blocks that intersect with the provided modified line ranges.
///
/// - `modified_ranges_by_file` maps file paths to sorted, non-overlapping closed line ranges that were changed.
/// - `file_reader` provides async access to file contents within a root path.
/// - `parsers` maps file extensions to language-specific block parsers.
/// - `extra_file_extensions` allows remapping unknown extensions to supported ones (e.g., "cxx" -> "cpp").
///
/// Returns a map of file paths to the list of intersecting blocks found in that file.
pub fn parse_blocks(
    modified_ranges_by_file: &HashMap<String, Vec<(usize, usize)>>,
    file_reader: &impl FileReader,
    parsers: HashMap<String, Rc<Box<dyn BlocksParser>>>,
    extra_file_extensions: HashMap<String, String>,
) -> anyhow::Result<HashMap<String, FileBlocks>> {
    let mut blocks = HashMap::new();
    for (file_path, modified_ranges) in modified_ranges_by_file {
        let source_code = file_reader.read_to_string(Path::new(&file_path))?;
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
                    if !block.intersects_with_any(modified_ranges) {
                        // Skip untouched blocks.
                        continue;
                    }
                    file_blocks.push(Arc::new(block));
                }
            }
        }
        if !file_blocks.is_empty() {
            blocks.insert(
                file_path.to_string(),
                FileBlocks {
                    file_contents: source_code,
                    blocks: file_blocks,
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
mod test_utils {
    use crate::blocks::Block;
    use std::collections::HashMap;

    pub(crate) fn new_empty_block(starts_at: usize, ends_at: usize) -> Block {
        Block::new(starts_at, ends_at, HashMap::new(), 0..0)
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
        let block = Block::new(0, 0, HashMap::new(), 0..0);

        assert_eq!(block.severity().unwrap(), BlockSeverity::Error);
    }

    #[test]
    fn with_invalid_value_returns_error() {
        let block = new_empty_block_with_severity("warn");

        assert!(block.severity().is_err());
    }
}

#[cfg(test)]
mod block_intersects_with_tests {
    use crate::blocks::test_utils::*;

    #[test]
    fn non_overlapping_returns_false() {
        let block = new_empty_block(3, 4);

        assert!(!block.intersects_with(1, 2));
        assert!(!block.intersects_with(2, 2));
        assert!(!block.intersects_with(5, 5));
        assert!(!block.intersects_with(5, 6));
    }

    #[test]
    fn non_overlapping_single_line_returns_false() {
        let block = new_empty_block(3, 3);

        assert!(!block.intersects_with(1, 2));
        assert!(!block.intersects_with(2, 2));
        assert!(!block.intersects_with(4, 4));
        assert!(!block.intersects_with(4, 5));
    }

    #[test]
    fn overlapping_returns_true() {
        let block = new_empty_block(3, 6);

        assert!(block.intersects_with(1, 3));
        assert!(block.intersects_with(1, 7));
        assert!(block.intersects_with(2, 4));
        assert!(block.intersects_with(3, 3));
        assert!(block.intersects_with(3, 4));
        assert!(block.intersects_with(3, 6));
        assert!(block.intersects_with(4, 5));
        assert!(block.intersects_with(4, 6));
        assert!(block.intersects_with(5, 7));
        assert!(block.intersects_with(6, 7));
        assert!(block.intersects_with(6, 6));
    }

    #[test]
    fn overlapping_single_line_returns_true() {
        let block = new_empty_block(3, 3);

        assert!(block.intersects_with(1, 3));
        assert!(block.intersects_with(3, 3));
        assert!(block.intersects_with(3, 4));
    }
}

#[cfg(test)]
mod block_intersects_with_any_tests {
    use crate::blocks::test_utils::*;

    #[test]
    fn non_overlapping_returns_false() {
        let block = new_empty_block(3, 4);

        assert!(!block.intersects_with_any(&[(1, 2), (5, 6), (10, 16)]));
    }

    #[test]
    fn overlapping_returns_true() {
        let block = new_empty_block(4, 6);

        // Intersecting range is first.
        assert!(block.intersects_with_any(&[(1, 5), (6, 8), (10, 16)]));
        // Intersecting range is second.
        assert!(block.intersects_with_any(&[(1, 2), (3, 5), (6, 8), (10, 16)]));
        // Intersecting range is last.
        assert!(block.intersects_with_any(&[(1, 1), (2, 3), (4, 8)]));
        // Intersecting range is second from last.
        assert!(block.intersects_with_any(&[(1, 1), (2, 2), (4, 8), (10, 16)]));
        // Multiple intersecting ranges in the beginning.
        assert!(block.intersects_with_any(&[(1, 4), (5, 5), (6, 8), (10, 16)]));
        // Multiple intersecting ranges in the middle.
        assert!(block.intersects_with_any(&[(1, 1), (2, 3), (4, 5), (6, 8), (10, 16)]));
        // Multiple intersecting ranges in the end.
        assert!(block.intersects_with_any(&[(1, 1), (2, 2), (3, 3), (4, 5), (6, 8)]));
    }
}

#[cfg(test)]
mod parse_blocks_tests {
    use crate::blocks::*;
    use crate::parsers::language_parsers;

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

    #[test]
    fn returns_blocks_for_modified_ranges_only() -> anyhow::Result<()> {
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
            ("a.rs".to_string(), vec![(3, 3), (7, 8)]), // Both blocks are modified.
            ("b.rs".to_string(), vec![(5, 6)]),         // The inner block is modified.
            ("c.rs".to_string(), vec![(6, 7)]),         // No block is modified.
        ]);
        let parsers = language_parsers()?;

        let blocks_by_file = parse_blocks(&modified_ranges, &file_reader, parsers, HashMap::new())?;

        assert_eq!(blocks_by_file.len(), 2);
        let blocks_a = &blocks_by_file["a.rs"].blocks;
        assert_eq!(blocks_a.len(), 2);
        assert_eq!(blocks_a[0].name(), Some("first"));
        assert_eq!(blocks_a[1].name(), Some("second"));
        let blocks_b = &blocks_by_file["b.rs"].blocks;
        assert_eq!(blocks_b.len(), 2);
        assert_eq!(blocks_b[0].name(), Some("outer"));
        assert_eq!(blocks_b[1].name(), Some("inner"));
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
            ("a.rs".to_string(), vec![(3, 3), (7, 8)]), // Both blocks are modified.
        ]);
        let parsers = language_parsers()?;

        let blocks_by_file = parse_blocks(&modified_ranges, &file_reader, parsers, HashMap::new())?;

        let content_a = &blocks_by_file["a.rs"].file_contents;
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
        let modified_ranges = HashMap::from([("a.rust".to_string(), vec![(3, 3)])]);
        let parsers = language_parsers()?;

        let blocks_by_file = parse_blocks(
            &modified_ranges,
            &file_reader,
            parsers,
            HashMap::from([("rust".to_string(), "rs".to_string())]),
        )?;

        assert_eq!(blocks_by_file.len(), 1);
        assert_eq!(blocks_by_file["a.rust"].blocks.len(), 1);
        Ok(())
    }

    #[test]
    fn skips_unknown_files() -> anyhow::Result<()> {
        let files = HashMap::from([("test.unknown".to_string(), "test content".to_string())]);
        let modified_ranges = HashMap::from([("test.unknown".to_string(), vec![(1, 2)])]);

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
