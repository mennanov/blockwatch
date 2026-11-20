//! Places in a text, as 1-based lines and characters, and how to find them from a byte offset.

use serde::Serialize;
use std::ops::Range;

/// A place in a source file.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct Position {
    /// 1-based line number.
    pub(crate) line: usize,
    /// 1-based character (column) number.
    pub(crate) character: usize,
}

impl Position {
    /// Creates a position from a 1-based `line` and `character`.
    ///
    /// Callers converting from a 0-based source (tree-sitter rows/columns, diff offsets) must add
    /// one first.
    pub(crate) fn new(line: usize, character: usize) -> Self {
        Self { line, character }
    }
}

/// Finds the [`Position`] of a byte offset in a text.
///
/// It finds where the lines start once. Counting the line breaks before each offset would scan
/// the text from its start every time, which is slow for a file with many comments.
#[derive(Debug)]
pub(crate) struct LineIndex<'a> {
    text: &'a str,
    /// The byte offset where each line of `text` starts, in order.
    line_starts: Vec<usize>,
}

impl<'a> LineIndex<'a> {
    pub(crate) fn new(text: &'a str) -> Self {
        let line_starts = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        Self { text, line_starts }
    }

    /// The position of `byte_offset` in the text.
    ///
    /// # Panics
    /// Panics if `byte_offset` is past the end of the text or not on a character boundary.
    pub(crate) fn position(&self, byte_offset: usize) -> Position {
        let line = self
            .line_starts
            .partition_point(|&start| start <= byte_offset);
        Position::new(line, character_column_at(self.text, byte_offset))
    }

    /// The positions of the start and the end of `byte_range`.
    ///
    /// # Panics
    /// Panics if an end of `byte_range` is past the end of the text or not on a character
    /// boundary.
    pub(crate) fn range(&self, byte_range: &Range<usize>) -> Range<Position> {
        self.position(byte_range.start)..self.position(byte_range.end)
    }
}

/// The 1-based character column of `byte_offset` within its line in `text`.
pub(crate) fn character_column_at(text: &str, byte_offset: usize) -> usize {
    #[expect(
        clippy::string_slice,
        reason = "callers give an offset on a character boundary"
    )]
    let line_start = text[..byte_offset].rfind('\n').map_or(0, |i| i + 1);
    #[expect(
        clippy::string_slice,
        reason = "`line_start` is right after a `\\n` or at 0"
    )]
    let before = &text[line_start..byte_offset];
    before.chars().count() + 1
}

#[cfg(test)]
pub(crate) mod test_utils {
    use std::ops::Range;

    /// Finds the byte range of the first occurrence of a substring within a string.
    ///
    /// # Arguments
    /// * `input` - The string to search in
    /// * `substr` - The substring to find
    pub(crate) fn substr_range(input: &str, substr: &str) -> Range<usize> {
        let pos = input.find(substr).unwrap();
        pos..(pos + substr.len())
    }
}
