//! Finds `#` comments and `KEY=VALUE` symbols in dotenv (`.env`) files with `winnow`.

use crate::languages::{
    Comment, CommentsParser, LanguageParser, LanguageParserImpl, blank_line_marker,
};
use crate::position::LineIndex;
use crate::symbols::{ScalarValue, Symbol, SymbolPath, Symbols, SymbolsParser};
use anyhow::bail;
use std::ops::Range;
use winnow::Result as PResult;
use winnow::combinator::{alt, eof, not, opt, peek, repeat};
use winnow::error::ContextError;
use winnow::prelude::*;
use winnow::stream::{LocatingSlice, Location, Stream};
use winnow::token::{any, take_while};

/// Returns a [`LanguageParser`] for dotenv files, with `#` comments and `KEY=VALUE` symbols.
pub(super) fn parser() -> impl LanguageParser {
    LanguageParserImpl::new(DotenvCommentsParser).with_symbols(DotenvSymbolsParser)
}

/// Finds `#` comments in a dotenv file.
struct DotenvCommentsParser;

impl CommentsParser for DotenvCommentsParser {
    fn parse<'source>(
        &'source mut self,
        source: &'source str,
    ) -> impl Iterator<Item = Comment> + 'source {
        let parsed = scan(source, ScanMode::Comments)
            .expect("scanning in comments mode recovers from malformed lines");
        let lines = LineIndex::new(source);
        parsed.comments.into_iter().map(move |range| {
            #[expect(
                clippy::string_slice,
                reason = "winnow's LocatingSlice spans stop on character boundaries"
            )]
            let text = &source[range.clone()];
            Comment {
                position_range: lines.range(&range),
                comment_text: blank_line_marker(text, &["#"])
                    .expect("the scanner starts every comment at `#`"),
                source_range: range,
            }
        })
    }
}

/// Finds `KEY=VALUE` symbols in a dotenv file.
struct DotenvSymbolsParser;

impl SymbolsParser for DotenvSymbolsParser {
    /// Returns every `KEY=VALUE` assignment in document order.
    ///
    /// # Errors
    /// Returns an error if a non-empty, non-comment line is missing `=`, has an invalid key, has
    /// an unterminated quoted value, or has unexpected text after a closing quote.
    fn parse(&mut self, source: &str) -> anyhow::Result<Symbols> {
        let parsed = scan(source, ScanMode::Symbols)?;
        Ok(Symbols::from(parsed.symbols))
    }
}

type Input<'s> = LocatingSlice<&'s str>;

/// Whether [`scan`] recovers past syntax errors (for comments) or fails loudly (for symbols).
#[derive(Clone, Copy)]
enum ScanMode {
    Comments,
    Symbols,
}

/// Quoting style of a dotenv value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Quote {
    Double,
    Single,
    Backtick,
}

impl Quote {
    fn from_char(ch: char) -> Option<Self> {
        match ch {
            '"' => Some(Self::Double),
            '\'' => Some(Self::Single),
            '`' => Some(Self::Backtick),
            _ => None,
        }
    }

    fn delimiter(self) -> char {
        match self {
            Self::Double => '"',
            Self::Single => '\'',
            Self::Backtick => '`',
        }
    }
}

struct ParseError {
    offset: usize,
    message: String,
}

struct ScanResult {
    comments: Vec<Range<usize>>,
    symbols: Vec<Symbol>,
}

/// Scans `source` once, collecting `#` comment ranges and `KEY=VALUE` symbols.
fn scan(source: &str, mode: ScanMode) -> anyhow::Result<ScanResult> {
    let mut input = Input::new(source);
    try_parse(&mut input, '\u{feff}');
    let mut result = ScanResult {
        comments: Vec::new(),
        symbols: Vec::new(),
    };
    while input.eof_offset() > 0 {
        if let Err(err) = parse_line(&mut input, mode, &mut result) {
            match mode {
                ScanMode::Comments => skip_to_eol(&mut input),
                ScanMode::Symbols => {
                    let position = LineIndex::new(source).position(err.offset);
                    bail!(
                        "file is not valid dotenv at line {}, column {}: {}",
                        position.line,
                        position.character,
                        err.message
                    );
                }
            }
        }
        try_parse(&mut input, line_ending);
    }
    Ok(result)
}

