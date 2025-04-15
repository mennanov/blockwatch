use crate::parsers::BlocksParser;
use anyhow::{Context, anyhow};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Represents a `block` tag parsed from the source file comments.
#[derive(Debug, PartialEq, Clone)]
pub(crate) struct Block {
    pub(crate) name: Option<String>,
    // Source line number with the `<block>` tag.
    pub(crate) starts_at: usize,
    // Source line number with the corresponding `</block>` tag.
    pub(crate) ends_at: usize,
    pub(crate) affects: Vec<(Option<String>, String)>,
}

const UNNAMED_BLOCK_LABEL: &str = "(unnamed)";

impl Block {
    /// Whether the `Block` intersects with the given closed-closed interval of `start` and `end`.
    fn intersects_with(&self, start: usize, end: usize) -> bool {
        self.ends_at >= start && end >= self.starts_at
    }

    /// Whether the `Block` intersects with any of the **ordered** `ranges`.
    fn intersects_with_any(&self, ranges: &[(usize, usize)]) -> bool {
        let idx = ranges.binary_search_by(|(start, end)| {
            if self.intersects_with(*start, *end) {
                Ordering::Equal
            } else if *end < self.starts_at {
                Ordering::Less
            } else {
                Ordering::Greater
            }
        });
        idx.is_ok()
    }

    pub(crate) fn name_display(&self) -> &str {
        self.name.as_deref().unwrap_or(UNNAMED_BLOCK_LABEL)
    }
}

#[cfg(test)]
mod block_intersects_with_tests {
    use super::*;

    #[test]
    fn non_overlapping_returns_false() {
        let block = Block {
            name: None,
            starts_at: 3,
            ends_at: 4,
            affects: vec![],
        };

        assert!(!block.intersects_with(1, 2));
        assert!(!block.intersects_with(2, 2));
        assert!(!block.intersects_with(5, 5));
        assert!(!block.intersects_with(5, 6));
    }

    #[test]
    fn non_overlapping_single_line_returns_false() {
        let block = Block {
            name: None,
            starts_at: 3,
            ends_at: 3,
            affects: vec![],
        };

        assert!(!block.intersects_with(1, 2));
        assert!(!block.intersects_with(2, 2));
        assert!(!block.intersects_with(4, 4));
        assert!(!block.intersects_with(4, 5));
    }

    #[test]
    fn overlapping_returns_true() {
        let block = Block {
            name: None,
            starts_at: 3,
            ends_at: 6,
            affects: vec![],
        };

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
        let block = Block {
            name: None,
            starts_at: 3,
            ends_at: 3,
            affects: vec![],
        };

        assert!(block.intersects_with(1, 3));
        assert!(block.intersects_with(3, 3));
        assert!(block.intersects_with(3, 4));
    }
}

#[cfg(test)]
mod block_intersects_with_any_tests {
    use super::*;

    #[test]
    fn non_overlapping_returns_false() {
        let block = Block {
            name: None,
            starts_at: 3,
            ends_at: 4,
            affects: vec![],
        };

        assert!(!block.intersects_with_any(&[(1, 2), (5, 6), (10, 16)]));
    }

    #[test]
    fn overlapping_returns_true() {
        let block = Block {
            name: None,
            starts_at: 4,
            ends_at: 6,
            affects: vec![],
        };

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

pub(crate) trait FileReader {
    /// Reads the entire contents of a file into a string.
    fn read_to_string(&self, path: &Path) -> anyhow::Result<String>;
}

pub(crate) struct FsReader {
    root_path: PathBuf,
}

impl FsReader {
    pub(crate) fn new(root_path: PathBuf) -> Self {
        Self { root_path }
    }
}

impl FileReader for FsReader {
    fn read_to_string(&self, path: &Path) -> anyhow::Result<String> {
        std::fs::read_to_string(self.root_path.join(path))
            .context(format!("Failed to read file \"{}\"", path.display()))
    }
}

/// Represents a referencing block (the one that affects others).
///
/// Used to display meaningful error messages.
struct ReferencingBlock {
    file_path: String,
    name: Option<String>,
    starts_at: usize,
}

impl fmt::Display for ReferencingBlock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{} at line {}",
            self.file_path,
            self.name.as_deref().unwrap_or(UNNAMED_BLOCK_LABEL),
            self.starts_at
        )
    }
}

