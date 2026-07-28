use serde::Serialize;

mod block_parser;
pub mod blocks;
pub mod diff_parser;
pub mod flags;
pub mod fs;
pub mod language_parsers;
pub mod repo_path;
mod tag_parser;
pub mod validators;

#[derive(Serialize, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Position {
    // 1-based line number.
    line: usize,
    // 1-based character (column) number.
    character: usize,
}

impl Position {
    pub fn new(line: usize, character: usize) -> Self {
        Self { line, character }
    }
}

#[cfg(test)]
mod test_utils {
    use crate::blocks::{FileBlocks, parse_blocks};
    use crate::diff_parser::LineChange;
    use crate::fs::test_utils::{FakeFileSystem, FakePathChecker};
    use crate::language_parsers;
    use crate::repo_path::RepoPath;
    use crate::validators::ValidationContext;
    use std::collections::HashMap;
    use std::ops::Range;
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
                ranges: None,
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
                line_changes_by_file,
                false,
                &file_system,
                &FakePathChecker::allow_all(),
                &parsers,
                HashMap::new(),
            )
            .unwrap(),
            parsers,
        ))
    }

    pub(crate) fn merge_validation_contexts(
        contexts: Vec<Arc<ValidationContext>>,
    ) -> Arc<ValidationContext> {
        let parsers = contexts
            .first()
            .map(|context| context.parsers.clone())
            .unwrap_or_default();
        let mut merged_modified_blocks = HashMap::new();
        for context in contexts {
            for (file_path, file_blocks) in &context.blocks {
                merged_modified_blocks
                    .entry(file_path.clone())
                    .or_insert_with(|| FileBlocks {
                        file_content: file_blocks.file_content.clone(),
                        blocks_with_context: vec![],
                    })
                    .blocks_with_context
                    .extend(file_blocks.blocks_with_context.clone());
            }
        }
        Arc::new(ValidationContext::new(merged_modified_blocks, parsers))
    }
}
