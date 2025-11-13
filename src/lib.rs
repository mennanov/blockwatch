use serde::Serialize;

pub mod blocks;
pub mod differ;
pub mod flags;
pub mod parsers;
pub mod validators;

#[cfg(test)]
mod test_utils {
    use crate::blocks::{Block, BlockWithContext, FileBlocks};
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

    /// Creates a `BlockWithContext` from a `Block` with default modification flags.
    ///
    /// # Arguments
    /// * `block` - The block to wrap in a context
    ///
    /// # Returns
    /// A `BlockWithContext` with both `is_start_tag_modified` and `is_content_modified` set to
    /// false.
    pub(crate) fn block_with_context_default(block: Block) -> BlockWithContext {
        BlockWithContext {
            block: Arc::new(block),
            _is_start_tag_modified: false,
            is_content_modified: false,
        }
    }

    /// Creates a `BlockWithContext` from a `Block` with specified modification flags.
    ///
    /// # Arguments
    /// * `block` - The block to wrap in a context
    /// * `is_start_tag_modified` - Whether the block's start tag is modified
    /// * `is_content_modified` - Whether the block's content is modified
    ///
    /// # Returns
    /// A `BlockWithContext` with the specified modification flags.
    pub(crate) fn block_with_context(
        block: Block,
        is_start_tag_modified: bool,
        is_content_modified: bool,
    ) -> BlockWithContext {
        BlockWithContext {
            block: Arc::new(block),
            _is_start_tag_modified: is_start_tag_modified,
            is_content_modified,
        }
    }

    /// Creates a `FileBlock` with an empty file contents.
    pub(crate) fn file_blocks_default(blocks_with_context: Vec<BlockWithContext>) -> FileBlocks {
        FileBlocks {
            file_content: "".to_string(),
            file_content_new_lines: vec![],
            blocks_with_context,
        }
    }

    /// Collects the starting byte indices of all newline characters (`\n`) within the input string.
    pub(crate) fn new_line_positions(input: &str) -> Vec<usize> {
        input.match_indices('\n').map(|(idx, _)| idx).collect()
    }
}

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