/// Parses one logical assignment, comment, or blank line.
///
/// Multiline quoted values consume internal line breaks; the caller consumes the line ending
/// after the entry.
fn parse_line(
    input: &mut Input<'_>,
    mode: ScanMode,
    result: &mut ScanResult,
) -> Result<(), ParseError> {
    horizontal_ws(input);
    if eol_or_eof(input) {
        return Ok(());
    }
    if let Some(span) = try_parse(input, comment) {
        record_comment(mode, result, span);
        return Ok(());
    }

    let def_start = input.current_token_start();
    try_parse(input, export_prefix);

    let (key, key_span) = take_while::<_, _, ContextError>(0.., is_key_char)
        .with_span()
        .parse_next(input)
        .expect("`take_while(0..)` cannot fail");

    let ws_after_key = horizontal_ws(input);
    if try_parse(input, '=').is_none() {
        return Err(missing_equals_error(input, key, &key_span, ws_after_key));
    }
    if key.is_empty() {
        return Err(ParseError {
            offset: key_span.start,
            message: "expected a variable name before `=`".to_string(),
        });
    }

    if let Some((value, def_end)) = read_value(input, mode, result)? {
        result.symbols.push(Symbol {
            path: SymbolPath::from_segments(vec![key.to_string()]),
            #[expect(
                clippy::single_range_in_vec_init,
                reason = "each assignment is written in one range"
            )]
            def_byte_ranges: vec![def_start..def_end],
            value: Some(value),
        });
    }
    Ok(())
}

fn missing_equals_error(
    input: &mut Input<'_>,
    key: &str,
    key_span: &Range<usize>,
    ws_after_key: &str,
) -> ParseError {
    if ws_after_key.is_empty()
        && !eol_or_eof(input)
        && let Some(ch) = try_parse(input, any)
    {
        return ParseError {
            offset: input.previous_token_end() - ch.len_utf8(),
            message: format!("invalid character `{ch}` in variable name"),
        };
    }
    ParseError {
        offset: key_span.end,
        message: format!("expected `=` after variable name `{key}`"),
    }
}

/// Reads a value after `=`, returning `Some` only in [`ScanMode::Symbols`].
fn read_value(
    input: &mut Input<'_>,
    mode: ScanMode,
    result: &mut ScanResult,
) -> Result<Option<(ScalarValue, usize)>, ParseError> {
    let after_eq = input.current_token_start();
    let ws = horizontal_ws(input);
    let after_ws = input.current_token_start();

    if eol_or_eof(input) {
        return Ok(empty_value(mode, after_eq, after_ws));
    }
    if !ws.is_empty()
        && let Some(span) = try_parse(input, comment)
    {
        record_comment(mode, result, span);
        return Ok(empty_value(mode, after_eq, after_ws));
    }

    match try_parse(input, peek_quote) {
        Some(quote) => read_quoted_value(input, mode, quote, result),
        None => Ok(read_unquoted_value(input, mode, result)),
    }
}