pub(crate) fn check_blocks<'a>(
    modified_ranges_by_file: impl Iterator<Item = (&'a str, &'a [(usize, usize)])>,
    file_reader: impl FileReader,
    parsers: HashMap<String, Rc<Box<dyn BlocksParser>>>,
    extra_file_extensions: HashMap<String, String>,
) -> anyhow::Result<()> {
    let mut named_modified_blocks = HashSet::new();
    let mut affected_blocks: HashMap<(String, String), Vec<ReferencingBlock>> = HashMap::new();
    for (file_path, modified_ranges) in modified_ranges_by_file {
        let source_code = file_reader.read_to_string(Path::new(&file_path))?;
        if let Some(mut ext) = file_name_extension(file_path) {
            ext = extra_file_extensions
                .get(ext)
                .map(|e| e.as_str())
                .unwrap_or(ext);
            if let Some(parser) = parsers.get(ext) {
                for block in parser
                    .parse(&source_code)
                    .context(format!("Failed to parse file \"{}\"", file_path))?
                {
                    if !block.intersects_with_any(modified_ranges) {
                        // Skip untouched blocks.
                        continue;
                    }
                    if let Some(name) = &block.name {
                        named_modified_blocks.insert((file_path, name.clone()));
                    }
                    for affected_block in block.affects.iter() {
                        let (affected_file_name, affected_name) = affected_block;
                        let affected_file_name = affected_file_name
                            .clone()
                            .unwrap_or_else(|| file_path.to_string());
                        let refs = affected_blocks
                            .entry((affected_file_name, affected_name.clone()))
                            .or_default();
                        refs.push(ReferencingBlock {
                            file_path: file_path.to_string(),
                            name: block.name.clone(),
                            starts_at: block.starts_at,
                        });
                    }
                }
            }
        }
    }
    let mut missing_blocks = Vec::new();
    for ((file_path, name), refs) in affected_blocks {
        if !named_modified_blocks.contains(&(file_path.as_str(), name.clone())) {
            missing_blocks.push(((file_path, name), refs));
        }
    }
    if !missing_blocks.is_empty() {
        return Err(anyhow!(
            "Blocks need to be updated:\n{}",
            missing_blocks
                .iter()
                .map(|((file_path, name), refs)| format!(
                    "{}:{} referenced by {}",
                    file_path,
                    name,
                    refs.iter()
                        .map(|r| format!("{}", r))
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }

    Ok(())
}

fn file_name_extension(file_name: &str) -> Option<&str> {
    file_name.rsplit('.').next()
}

#[cfg(test)]
mod check_blocks_tests {
    use super::*;
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
    fn with_missing_blocks_returns_error() -> anyhow::Result<()> {
        let file_reader = FakeFileReader::new(HashMap::from([
            (
                "main.rs".to_string(),
                r#"
        // <block affects="other.rs:foo">
        fn main() {
          println!("Hello, world!");
        }
        // </block>
        "#
                .to_string(),
            ),
            (
                "other.rs".to_string(),
                r#"
            // <block name="foo">
            fn say() {
              println!("Moo");
            }
            // </block>
            "#
                .to_string(),
            ),
        ]));
        let modified_ranges_by_file = [("main.rs", &[(3usize, 4usize)][..])];
        let parsers = language_parsers()?;

        let error = check_blocks(
            modified_ranges_by_file.into_iter(),
            file_reader,
            parsers,
            HashMap::new(),
        )
        .unwrap_err()
        .to_string();
        assert_eq!(
            "Blocks need to be updated:\nother.rs:foo referenced by main.rs:(unnamed) at line 2",
            error
        );

        Ok(())
    }

    #[test]
    fn without_missing_blocks_returns_ok() -> anyhow::Result<()> {
        let file_reader = FakeFileReader::new(HashMap::from([
            (
                "main.rs".to_string(),
                r#"
        // <block affects="other.rs:foo">
        fn main() {
          println!("Hello, world!");
        }
        // </block>
        "#
                .to_string(),
            ),
            (
                "other.rs".to_string(),
                r#"
            // <block name="foo">
            fn say() {
              println!("Moo");
            }
            // </block>
            "#
                .to_string(),
            ),
        ]));
        let modified_ranges_by_file = [
            ("main.rs", &[(3usize, 4usize)][..]),
            ("other.rs", &[(4usize, 5usize)][..]),
        ];
        let parsers = language_parsers()?;

        let result = check_blocks(
            modified_ranges_by_file.into_iter(),
            file_reader,
            parsers,
            HashMap::new(),
        );
        assert!(result.is_ok());

        Ok(())
    }

    #[test]
    fn blocks_in_same_file_all_modified_returns_ok() -> anyhow::Result<()> {
        let file_reader = FakeFileReader::new(HashMap::from([(
            "main.rs".to_string(),
            r#"
        // <block affects=":foo, :bar">
        fn main() {
          println!("Hello, world!");
        }
        // </block>
        
        // <block name="foo">
        fn foo() {
          println!("Hello, world!");
        }
        // </block>
        
        // <block name="bar">
        fn bar() {
          println!("Hello, world!");
        }
        // </block>
        "#
            .to_string(),
        )]));
        let modified_ranges_by_file = [("main.rs", &[(3usize, 4usize), (9, 10), (15, 16)][..])];
        let parsers = language_parsers()?;

        let result = check_blocks(
            modified_ranges_by_file.into_iter(),
            file_reader,
            parsers,
            HashMap::new(),
        );
        assert!(result.is_ok());

        Ok(())
    }

    #[test]
    fn blocks_in_same_file_some_modified_returns_error() -> anyhow::Result<()> {
        let file_reader = FakeFileReader::new(HashMap::from([(
            "main.rs".to_string(),
            r#"
        // <block affects=":foo, :bar">
        fn main() {
          println!("Hello, world!");
        }
        // </block>
        
        // <block name="foo">
        fn foo() {
          println!("Hello, world!");
        }
        // </block>
        
        // <block name="bar">
        fn bar() {
          println!("Hello, world!");
        }
        // </block>
        "#
            .to_string(),
        )]));

        let modified_ranges_by_file = [("main.rs", &[(3usize, 4usize), (9, 10)][..])];
        let parsers = language_parsers()?;

        let error = check_blocks(
            modified_ranges_by_file.into_iter(),
            file_reader,
            parsers,
            HashMap::new(),
        )
        .unwrap_err()
        .to_string();
        assert_eq!(
            "Blocks need to be updated:\nmain.rs:bar referenced by main.rs:(unnamed) at line 2",
            error
        );

        Ok(())
    }

    #[test]
    fn blocks_referencing_each_other_modified_returns_ok() -> anyhow::Result<()> {
        let file_reader = FakeFileReader::new(HashMap::from([
            (
                "a.rs".to_string(),
                r#"
        // <block name="foo" affects="b.rs:bar">
        fn a() {}
        // </block>
        "#
                .to_string(),
            ),
            (
                "b.rs".to_string(),
                r#"
        // <block name="bar" affects="a.rs:foo">
        fn b() {}
        // </block>
        "#
                .to_string(),
            ),
        ]));
        let modified_ranges_by_file = [
            ("a.rs", &[(3usize, 3usize)][..]),
            ("b.rs", &[(3usize, 4usize)][..]),
        ];
        let parsers = language_parsers()?;

        let result = check_blocks(
            modified_ranges_by_file.into_iter(),
            file_reader,
            parsers,
            HashMap::new(),
        );
        assert!(result.is_ok());

        Ok(())
    }

    #[test]
    fn block_with_multiple_references_modified_only_returns_error() -> anyhow::Result<()> {
        let file_reader = FakeFileReader::new(HashMap::from([
            (
                "a.rs".to_string(),
                r#"
        // <block affects="c.rs:target">
        fn a() {}
        // </block>
        "#
                .to_string(),
            ),
            (
                "b.rs".to_string(),
                r#"
        // <block name="second" affects="c.rs:target">
        fn b() {}
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
        "#
                .to_string(),
            ),
        ]));
        let modified_ranges_by_file = [
            ("a.rs", &[(3usize, 3usize)][..]),
            ("b.rs", &[(3usize, 3usize)][..]),
        ];
        let parsers = language_parsers()?;

        let error = check_blocks(
            modified_ranges_by_file.into_iter(),
            file_reader,
            parsers,
            HashMap::new(),
        )
        .unwrap_err()
        .to_string();
        assert_eq!(
            "Blocks need to be updated:\nc.rs:target referenced by a.rs:(unnamed) at line 2, b.rs:second at line 2",
            error
        );

        Ok(())
    }

    #[test]
    fn block_with_multiple_references_modified_only_returns_ok() -> anyhow::Result<()> {
        let file_reader = FakeFileReader::new(HashMap::from([
            (
                "a.rs".to_string(),
                r#"
        // <block affects="c.rs:target">
        fn a() {}
        // </block>
        "#
                .to_string(),
            ),
            (
                "b.rs".to_string(),
                r#"
        // <block name="second" affects="c.rs:target">
        fn b() {}
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
        "#
                .to_string(),
            ),
        ]));
        let modified_ranges_by_file = [
            ("a.rs", &[(3usize, 3usize)][..]),
            ("b.rs", &[(3usize, 3usize)][..]),
            ("c.rs", &[(3usize, 3usize)][..]),
        ];
        let parsers = language_parsers()?;

        let result = check_blocks(
            modified_ranges_by_file.into_iter(),
            file_reader,
            parsers,
            HashMap::new(),
        );

        assert!(result.is_ok());
        Ok(())
    }

    #[test]
    fn with_extra_file_extensions_returns_ok() -> anyhow::Result<()> {
        let file_reader = FakeFileReader::new(HashMap::from([
            (
                "a.custom-rust-extension1".to_string(),
                r#"
        // <block affects="b.custom-rust-extension2:foo">
        fn a() {}
        // </block>
        "#
                .to_string(),
            ),
            (
                "b.custom-rust-extension2".to_string(),
                r#"
        // <block name="foo">
        fn b() {}
        // </block>
        "#
                .to_string(),
            ),
        ]));
        let modified_ranges_by_file = [
            ("a.custom-rust-extension1", &[(3usize, 3usize)][..]),
            ("b.custom-rust-extension2", &[(3usize, 3usize)][..]),
        ];
        let parsers = language_parsers()?;

        let result = check_blocks(
            modified_ranges_by_file.into_iter(),
            file_reader,
            parsers,
            HashMap::from([
                ("custom-rust-extension1".into(), "rs".into()),
                ("custom-rust-extension2".into(), "rs".into()),
            ]),
        );

        assert!(result.is_ok());
        Ok(())
    }

    #[test]
    fn empty_input_returns_ok() -> anyhow::Result<()> {
        let file_reader = FakeFileReader::new(HashMap::new());
        let modified_ranges_by_file: [(&str, &[(usize, usize)]); 0] = [];
        let parsers = language_parsers()?;

        let result = check_blocks(
            modified_ranges_by_file.into_iter(),
            file_reader,
            parsers,
            HashMap::new(),
        );
        assert!(result.is_ok());

        Ok(())
    }
}
