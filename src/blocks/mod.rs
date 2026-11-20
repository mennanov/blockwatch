//! The `Block` type and the other types that describe the blocks of a file.

mod address;
mod from_comments;
mod tags;
mod virtual_block;

pub(crate) use address::BlockAddress;
pub(crate) use from_comments::{Blocks, BlocksFromCommentsParser};
pub(crate) use virtual_block::{ConfigEntry, Locator, TargetIf, TargetPattern, VirtualBlock};

use crate::attributes::Attributes;
use crate::diff::{self, LineChange, LineChangeKind, LineChanges};
use crate::position::{LineIndex, Position};
use crate::repo_path::RepoPath;
use crate::symbols::Symbol;
use anyhow::anyhow;
use std::collections::HashMap;
use std::fmt;
use std::ops::Range;

const UNNAMED_BLOCK_LABEL: &str = "(unnamed)";

/// A block: a part of a file that its attributes set rules for.
///
/// A pair of tags in the file's comments declares a block. So does a `[[block]]` entry of the
/// config file, around a symbol of the file. That is a virtual block.
#[derive(Debug, PartialEq, Eq, Clone)]
pub(crate) struct Block {
    /// Optional attributes in the `block` tag, or in the `[[block]]` entry of a virtual block.
    /// Their names are what selects the validators that will check this block (`affects`,
    /// `keep-sorted`, …).
    pub(crate) attributes: Attributes,
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
    /// The source text in this byte range: the lines between the block's tags, or the part of the
    /// file that a virtual block wraps when it is written as is, such as a JSON object or a
    /// Markdown section.
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

impl Declaration {
    /// The line of the `[[block]]` entry that declared the block, or `None` when declared by tags.
    pub(crate) fn config_line(&self) -> Option<usize> {
        match self {
            Self::Tags => None,
            Self::ConfigEntry(entry) => Some(entry.line),
        }
    }
}

/// A block and its file, written the way messages refer to the block. [`Block::label`] makes it.
pub(crate) struct BlockLabel<'a> {
    file: &'a RepoPath,
    block: &'a Block,
}

impl fmt::Display for BlockLabel<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{} at {}",
            self.file,
            self.block.name_display(),
            self.block.declared_at()
        )
    }
}

/// What tells a block apart from the other blocks of its file.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct BlockKey {
    /// Where the block starts. Two blocks with tags never start at the same place.
    start: Position,
    /// The config line of a virtual block, or `None` for a block with tags. Two virtual blocks
    /// can start at the same place, such as TOML's `/a` and `/a/b` in `a.b = 1`, but they never
    /// share a config line.
    config_line: Option<usize>,
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

    /// What a reference to `symbol` reads, as the content of a block, with `lines` indexing
    /// `source`. `source` must be the text the symbol was derived from.
    ///
    /// A value written as is, such as a number or a Markdown section, keeps the position of each
    /// of its lines. Any other text, such as a value with quotes or escapes, or the text of a
    /// symbol written in several places, is not a part of the source. Each of its lines is at the
    /// start of the symbol.
    ///
    /// # Panics
    /// Panics if a byte range does not fit `source`.
    pub(crate) fn of_symbol(symbol: &Symbol, source: &str, lines: &LineIndex) -> Self {
        match (&symbol.value, symbol.def_byte_ranges.as_slice()) {
            (Some(value), _) if source.get(value.byte_range.clone()) == Some(&value.text) => {
                Self::source(value.byte_range.clone(), lines.range(&value.byte_range))
            }
            (Some(value), _) => Self::decoded(value.text.clone(), lines.range(&value.byte_range)),
            (None, [definition]) => Self::source(definition.clone(), lines.range(definition)),
            (None, ranges) => {
                let start = ranges.first().expect("a symbol is written somewhere").start;
                let end = ranges.last().expect("a symbol is written somewhere").end;
                Self::decoded(
                    symbol.value_or_definition(source).into_owned(),
                    lines.range(&(start..end)),
                )
            }
        }
    }

    /// Where the content is written in the block's file.
    pub(crate) fn positions(&self) -> &Range<Position> {
        &self.positions
    }

    /// Returns the content's text. `source` must be the text of the block's file.
    pub(crate) fn text<'a>(&'a self, source: &'a str) -> &'a str {
        match &self.text {
            #[expect(
                clippy::string_slice,
                reason = "a comment, a symbol or a match sets `range`"
            )]
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
            ContentText::Decoded(_) => self.positions.start,
        }
    }

    /// Whether any of the `line_changes` touch where the content is written.
    pub(super) fn intersects_any(&self, line_changes: &LineChanges) -> bool {
        line_changes
            .on_lines(self.positions.start.line, self.positions.end.line)
            .any(|line_change| self.intersects(line_change))
    }

    /// Whether `line_change`, which has to fall on one of the lines the content spans, touches
    /// where the content is written.
    fn intersects(&self, line_change: &LineChange) -> bool {
        match &line_change.kind {
            LineChangeKind::Deleted => {
                let point = diff::deletion_point(line_change.line);
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

                diff::intersects_columns(ranges, start_col, end_col)
            }
        }
    }
}