fn read_quoted_value(
    input: &mut Input<'_>,
    mode: ScanMode,
    quote: Quote,
    result: &mut ScanResult,
) -> Result<Option<(ScalarValue, usize)>, ParseError> {
    let quote_start = input.current_token_start();
    let mut text = String::new();
    let out = match mode {
        ScanMode::Comments => None,
        ScanMode::Symbols => Some(&mut text),
    };
    let Some(value_end) = read_quoted(input, quote, out) else {
        return Err(ParseError {
            offset: quote_start,
            message: "unterminated quoted value".to_string(),
        });
    };

    horizontal_ws(input);
    if !eol_or_eof(input) {
        if let Some(span) = try_parse(input, comment) {
            record_comment(mode, result, span);
        } else {
            return Err(ParseError {
                offset: input.current_token_start(),
                message: "unexpected text after closing quote".to_string(),
            });
        }
    }
    Ok(match mode {
        ScanMode::Comments => None,
        ScanMode::Symbols => Some((
            ScalarValue {
                text,
                byte_range: quote_start..value_end,
            },
            value_end,
        )),
    })
}

fn read_quoted(input: &mut Input<'_>, quote: Quote, mut out: Option<&mut String>) -> Option<usize> {
    let delimiter = quote.delimiter();
    try_parse(input, delimiter)?;
    while let Some(ch) = try_parse(input, any) {
        if ch == delimiter {
            return Some(input.previous_token_end());
        }
        match quote {
            Quote::Double if ch == '\\' => {
                let (backslash, decoded) = try_parse(input, double_quote_escape)?;
                if let Some(buf) = out.as_deref_mut() {
                    if backslash {
                        buf.push('\\');
                    }
                    buf.push(decoded);
                }
            }
            Quote::Double | Quote::Single | Quote::Backtick => {
                let normalized = if ch == '\r' && try_parse(input, '\n').is_some() {
                    '\n'
                } else {
                    ch
                };
                if let Some(buf) = out.as_deref_mut() {
                    buf.push(normalized);
                }
            }
        }
    }
    None
}

fn double_quote_escape(input: &mut Input<'_>) -> PResult<(bool, char)> {
    alt((
        'n'.value((false, '\n')),
        'r'.value((false, '\r')),
        't'.value((false, '\t')),
        '"'.value((false, '"')),
        '\\'.value((false, '\\')),
        '$'.value((false, '$')),
        "\r\n".value((true, '\n')),
        any.map(|ch| (true, ch)),
    ))
    .parse_next(input)
}

fn read_unquoted_value(
    input: &mut Input<'_>,
    mode: ScanMode,
    result: &mut ScanResult,
) -> Option<(ScalarValue, usize)> {
    let (raw, span) = unquoted_raw(input).expect("unquoted value repetition cannot fail");

    horizontal_ws(input);
    if let Some(comment_span) = try_parse(input, comment) {
        record_comment(mode, result, comment_span);
    }

    match mode {
        ScanMode::Comments => None,
        ScanMode::Symbols => {
            let trimmed = raw.trim_end_matches([' ', '\t']);
            let end = span.start + trimmed.len();
            Some((
                ScalarValue {
                    text: trimmed.to_string(),
                    byte_range: span.start..end,
                },
                end,
            ))
        }
    }
}

fn unquoted_raw<'s>(input: &mut Input<'s>) -> PResult<(&'s str, Range<usize>)> {
    let chunk = alt((
        take_while(1.., |c: char| !matches!(c, ' ' | '\t' | '\r' | '\n')).void(),
        (take_while(1.., [' ', '\t']), not('#')).void(),
        ('\r', not('\n')).void(),
    ));
    repeat::<_, _, (), _, _>(0.., chunk)
        .take()
        .with_span()
        .parse_next(input)
}

fn horizontal_ws<'s>(input: &mut Input<'s>) -> &'s str {
    take_while::<_, _, ContextError>(0.., [' ', '\t'])
        .parse_next(input)
        .expect("`take_while(0..)` cannot fail")
}

fn till_eol<'s>(input: &mut Input<'s>) -> PResult<&'s str> {
    let chunk = alt((
        take_while(1.., |c: char| c != '\r' && c != '\n').void(),
        ('\r', not('\n')).void(),
    ));
    repeat::<_, _, (), _, _>(0.., chunk)
        .take()
        .parse_next(input)
}

