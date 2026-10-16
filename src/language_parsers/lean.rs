//! Finds the comments in Lean 4 files with a scanner written with winnow. Other languages use a
//! tree-sitter grammar, but no Lean grammar on crates.io works here:
//!
//! - `tree-sitter-lean4` 0.3.0 depends on `tree-sitter` 0.25 directly. Each `tree-sitter` version
//!   links the same C library (`links = "tree-sitter"`), so Cargo can't build it next to the
//!   newer `tree-sitter` that blockwatch uses.
//! - `arborium-lean` 2.18.2 builds, but it adds about 18 MB to the binary. Its block comments
//!   don't nest, and it reads most of a real Lean 4 file as `ERROR` nodes, because it doesn't
//!   know newer syntax such as `module` and `public import`.
//!
//! Lean's rules for comments are short, so the scanner is small: `--` to the end of the line,
//! nested `/- -/`, and the strings, chars and `«»` names that can hold comment markers.

use crate::LineIndex;
use crate::language_parsers::{Comment, CommentsParser, LanguageParser, LanguageParserImpl};
use std::ops::Range;
use winnow::Result as PResult;
use winnow::combinator::{alt, delimited, eof, opt, repeat_till};
use winnow::prelude::*;
use winnow::stream::{LocatingSlice, Stream};
use winnow::token::{any, one_of, rest, take_till, take_until, take_while};

/// Returns a [`LanguageParser`] for Lean 4.
pub(super) fn parser() -> anyhow::Result<impl LanguageParser> {
    Ok(LanguageParserImpl::new(LeanCommentsParser))
}

/// Finds the comments in Lean 4 source by Lean's own lexical rules.
///
/// A string right after `!`, as in `s!"{x}"` or `println! "{x}"`, is read as an interpolated
/// string: a `{` in it opens Lean code, which can hold comments. Lean also reads the string after
/// some keywords, such as `throwError`, as interpolated. Here, those are plain strings, so a
/// string inside their braces ends the outer string early.
struct LeanCommentsParser;

impl CommentsParser for LeanCommentsParser {
    fn parse<'source>(
        &'source mut self,
        source_code: &'source str,
    ) -> impl Iterator<Item = Comment> + 'source {
        let comments = comments
            .parse(LocatingSlice::new(source_code))
            .expect("each step of `comments` takes at least one character");
        let lines = LineIndex::new(source_code);
        comments
            .into_iter()
            .map(move |((opening, closing), range)| {
                #[expect(clippy::string_slice, reason = "the scanner reads whole characters")]
                let text = &source_code[range.clone()];
                #[expect(clippy::string_slice, reason = "comment markers are ASCII")]
                Comment {
                    position_range: lines.range(&range),
                    comment_text: format!(
                        "{}{}{}",
                        " ".repeat(opening),
                        &text[opening..text.len() - closing],
                        " ".repeat(closing)
                    ),
                    source_range: range,
                }
            })
    }
}

type Input<'source> = LocatingSlice<&'source str>;

/// The lengths in bytes of a comment's opening and closing markers, such as `(2, 2)` for
/// `/- text -/`. The closing length is 0 for a line comment, and for a block comment that the end
/// of the file cuts off.
type MarkerLengths = (usize, usize);

/// Parses the whole input and returns the markers and byte range of each comment, in source order.
fn comments(input: &mut Input<'_>) -> PResult<Vec<(MarkerLengths, Range<usize>)>> {
    // The interpolated strings and their code that the scanner is inside, innermost last. A stack
    // instead of recursion, so that many nested strings can't overflow the stack.
    let mut contexts = Vec::new();
    let mut comments = Vec::new();
    while input.eof_offset() > 0 {
        match contexts.last_mut() {
            Some(Context::String) => match string_piece.parse_next(input)? {
                StringPiece::Text => {}
                StringPiece::OpenBrace => contexts.push(Context::Code { open_braces: 0 }),
                StringPiece::End => {
                    contexts.pop();
                }
            },
            Some(Context::Code { open_braces }) => match code_token.parse_next(input)? {
                CodeToken::Comment(markers, range) => comments.push((markers, range)),
                CodeToken::InterpolatedStringStart => contexts.push(Context::String),
                CodeToken::OpenBrace => *open_braces += 1,
                CodeToken::CloseBrace => match open_braces.checked_sub(1) {
                    Some(still_open) => *open_braces = still_open,
                    None => {
                        contexts.pop();
                    }
                },
                CodeToken::Other => {}
            },
            None => match code_token.parse_next(input)? {
                CodeToken::Comment(markers, range) => comments.push((markers, range)),
                CodeToken::InterpolatedStringStart => contexts.push(Context::String),
                // Outside interpolated strings, braces don't change how the code is read.
                CodeToken::OpenBrace | CodeToken::CloseBrace | CodeToken::Other => {}
            },
        }
    }
    Ok(comments)
}

