use std::collections::HashMap;
use std::ops::Range;
use winnow::Result as PResult;
use winnow::ascii::{multispace0, multispace1, take_escaped};
use winnow::combinator::{alt, delimited, opt, preceded, repeat};
use winnow::prelude::*;
use winnow::token::{any, literal, take_till, take_while};

/// Represents a parsed block tag.
#[derive(Debug)]
pub(crate) enum BlockTag {
    /// A start tag.
    Start {
        /// Position of the start tag in a comment.
        tag_range: Range<usize>,
        /// Attribute name-value pairs (duplicate keys use last value).
        attributes: HashMap<String, String>,
    },
    /// An end tag.
    End {
        /// Byte position where the tag starts in the source
        start_position: usize,
    },
}

/// A start or end tag that was recognized as an attempt (see [`looks_like_tag_attempt`]) but
/// failed to parse as a complete tag, for example, a missing closing `>`.
#[derive(Debug)]
pub(crate) struct MalformedTag {
    /// Byte offset of the offending `<` within the comment text passed to the parser.
    pub(crate) position: usize,
}

/// A winnow-based parser for block tags.
pub(crate) struct WinnowBlockTagParser<'source> {
    source: &'source str,
    cursor: usize,
}

impl<'source> WinnowBlockTagParser<'source> {
    /// Parses `source` starting at byte offset `cursor`, which must be a character boundary in
    /// `source`. Otherwise [`Iterator::next`] panics.
    ///
    /// Callers resume scanning a comment they have already partly consumed by passing back the
    /// [`Self::cursor`] of the previous parser.
    pub(crate) fn new(source: &'source str, cursor: usize) -> Self {
        Self { source, cursor }
    }

    /// The byte offset just past the last tag returned, for resuming the scan.
    pub(crate) fn cursor(&self) -> usize {
        self.cursor
    }

    /// Parses the tag at byte offset `start` of the source, which holds a `<`, and moves the cursor
    /// past it.
    ///
    /// Returns `None` if the text there is not a tag, and an error if it looks like one but does
    /// not parse (see [`looks_like_tag_attempt`]). The cursor moves only when a tag is returned.
    fn parse_tag_at(&mut self, start: usize) -> Result<Option<BlockTag>, MalformedTag> {
        #[expect(clippy::string_slice, reason = "`start` is the index of a `<`")]
        let input = &self.source[start..];
        if let Ok((remaining, attributes)) = parse_start_tag.parse_peek(input) {
            self.cursor = self.source.len() - remaining.len();
            return Ok(Some(BlockTag::Start {
                tag_range: start..self.cursor,
                attributes,
            }));
        }
        if let Ok((remaining, ())) = parse_end_tag.parse_peek(input) {
            self.cursor = self.source.len() - remaining.len();
            return Ok(Some(BlockTag::End {
                start_position: start,
            }));
        }
        if looks_like_tag_attempt(input) {
            return Err(MalformedTag { position: start });
        }
        Ok(None)
    }
}

impl<'source> Iterator for WinnowBlockTagParser<'source> {
    type Item = Result<BlockTag, MalformedTag>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut position = self.cursor;
        loop {
            #[expect(
                clippy::string_slice,
                reason = "`position` is the cursor or right after a `<`"
            )]
            let rest = &self.source[position..];
            let offset = match find_tag_start(rest) {
                Some(offset) => offset,
                None => break,
            };
            let start = position + offset;
            match self.parse_tag_at(start) {
                Ok(Some(tag)) => return Some(Ok(tag)),
                Ok(None) => {
                    // Not a tag, such as `<blockquote>`, so the search goes on past its `<`.
                    position = start + 1;
                }
                Err(malformed) => return Some(Err(malformed)),
            }
        }
        self.cursor = self.source.len();
        None
    }
}

/// The byte offset of the first `<` in `input` that is outside a code span, or `None` if there is
/// no such `<`.
fn find_tag_start(input: &str) -> Option<usize> {
    let mut position = 0;
    loop {
        #[expect(
            clippy::string_slice,
            reason = "`position` is at a `<` or after backticks"
        )]
        let offset = input[position..].find(['<', '`'])?;
        position += offset;
        #[expect(
            clippy::string_slice,
            reason = "`position` is the index of a `<` or a backtick"
        )]
        let rest = &input[position..];
        if rest.starts_with('<') {
            return Some(position);
        }
        // Prose quotes code in backticks, as Markdown does, so a tag in a code span is an example
        // of a tag rather than a real one.
        position += code_span_len(rest);
    }
}