fn skip_to_eol(input: &mut Input<'_>) {
    till_eol
        .void()
        .parse_next(input)
        .expect("`till_eol` cannot fail");
}

fn line_ending(input: &mut Input<'_>) -> PResult<()> {
    alt(("\r\n", "\n")).void().parse_next(input)
}

fn eol_or_eof(input: &mut Input<'_>) -> bool {
    try_parse(input, peek(alt((line_ending, eof.void())))).is_some()
}

fn comment(input: &mut Input<'_>) -> PResult<Range<usize>> {
    ('#', till_eol).span().parse_next(input)
}

fn export_prefix(input: &mut Input<'_>) -> PResult<()> {
    (
        "export",
        take_while(1.., [' ', '\t']),
        peek(not(alt(("=".void(), "#".void(), line_ending, eof.void())))),
    )
        .void()
        .parse_next(input)
}

fn peek_quote(input: &mut Input<'_>) -> PResult<Quote> {
    peek(any.verify_map(Quote::from_char)).parse_next(input)
}

fn try_parse<'s, O>(
    input: &mut Input<'s>,
    parser: impl Parser<Input<'s>, O, ContextError>,
) -> Option<O> {
    opt(parser).parse_next(input).expect("`opt` cannot fail")
}

fn record_comment(mode: ScanMode, result: &mut ScanResult, span: Range<usize>) {
    match mode {
        ScanMode::Comments => result.comments.push(span),
        ScanMode::Symbols => {}
    }
}

fn empty_value(mode: ScanMode, after_eq: usize, after_ws: usize) -> Option<(ScalarValue, usize)> {
    match mode {
        ScanMode::Comments => None,
        ScanMode::Symbols => Some((
            ScalarValue {
                text: String::new(),
                byte_range: after_eq..after_eq,
            },
            after_ws,
        )),
    }
}

fn is_key_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::position::Position;

    fn comments_in(source: &str) -> Vec<&str> {
        DotenvCommentsParser
            .parse(source)
            .map(|comment| &source[comment.source_range])
            .collect()
    }

    #[test]
    fn parses_comments_correctly() {
        let comments: Vec<Comment> = DotenvCommentsParser
            .parse(
                r##"# Top comment
PORT=8080 # inline comment
  # Indented comment
## Section heading repeats the marker
EMPTY= # after empty value
QUOTED="a # b" # after quoted value
TIGHT="a"#tight inline comment
"##,
            )
            .collect();

        assert_eq!(
            comments,
            vec![
                Comment {
                    position_range: Position::new(1, 1)..Position::new(1, 14),
                    source_range: 0..13,
                    comment_text: "  Top comment".to_string(),
                },
                Comment {
                    position_range: Position::new(2, 11)..Position::new(2, 27),
                    source_range: 24..40,
                    comment_text: "  inline comment".to_string(),
                },
                Comment {
                    position_range: Position::new(3, 3)..Position::new(3, 21),
                    source_range: 43..61,
                    comment_text: "  Indented comment".to_string(),
                },
                Comment {
                    position_range: Position::new(4, 1)..Position::new(4, 38),
                    source_range: 62..99,
                    comment_text: "   Section heading repeats the marker".to_string(),
                },
                Comment {
                    position_range: Position::new(5, 8)..Position::new(5, 27),
                    source_range: 107..126,
                    comment_text: "  after empty value".to_string(),
                },
                Comment {
                    position_range: Position::new(6, 16)..Position::new(6, 36),
                    source_range: 142..162,
                    comment_text: "  after quoted value".to_string(),
                },
                Comment {
                    position_range: Position::new(7, 10)..Position::new(7, 31),
                    source_range: 172..193,
                    comment_text: " tight inline comment".to_string(),
                },
            ]
        );
    }

    #[test]
    fn hash_inside_quotes_or_without_space_in_unquoted_value_is_not_a_comment() {
        assert_eq!(
            comments_in(
                r#"COLOR=#5097ba
URL=https://example.com#section
DOUBLE="line 1
# not a comment
line 2"
SINGLE='literal # hash'
BACKTICK=`backtick # hash`
"#,
            ),
            Vec::<&str>::new()
        );
    }

    #[test]
    fn comments_survive_bom_crlf_utf8_and_malformed_lines() {
        let source = "\u{feff}A=café # café\r\nBAD LINE\r\n=missing\r\nB=\"x\" extra # ignored\r\n# still read\r\n";
        let comments: Vec<Comment> = DotenvCommentsParser.parse(source).collect();

        assert_eq!(
            comments,
            vec![
                Comment {
                    position_range: Position::new(1, 9)..Position::new(1, 15),
                    source_range: 11..18,
                    comment_text: "  café".to_string(),
                },
                Comment {
                    position_range: Position::new(5, 1)..Position::new(5, 13),
                    source_range: 63..75,
                    comment_text: "  still read".to_string(),
                },
            ]
        );
    }

    #[test]
    fn unterminated_quote_does_not_emit_inner_hash_as_comment() {
        assert_eq!(
            comments_in("BROKEN=\"open\n# inside quote\n"),
            Vec::<&str>::new()
        );
        assert_eq!(comments_in("A=1 # at eof"), ["# at eof"]);
        assert_eq!(comments_in("MALFORMED_AT_EOF"), Vec::<&str>::new());
    }
}

