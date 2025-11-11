pub mod blocks;
pub mod differ;
pub mod flags;
pub mod parsers;
pub mod validators;

#[cfg(test)]
mod test_utils {
    use crate::blocks::{Block, BlockWithContext};
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
}