/// The length in bytes of the code span at the start of `input`, which starts with a backtick.
/// The result is at least 1. In a debug build, it panics if `input` does not start with a
/// backtick, because the result would then be 0.
///
/// As in Markdown, a span that starts with N backticks in a row ends at the next N backticks in a
/// row on the same line. More or fewer backticks in a row don't end it. If nothing ends it, the
/// result is N.
///
/// ```text
/// `a` b        -> 3, the span is `a`
/// ``a ` b`` c  -> 9, one backtick does not end a span that starts with two
/// `a b         -> 1, no backtick closes the span on this line
/// ```
fn code_span_len(input: &str) -> usize {
    debug_assert!(input.starts_with('`'));
    // A span ends on its own line, so a stray backtick can't hide a tag on a later line.
    #[expect(clippy::string_slice, reason = "`find` returns the index of a `\\n`")]
    let line = &input[..input.find('\n').unwrap_or(input.len())];
    let fence_len = line.len() - line.trim_start_matches('`').len();
    let mut cursor = fence_len;
    loop {
        #[expect(clippy::string_slice, reason = "`cursor` is right after a backtick")]
        let rest = &line[cursor..];
        let pos = match rest.find('`') {
            Some(pos) => pos,
            None => break,
        };
        #[expect(clippy::string_slice, reason = "`pos` is the index of a backtick")]
        let run = &rest[pos..];
        let run_len = run.len() - run.trim_start_matches('`').len();
        cursor += pos + run_len;
        if run_len == fence_len {
            return cursor;
        }
    }
    fence_len
}

/// Whether `c` can appear in an attribute name or an unquoted attribute value (see
/// [`parse_attribute_name`] and [`parse_attribute_value`]).
fn is_attribute_char(c: char) -> bool {
    c.is_alphanumeric() || c == '-' || c == '_'
}

/// Whether `input`, which starts with `<`, looks like an attempt at a block start or end tag.
///
/// The text right after continues as `block` (an end tag's optional `/` and whitespace aside),
/// followed by whitespace or the end of `input`, as in a real tag. Anything else there means
/// prose: a letter, as in `blockquote` or the generic type parameter `Vec<blockstart>`, or
/// punctuation, as in a comment that puts a comma right after the word.
fn looks_like_tag_attempt(input: &str) -> bool {
    let after_marker = input.strip_prefix("<block").or_else(|| {
        // Not a start-tag attempt; try an end-tag one instead.
        input
            .strip_prefix('<')
            .map(str::trim_start)
            .and_then(|rest| rest.strip_prefix('/'))
            .map(str::trim_start)
            .and_then(|rest| rest.strip_prefix("block"))
    });
    match after_marker {
        Some(rest) => rest.is_empty() || rest.starts_with(char::is_whitespace),
        None => false,
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
    take_while(1.., is_attribute_char)
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
/// In a quoted value, a backslash escapes the character after it, so `"a\"b"` is one value. The
/// value is returned as written, with the backslash: `a\"b`. A value that ends in one backslash
/// has no closing quote, so it fails to parse.
///
/// Note: HTML entities are NOT decoded (e.g., `&quot;` stays as `&quot;`)
fn parse_attribute_value(input: &mut &str) -> PResult<String> {
    // Escapes are not decoded, because most quoted values are regexes. A regex already reads `\"`
    // as a quote, and decoding `\\` would change what an existing pattern matches.
    alt((
        // Double-quoted value
        delimited(
            literal("\""),
            take_escaped(take_till(1.., ['"', '\\']), '\\', any),
            literal("\""),
        ),
        // Single-quoted value
        delimited(
            literal("'"),
            take_escaped(take_till(1.., ['\'', '\\']), '\\', any),
            literal("'"),
        ),
        // Unquoted value (restricted character set)
        take_while(1.., is_attribute_char),
    ))
    .map(|s: &str| s.to_string())
    .parse_next(input)
}