/// A part of an interpolated string that the scanner is inside.
enum Context {
    /// The text of the string.
    String,
    /// The Lean code between a `{` in the string and its matching `}`, with the count of `{` in
    /// that code that are still open.
    Code { open_braces: usize },
}

/// One token of Lean code.
#[derive(Clone)]
enum CodeToken {
    /// A comment, with its markers and byte range.
    Comment(MarkerLengths, Range<usize>),
    /// The opening quote of an interpolated string, with the `!` before it.
    InterpolatedStringStart,
    OpenBrace,
    CloseBrace,
    /// Code that can't hold a comment, such as a name or a plain string.
    Other,
}

/// Parses one [`CodeToken`]. Fails only at the end of the input.
fn code_token(input: &mut Input<'_>) -> PResult<CodeToken> {
    alt((
        alt((line_comment, block_comment))
            .with_span()
            .map(|(markers, range)| CodeToken::Comment(markers, range)),
        // `s!`, `m!`, `f!` and `println!` take an interpolated string.
        ('!', take_while(0.., [' ', '\t']), '"').value(CodeToken::InterpolatedStringStart),
        '{'.value(CodeToken::OpenBrace),
        '}'.value(CodeToken::CloseBrace),
        alt((
            string,
            // Before `identifier`, which would read the `r` as a name.
            raw_string,
            quoted_name,
            identifier,
            char_literal,
            any.void(),
        ))
        .value(CodeToken::Other),
    ))
    .parse_next(input)
}

/// One piece of the text of an interpolated string.
#[derive(Clone)]
enum StringPiece {
    /// A character, or an escape such as `\{`.
    Text,
    /// The `{` that opens Lean code.
    OpenBrace,
    /// The closing quote.
    End,
}

/// Parses one [`StringPiece`]. Fails only at the end of the input.
fn string_piece(input: &mut Input<'_>) -> PResult<StringPiece> {
    alt((
        '"'.value(StringPiece::End),
        '{'.value(StringPiece::OpenBrace),
        ('\\', any).value(StringPiece::Text),
        any.value(StringPiece::Text),
    ))
    .parse_next(input)
}

/// Parses `--` and the rest of its line.
fn line_comment(input: &mut Input<'_>) -> PResult<MarkerLengths> {
    ("--", take_till(0.., '\n')).value((2, 0)).parse_next(input)
}

/// Parses a block comment, with the comments nested in it. If it is not closed, it runs to the
/// end of the file, as in Lean.
fn block_comment(input: &mut Input<'_>) -> PResult<MarkerLengths> {
    // `/--` opens a doc comment and `/-!` a module doc comment. Lean reads each as one marker,
    // so their third character is not part of the text.
    let opening = alt(("/--", "/-!", "/-")).parse_next(input)?;
    // A loop instead of recursion, so that many nested openers can't overflow the stack.
    let mut depth = 1;
    while depth > 0 {
        match opt(alt(("-/".value(-1), "/-".value(1), any.value(0)))).parse_next(input)? {
            Some(change) => depth += change,
            None => return Ok((opening.len(), 0)),
        }
    }
    Ok((opening.len(), 2))
}

/// Parses a string literal. A backslash escapes the character after it. If the string is not
/// closed, it runs to the end of the file, as in Lean.
fn string(input: &mut Input<'_>) -> PResult<()> {
    let body = repeat_till(
        0..,
        alt((('\\', any).void(), any.void())),
        alt(('"'.void(), eof.void())),
    )
    .map(|((), ())| ());
    ('"', body).void().parse_next(input)
}