#[cfg(test)]
mod symbols_tests {
    use super::*;
    use crate::languages::code_symbols::test_utils::{
        symbol, symbols, unresolved, values_as_written,
    };

    fn dotenv(source: &str) -> anyhow::Result<Symbols> {
        DotenvSymbolsParser.parse(source)
    }

    #[test]
    fn file_with_every_shape_parse_symbols_returns_every_symbol_in_order() -> anyhow::Result<()> {
        let source = r#"# Database configuration
DATABASE_POOL_LIMIT=10
EMAIL_FROM=Example <noreply@example.com>
export VITE_API_URL=https://api.example.com
COLOR=#5097ba
PORT = 8080 # inline comment
MESSAGE="hello\nworld\t\"\$1\"\\end\d"
SINGLE='literal \n $HOME # hash'
MULTILINE=`line 1
line 2`
EMPTY=
EMPTY_COMMENT= # nothing
EMPTY_QUOTED=""
app.name=blockwatch
my-key=value
export=1
export = 2
export export=3
"#;

        // <block affects="docs/symbols.md:dotenv-paths">
        assert_eq!(
            symbols(&dotenv(source)?, source),
            vec![
                symbol("/DATABASE_POOL_LIMIT", "DATABASE_POOL_LIMIT=10", Some("10")),
                symbol(
                    "/EMAIL_FROM",
                    "EMAIL_FROM=Example <noreply@example.com>",
                    Some("Example <noreply@example.com>"),
                ),
                symbol(
                    "/VITE_API_URL",
                    "export VITE_API_URL=https://api.example.com",
                    Some("https://api.example.com"),
                ),
                symbol("/COLOR", "COLOR=#5097ba", Some("#5097ba")),
                symbol("/PORT", "PORT = 8080", Some("8080")),
                symbol(
                    "/MESSAGE",
                    r#"MESSAGE="hello\nworld\t\"\$1\"\\end\d""#,
                    Some("hello\nworld\t\"$1\"\\end\\d"),
                ),
                symbol(
                    "/SINGLE",
                    r"SINGLE='literal \n $HOME # hash'",
                    Some(r"literal \n $HOME # hash"),
                ),
                symbol(
                    "/MULTILINE",
                    "MULTILINE=`line 1\nline 2`",
                    Some("line 1\nline 2"),
                ),
                symbol("/EMPTY", "EMPTY=", Some("")),
                symbol("/EMPTY_COMMENT", "EMPTY_COMMENT= ", Some("")),
                symbol("/EMPTY_QUOTED", "EMPTY_QUOTED=\"\"", Some("")),
                symbol("/app.name", "app.name=blockwatch", Some("blockwatch")),
                symbol("/my-key", "my-key=value", Some("value")),
                symbol("/export", "export=1", Some("1")),
                symbol("/export", "export = 2", Some("2")),
                symbol("/export", "export export=3", Some("3")),
            ]
        );
        // </block>
        Ok(())
    }

