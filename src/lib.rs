use serde::Serialize;

pub mod blocks;
pub mod differ;
pub mod flags;
pub mod parsers;
pub mod validators;

#[derive(Serialize, Debug, PartialEq)]
struct Position {
    line: usize,
    character: usize,
}

impl Position {
    pub fn new(line: usize, character: usize) -> Self {
        Self { line, character }
    }

    pub fn from_byte_offset(offset: usize, new_line_positions: &[usize]) -> Self {
        let line_idx = new_line_positions
            .binary_search(&offset)
            .unwrap_or_else(|i| i);
        let line = line_idx + 1; // Line number is 1-based.
        let character = if line_idx > 0 {
            offset - new_line_positions[line_idx - 1]
        } else {
            offset
        };
        Self { line, character }
    }
}

#[cfg(test)]
mod test_utils {
    use crate::blocks::{FileBlocks, FileReader, parse_blocks};
    use crate::differ::LineChange;
    use crate::parsers;
    use crate::validators::ValidationContext;
    use std::collections::HashMap;
    use std::ops::Range;
    use std::path::Path;
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

    /// Finds the byte range of the nth occurrence of a substring within a string.
    ///
    /// # Arguments
    /// * `input` - The string to search in
    /// * `substr` - The substring to find
    /// * `nth` - The zero-based index of the occurrence to find
    pub(crate) fn substr_range_nth(input: &str, substr: &str, nth: usize) -> Range<usize> {
        let (pos, _) = input.match_indices(substr).nth(nth).unwrap();
        pos..(pos + substr.len())
    }

    pub(crate) struct FakeFileReader {
        files: HashMap<String, String>,
    }

    impl FakeFileReader {
        pub(crate) fn new(files: HashMap<String, String>) -> Self {
            Self { files }
        }
    }

    impl FileReader for FakeFileReader {
        fn read_to_string(&self, path: &Path) -> anyhow::Result<String> {
            Ok(self.files[&path.display().to_string()].clone())
        }
    }

    /// Creates a [`ValidationContext`] for the given `file_name` with `contents` with all lines modified.
    pub(crate) fn validation_context(file_name: &str, contents: &str) -> Arc<ValidationContext> {
        let line_changes: Vec<LineChange> = contents
            .lines()
            .enumerate()
            .map(|(line, _)| LineChange { line, ranges: None })
            .collect();
        validation_context_with_changes(file_name, contents, line_changes)
    }

    /// Creates a [`ValidationContext`] for the given `file_name` with `contents` and specified `line_changes`.
    pub(crate) fn validation_context_with_changes(
        file_name: &str,
        contents: &str,
        line_changes: Vec<LineChange>,
    ) -> Arc<ValidationContext> {
        let file_reader = FakeFileReader::new(HashMap::from([(
            file_name.to_string(),
            contents.to_string(),
        )]));
        let line_changes_by_file = HashMap::from([(file_name.to_string(), line_changes)]);
        Arc::new(ValidationContext::new(
            parse_blocks(
                &line_changes_by_file,
                &file_reader,
                parsers::language_parsers().unwrap(),
                HashMap::new(),
            )
            .unwrap(),
        ))
    }

    pub(crate) fn merge_validation_contexts(
        contexts: Vec<Arc<ValidationContext>>,
    ) -> Arc<ValidationContext> {
        let mut merged_modified_blocks = HashMap::new();
        for context in contexts {
            for (file_path, file_blocks) in &context.modified_blocks {
                merged_modified_blocks
                    .entry(file_path.clone())
                    .or_insert_with(|| FileBlocks {
                        file_content: file_blocks.file_content.clone(),
                        file_content_new_lines: file_blocks.file_content_new_lines.clone(),
                        blocks_with_context: vec![],
                    })
                    .blocks_with_context
                    .extend(file_blocks.blocks_with_context.clone());
            }
        }
        Arc::new(ValidationContext::new(merged_modified_blocks))
    }
}

#[cfg(test)]
mod position_from_byte_offset_tests {
    use super::*;

    #[test]
    fn with_single_line_returns_correct_position() {
        // A single line file has no new lines.
        let result = Position::from_byte_offset(10, &[]);
        assert_eq!(result.line, 1);
        assert_eq!(result.character, 10);
    }

    #[test]
    fn with_multiple_lines_returns_correct_position_on_first_line() {
        let result = Position::from_byte_offset(10, &[20]);
        assert_eq!(result.line, 1);
        assert_eq!(result.character, 10);
    }

    #[test]
    fn with_multiple_lines_returns_correct_position_on_middle_line() {
        let result = Position::from_byte_offset(25, &[20, 30]);
        assert_eq!(result.line, 2);
        assert_eq!(result.character, 5);
    }

    #[test]
    fn with_multiple_lines_returns_correct_position_on_last_line() {
        let result = Position::from_byte_offset(21, &[20]);
        assert_eq!(result.line, 2);
        assert_eq!(result.character, 1);
    }
}