/// Parses a raw string literal such as `r"text"` or `r#"text"#`. It has no escapes, and ends at a
/// quote followed by as many `#` as it opens with.
fn raw_string(input: &mut Input<'_>) -> PResult<()> {
    let hashes = delimited('r', take_while(0.., '#'), '"').parse_next(input)?;
    let closing = format!("\"{hashes}");
    alt((
        (take_until(0.., closing.as_str()), closing.as_str()).void(),
        rest.void(),
    ))
    .parse_next(input)
}

/// Parses a name in `«»`, which can hold any character but `»`.
fn quoted_name(input: &mut Input<'_>) -> PResult<()> {
    ('«', take_till(0.., '»'), opt('»'))
        .void()
        .parse_next(input)
}

/// Parses a name, with any primes it ends with, such as `h'`.
fn identifier(input: &mut Input<'_>) -> PResult<()> {
    // The primes are part of the name, so that the prime in `h'` is not read as the opening quote
    // of a char literal.
    (
        one_of(|c: char| c.is_alphabetic() || c == '_'),
        take_while(0.., |c: char| c.is_alphanumeric() || c == '_' || c == '\''),
    )
        .void()
        .parse_next(input)
}

/// Parses a char literal such as `'a'` or `'\''`.
///
/// A longer escape, such as `'\x41'`, doesn't match. Its characters are then read one at a time,
/// and none of them opens a string or a comment.
fn char_literal(input: &mut Input<'_>) -> PResult<()> {
    ('\'', alt((('\\', any).void(), any.void())), '\'')
        .void()
        .parse_next(input)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Position;

    /// The source text of each comment that [`LeanCommentsParser`] finds in `source`.
    fn comment_sources(source: &str) -> Vec<&str> {
        LeanCommentsParser
            .parse(source)
            .map(|comment| &source[comment.source_range])
            .collect()
    }

    #[test]
    fn parses_comments_correctly() {
        let comments: Vec<Comment> = LeanCommentsParser
            .parse(
                r#"
-- line comment
def x : ℕ := 1 -- trailing
/- block comment
   spanning lines -/
/-- doc comment -/
/-! module doc -/
--- triple dash
"#,
            )
            .collect();

        assert_eq!(
            comments,
            vec![
                Comment {
                    position_range: Position::new(2, 1)..Position::new(2, 16),
                    source_range: 1..16,
                    comment_text: "   line comment".to_string()
                },
                Comment {
                    position_range: Position::new(3, 16)..Position::new(3, 27),
                    source_range: 34..45,
                    comment_text: "   trailing".to_string()
                },
                Comment {
                    position_range: Position::new(4, 1)..Position::new(5, 21),
                    source_range: 46..83,
                    comment_text: "   block comment\n   spanning lines   ".to_string()
                },
                Comment {
                    position_range: Position::new(6, 1)..Position::new(6, 19),
                    source_range: 84..102,
                    comment_text: "    doc comment   ".to_string()
                },
                Comment {
                    position_range: Position::new(7, 1)..Position::new(7, 18),
                    source_range: 103..120,
                    comment_text: "    module doc   ".to_string()
                },
                Comment {
                    position_range: Position::new(8, 1)..Position::new(8, 16),
                    source_range: 121..136,
                    comment_text: "  - triple dash".to_string()
                },
            ]
        );
    }

    #[test]
    fn nested_block_comment_is_one_comment() {
        assert_eq!(
            comment_sources("/- outer /- inner -/ still outer -/\n-- next\n"),
            ["/- outer /- inner -/ still outer -/", "-- next"]
        );
    }

    #[test]
    fn block_comment_ending_in_two_dashes_closes() {
        assert_eq!(
            comment_sources("/- note --/\n-- next\n"),
            ["/- note --/", "-- next"]
        );
    }

    #[test]
    fn deeply_nested_block_comment_does_not_overflow_stack() {
        let source = "/-".repeat(100_000);
        assert_eq!(comment_sources(&source), [source.as_str()]);
    }

    #[test]
    fn unterminated_block_comment_runs_to_end_of_file() {
        let source = "-- before\n/- open /- nested -/\n-- inside\n";
        let comments: Vec<Comment> = LeanCommentsParser.parse(source).collect();

        assert_eq!(
            comments,
            vec![
                Comment {
                    position_range: Position::new(1, 1)..Position::new(1, 10),
                    source_range: 0..9,
                    comment_text: "   before".to_string()
                },
                Comment {
                    position_range: Position::new(2, 1)..Position::new(4, 1),
                    source_range: 10..41,
                    comment_text: "   open /- nested -/\n-- inside\n".to_string()
                },
            ]
        );
    }

    #[test]
    fn string_comment_markers_are_ignored() {
        assert_eq!(
            comment_sources(r#"def s := "-- not /- a -/ comment" -- real"#),
            ["-- real"]
        );
    }

    #[test]
    fn string_backslash_escapes_next_character() {
        assert_eq!(
            comment_sources(
                r#"def s := "say \"-- hi\"" -- real
def t := "C:\\" -- also real"#
            ),
            ["-- real", "-- also real"]
        );
    }

    #[test]
    fn unterminated_string_runs_to_end_of_file() {
        assert_eq!(
            comment_sources("-- real\ndef s := \"open -- not\n-- not either\n"),
            ["-- real"]
        );
    }

    #[test]
    fn interpolated_string_inner_string_does_not_end_it() {
        assert_eq!(
            comment_sources(r#"s!"run {if ok then " --fast" else ""} now" -- real"#),
            ["-- real"]
        );
    }

    #[test]
    fn interpolated_string_comment_in_braces_is_found() {
        assert_eq!(
            comment_sources(r#"s!"{x /- note -/}" -- real"#),
            ["/- note -/", "-- real"]
        );
    }

    #[test]
    fn interpolated_string_nested_braces_close_in_order() {
        assert_eq!(
            comment_sources(r#"s!"{ {a := 1}.b ++ "--" }" -- real"#),
            ["-- real"]
        );
    }

    #[test]
    fn interpolated_string_in_braces_is_recognized() {
        assert_eq!(comment_sources(r#"s!"a {s!"--"} c" -- real"#), ["-- real"]);
    }

    #[test]
    fn interpolated_string_after_space_is_recognized() {
        assert_eq!(comment_sources(r#"println! "{"--"}" -- real"#), ["-- real"]);
    }

    #[test]
    fn interpolated_string_escaped_brace_is_text() {
        assert_eq!(comment_sources(r#"s!"\{ -- not" -- real"#), ["-- real"]);
    }

    #[test]
    fn plain_string_braces_are_text() {
        assert_eq!(comment_sources(r#"def s := "{" -- real"#), ["-- real"]);
    }

    #[test]
    fn unterminated_interpolation_runs_to_end_of_file() {
        assert_eq!(
            comment_sources("-- real\ns!\"{x -- inside\n"),
            ["-- real", "-- inside"]
        );
    }

    #[test]
    fn deeply_nested_interpolation_does_not_overflow_stack() {
        let source = format!("{}-- real", "s!\"{".repeat(100_000));
        assert_eq!(comment_sources(&source), ["-- real"]);
    }

    #[test]
    fn raw_string_comment_markers_are_ignored() {
        assert_eq!(
            comment_sources(r###"def s := r#"say "-- hi""# -- real"###),
            ["-- real"]
        );
    }

    #[test]
    fn raw_string_backslash_does_not_escape() {
        assert_eq!(comment_sources(r#"def p := r"C:\" -- real"#), ["-- real"]);
    }

    #[test]
    fn unterminated_raw_string_runs_to_end_of_file() {
        assert_eq!(
            comment_sources("-- real\ndef s := r#\"open \" -- not\n"),
            ["-- real"]
        );
    }

    #[test]
    fn char_literal_quote_does_not_open_string() {
        assert_eq!(comment_sources(r#"#eval ['\'','"'] -- real"#), ["-- real"]);
    }

    #[test]
    fn primed_identifier_does_not_open_char_literal() {
        assert_eq!(comment_sources(r#"#eval f x' '"' -- real"#), ["-- real"]);
    }

    #[test]
    fn quoted_name_comment_markers_are_ignored() {
        assert_eq!(comment_sources("def «a -- b» := 1 -- real"), ["-- real"]);
    }

    #[test]
    fn unterminated_quoted_name_runs_to_end_of_file() {
        assert_eq!(comment_sources("-- real\ndef «open -- not\n"), ["-- real"]);
    }
}