    #[test]
    fn duplicate_key_resolve_returns_ambiguous() -> anyhow::Result<()> {
        let source = "PORT=8080\nPORT=9090\n";
        let parsed = dotenv(source)?;

        assert_eq!(
            unresolved(&parsed, source, "/PORT")?,
            "ambiguous symbol, defined at 1:1, 2:1"
        );
        Ok(())
    }

    #[test]
    fn value_byte_ranges_include_quotes_and_exclude_surrounding_whitespace() -> anyhow::Result<()> {
        let source = "PLAIN=  hello   # comment\nQUOTED=  \"a\\nb\"  # comment\nEMPTY=   \n";
        let parsed = dotenv(source)?;

        assert_eq!(
            values_as_written(&parsed, source),
            vec![
                ("/PLAIN".to_string(), Some("hello")),
                ("/QUOTED".to_string(), Some(r#""a\nb""#)),
                ("/EMPTY".to_string(), Some("")),
            ]
        );
        Ok(())
    }

    #[test]
    fn bom_and_crlf_parse_symbols_preserves_ranges_and_normalizes_multiline_crlf()
    -> anyhow::Result<()> {
        let source = "\u{feff}PLAIN=1\r\nMULTI=\"line1\r\nline2\"\r\nESCAPED=\"a\\rb\\\\\r\nc\"\r\nCONT=\"x\\\r\ny\"\r\n";
        let parsed = dotenv(source)?;

        assert_eq!(
            symbols(&parsed, source),
            vec![
                symbol("/PLAIN", "PLAIN=1", Some("1")),
                symbol("/MULTI", "MULTI=\"line1\r\nline2\"", Some("line1\nline2")),
                symbol("/ESCAPED", "ESCAPED=\"a\\rb\\\\\r\nc\"", Some("a\rb\\\nc"),),
                symbol("/CONT", "CONT=\"x\\\r\ny\"", Some("x\\\ny")),
            ]
        );
        Ok(())
    }

    #[test]
    fn double_quote_cr_escape_and_bare_cr_are_preserved() -> anyhow::Result<()> {
        let source = "A=\"a\\rb\"\nB=\"x\ry\"\nC=\"line\\\rnext\"\n";
        let parsed = dotenv(source)?;

        assert_eq!(
            symbols(&parsed, source),
            vec![
                symbol("/A", r#"A="a\rb""#, Some("a\rb")),
                symbol("/B", "B=\"x\ry\"", Some("x\ry")),
                symbol("/C", "C=\"line\\\rnext\"", Some("line\\\rnext")),
            ]
        );
        Ok(())
    }

    #[test]
    fn empty_and_comment_only_files_parse_symbols_returns_empty() -> anyhow::Result<()> {
        assert_eq!(dotenv("")?.symbols, []);
        assert_eq!(dotenv("\u{feff}\n   \n# just a comment\n")?.symbols, []);
        Ok(())
    }

    #[test]
    fn edge_cases_export_tabs_eof_and_empty_whitespace_parse_correctly() -> anyhow::Result<()> {
        let source = "export\tTAB_EXPORT=1\nexport_foo=2\n1FOO=3\nTAB_COMMENT=x\t# c\nEMPTY_TAB=\t# c\nPORT = \nTIGHT=\"a\"#tight\nAT_EOF=4   ";
        let parsed = dotenv(source)?;

        assert_eq!(
            symbols(&parsed, source),
            vec![
                symbol("/TAB_EXPORT", "export\tTAB_EXPORT=1", Some("1")),
                symbol("/export_foo", "export_foo=2", Some("2")),
                symbol("/1FOO", "1FOO=3", Some("3")),
                symbol("/TAB_COMMENT", "TAB_COMMENT=x", Some("x")),
                symbol("/EMPTY_TAB", "EMPTY_TAB=\t", Some("")),
                symbol("/PORT", "PORT = ", Some("")),
                symbol("/TIGHT", r#"TIGHT="a""#, Some("a")),
                symbol("/AT_EOF", "AT_EOF=4", Some("4")),
            ]
        );

        let eof_no_ws = dotenv("A=1")?;
        assert_eq!(
            symbols(&eof_no_ws, "A=1"),
            vec![symbol("/A", "A=1", Some("1"))]
        );

        let mixed_dup = dotenv("PORT=8080\nexport PORT=9090\n")?;
        assert_eq!(
            unresolved(&mixed_dup, "PORT=8080\nexport PORT=9090\n", "/PORT")?,
            "ambiguous symbol, defined at 1:1, 2:1"
        );
        Ok(())
    }

    #[test]
    fn invalid_syntax_parse_symbols_returns_error_with_position() {
        for (source, expected) in [
            (
                "FOO\n",
                "file is not valid dotenv at line 1, column 4: expected `=` after variable name `FOO`",
            ),
            (
                "export FOO\n",
                "file is not valid dotenv at line 1, column 11: expected `=` after variable name `FOO`",
            ),
            (
                "FOO BAR=1\n",
                "file is not valid dotenv at line 1, column 4: expected `=` after variable name `FOO`",
            ),
            (
                "export\n",
                "file is not valid dotenv at line 1, column 7: expected `=` after variable name `export`",
            ),
            (
                "export # c\n",
                "file is not valid dotenv at line 1, column 7: expected `=` after variable name `export`",
            ),
            (
                "=value\n",
                "file is not valid dotenv at line 1, column 1: expected a variable name before `=`",
            ),
            (
                "\u{feff}=value\n",
                "file is not valid dotenv at line 1, column 2: expected a variable name before `=`",
            ),
            (
                "$FOO=1\n",
                "file is not valid dotenv at line 1, column 1: invalid character `$` in variable name",
            ),
            (
                "é=1\n",
                "file is not valid dotenv at line 1, column 1: invalid character `é` in variable name",
            ),
            (
                "FOO$BAR=1\n",
                "file is not valid dotenv at line 1, column 4: invalid character `$` in variable name",
            ),
            (
                "FOO#BAR=1\n",
                "file is not valid dotenv at line 1, column 4: invalid character `#` in variable name",
            ),
            (
                "KEY=\"unterminated\n",
                "file is not valid dotenv at line 1, column 5: unterminated quoted value",
            ),
            (
                "KEY=\"trailing\\",
                "file is not valid dotenv at line 1, column 5: unterminated quoted value",
            ),
            (
                "KEY='unterminated\n",
                "file is not valid dotenv at line 1, column 5: unterminated quoted value",
            ),
            (
                "KEY=`unterminated\n",
                "file is not valid dotenv at line 1, column 5: unterminated quoted value",
            ),
            (
                "KEY=\"a\" extra\n",
                "file is not valid dotenv at line 1, column 9: unexpected text after closing quote",
            ),
            (
                "KEY=\"café\" extra\n",
                "file is not valid dotenv at line 1, column 12: unexpected text after closing quote",
            ),
            (
                "A=\"x\"\rB=\"y\"\n",
                "file is not valid dotenv at line 1, column 6: unexpected text after closing quote",
            ),
        ] {
            let err = dotenv(source).expect_err(source);
            assert_eq!(err.to_string(), expected, "{source:?}");
        }
    }
}