impl Block {
    /// Builds the block that a pair of tags declares, with the start tag at
    /// `start_tag_position_range`.
    ///
    /// # Errors
    /// Returns an error that shows the block's name and line if its attributes are not valid; see
    /// [`Attributes::from_entries`].
    pub(crate) fn from_tags(
        attributes: HashMap<String, String>,
        start_tag_position_range: Range<Position>,
        content: Content,
    ) -> anyhow::Result<Self> {
        let name = attributes
            .get("name")
            .map_or(UNNAMED_BLOCK_LABEL, String::as_str)
            .to_string();
        let attributes = Attributes::from_tag(attributes).map_err(|error| {
            // The tag parser does not know the file, so the message cannot show it.
            anyhow!(
                "block {name} at line {} contains {error:#}",
                start_tag_position_range.start.line
            )
        })?;
        Ok(Self {
            attributes,
            start_tag_position_range,
            content,
            declaration: Declaration::Tags,
        })
    }

    /// Returns the optional value of the `name` attribute for this block.
    pub(crate) fn name(&self) -> Option<&str> {
        self.attributes.name()
    }

    /// Returns the block's name if present, otherwise a human-friendly placeholder label.
    pub(crate) fn name_display(&self) -> &str {
        self.name().unwrap_or(UNNAMED_BLOCK_LABEL)
    }

    /// The address of this block, which is in `file`: `FILE:BLOCK_NAME`. `None` for an unnamed
    /// block.
    pub(crate) fn address(&self, file: &RepoPath) -> Option<BlockAddress> {
        Some(BlockAddress::new(file.clone(), self.name()?.to_string()))
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

    /// How a message refers to this block, which is in `file`: `FILE:NAME at WHERE`, such as
    /// `src/a.rs:limits at line 3`.
    pub(crate) fn label<'a>(&'a self, file: &'a RepoPath) -> BlockLabel<'a> {
        BlockLabel { file, block: self }
    }

    /// What tells the block apart from the other blocks of its file.
    pub(crate) fn key(&self) -> BlockKey {
        BlockKey {
            start: self.start_tag_position_range.start,
            config_line: self.declaration.config_line(),
        }
    }
}

/// Represents a source field with its corresponding modified blocks.
#[derive(Debug)]
pub(crate) struct FileBlocks {
    /// Source file contents.
    pub(crate) file_content: String,
    /// Blocks to be validated.
    pub(crate) blocks_with_context: Vec<BlockWithContext>,
}

impl FileBlocks {
    pub(super) fn is_empty(&self) -> bool {
        self.blocks_with_context.is_empty()
    }
}

/// Represents a block with its corresponding validation context.
#[derive(Debug, Clone)]
pub(crate) struct BlockWithContext {
    /// The block itself, as parsed from the source comment or the config file.
    pub(crate) block: Block,
    /// Whether the block's start tag is modified (computed from the input diff).
    pub(crate) is_start_tag_modified: bool,
    /// Whether the content of the block is modified (computed from the input diff). Validators such
    /// as `affects` fire only for blocks whose content actually changed.
    pub(crate) is_content_modified: bool,
}

#[cfg(test)]
mod block_address_tests {
    use crate::attributes::test_utils::text_attributes;
    use crate::blocks::{Block, Content, Declaration};
    use crate::position::Position;
    use crate::repo_path::RepoPath;

    fn repo_path(path: &str) -> RepoPath {
        RepoPath::from_reference(path).expect("a valid repository path")
    }

    /// Builds a contentless block with `attributes`.
    fn block_with_attributes(attributes: &[(&str, &str)]) -> Block {
        Block {
            attributes: text_attributes(attributes),
            start_tag_position_range: Position::new(0, 0)..Position::new(0, 0),
            content: Content::source(0..0, Position::new(0, 0)..Position::new(0, 0)),
            declaration: Declaration::Tags,
        }
    }

    #[test]
    fn named_block_address_is_its_file_and_name() {
        let block = block_with_attributes(&[("name", "fruits")]);

        let address = block
            .address(&repo_path("a.py"))
            .expect("a named block has an address");

        assert_eq!(address.to_string(), "a.py:fruits");
    }

    #[test]
    fn unnamed_block_has_no_address() {
        let block = block_with_attributes(&[("keep-sorted", "asc")]);

        assert_eq!(block.address(&repo_path("a.py")), None);
    }
}

/// Test helpers for building and inspecting blocks.
#[cfg(test)]
pub(crate) mod test_utils {
    use super::*;

    pub(crate) use super::virtual_block::test_utils::{
        virtual_block, virtual_block_with_target_if,
    };

    /// The 1-based start line of `key`.
    pub(crate) fn block_key_start_line(key: &BlockKey) -> usize {
        key.start.line
    }

    /// Parses each of `addresses`, in order.
    ///
    /// # Panics
    /// Panics if an address is not valid.
    pub(crate) fn block_addresses(addresses: &[&str]) -> Vec<BlockAddress> {
        addresses
            .iter()
            .map(|address| BlockAddress::parse(address).expect("a valid address"))
            .collect()
    }
}
