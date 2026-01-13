use std::collections::HashMap;
use std::ops::Range;
use winnow::Result as PResult;
use winnow::ascii::{multispace0, multispace1};
use winnow::combinator::{alt, delimited, opt, preceded, repeat};
use winnow::prelude::*;
use winnow::token::{literal, take_till, take_while};

/// Parses block tags from the concatenated comment string.
///
/// The input string is implicitly bound to the implementing type when it's created.
pub(crate) trait BlockTagParser {
    fn next(&mut self) -> anyhow::Result<Option<BlockTag>>;
}

/// Represents a parsed block tag.
#[derive(Debug)]
pub(crate) enum BlockTag {
    /// A start tag like.
    Start {
        /// Position of the start tag in a comment.
        tag_range: Range<usize>,
        /// Attribute name-value pairs (duplicate keys use last value).
        attributes: HashMap<String, String>,
    },
    /// An end tag like.
    End {
        /// Byte position where the tag starts in the source
        start_position: usize,
    },
}

/// A winnow-based parser for block tags.
pub(crate) struct WinnowBlockTagParser<'source> {
    source: &'source str,
    cursor: usize,
}

impl<'source> WinnowBlockTagParser<'source> {
    pub(crate) fn new(source: &'source str) -> Self {
        Self { source, cursor: 0 }
    }
}

impl<'source> BlockTagParser for WinnowBlockTagParser<'source> {
    fn next(&mut self) -> anyhow::Result<Option<BlockTag>> {
        // Check if we've reached the end of input
        if self.cursor >= self.source.len() {
            return Ok(None);
        }

        let input = &self.source[self.cursor..];

        // Search for tags by finding '<' characters and testing if they're followed by valid block tag syntax
        let mut current_input = input;
        let mut offset = 0;

        loop {
            if let Some(pos) = current_input.find("<") {
                offset += pos;
                let potential_tag_start = &current_input[pos..];

                // Try to parse as start tag first
                if let Ok((remaining, attributes)) = parse_start_tag.parse_peek(potential_tag_start)
                {
                    let start_position = self.cursor + offset;
                    let match_len = potential_tag_start.len() - remaining.len();
                    let end_position = start_position + match_len;
                    self.cursor = end_position;
                    return Ok(Some(BlockTag::Start {
                        tag_range: start_position..end_position,
                        attributes,
                    }));
                }

                // Try to parse as end tag
                if let Ok((remaining, _)) = parse_end_tag.parse_peek(potential_tag_start) {
                    let start_position = self.cursor + offset;
                    let match_len = potential_tag_start.len() - remaining.len();
                    let end_position = start_position + match_len;
                    self.cursor = end_position;
                    return Ok(Some(BlockTag::End { start_position }));
                }

                // Not a valid tag, skip past this '<' and continue searching
                current_input = &potential_tag_start[1..];
                offset += 1;
            } else {
                // No more '<' characters found
                self.cursor = self.source.len();
                return Ok(None);
            }
        }
    }
}

/// Parses a block start tag.
///
/// Returns a map of attributes defined in the start tag.
fn parse_start_tag(input: &mut &str) -> PResult<HashMap<String, String>> {
    delimited(
        literal("<block"),
        parse_attributes,
        (multispace0, literal(">")),
    )
    .parse_next(input)
}

/// Parses a block end tag.
fn parse_end_tag(input: &mut &str) -> PResult<()> {
    (
        literal("<"),
        opt(multispace0),
        literal("/"),
        opt(multispace0),
        literal("block"),
        opt(multispace0),
        literal(">"),
    )
        .void()
        .parse_next(input)
}

/// Parses zero or more attributes from a block tag.
///
/// If duplicate attributes are found, the last value wins (HTML/XML semantics).
fn parse_attributes(input: &mut &str) -> PResult<HashMap<String, String>> {
    repeat(
        0..,
        preceded(
            multispace1, // Attributes must be preceded by whitespace
            (
                parse_attribute_name,
                opt(preceded(
                    (multispace0, literal("="), multispace0),
                    parse_attribute_value,
                )),
            ),
        ),
    )
    .fold(
        HashMap::new,
        |mut map: HashMap<String, String>, (key, value): (String, Option<String>)| {
            // Insert will replace any previous value for duplicate keys
            map.insert(key, value.unwrap_or_default());
            map
        },
    )
    .parse_next(input)
}

/// Parses an attribute name.
///
/// Valid characters: alphanumeric, '-', and '_'
/// Examples: `name`, `data-value`, `ng_bind`
fn parse_attribute_name(input: &mut &str) -> PResult<String> {
    take_while(1.., |c: char| c.is_alphanumeric() || c == '-' || c == '_')
        .map(|s: &str| s.to_string())
        .parse_next(input)
}

/// Parses an attribute value.
///
/// Supports three formats:
/// 1. Double-quoted: `"value with spaces"`
/// 2. Single-quoted: `'value with spaces'`
/// 3. Unquoted: `simple-value` (no spaces, alphanumeric + '-' + '_')
///
/// Note: HTML entities are NOT decoded (e.g., `&quot;` stays as `&quot;`)
fn parse_attribute_value(input: &mut &str) -> PResult<String> {
    alt((
        // Double-quoted value
        delimited(literal("\""), take_till(0.., '"'), literal("\"")),
        // Single-quoted value
        delimited(literal("'"), take_till(0.., '\''), literal("'")),
        // Unquoted value (restricted character set)
        take_while(1.., |c: char| c.is_alphanumeric() || c == '-' || c == '_'),
    ))
    .map(|s: &str| s.to_string())
    .parse_next(input)
}
