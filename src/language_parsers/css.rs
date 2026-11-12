//! Finds the comments and the symbols in CSS files with cssparser, the parser in Firefox. It
//! reads a file the way browsers do. `tree-sitter-css` 0.25 has a syntax error in many valid
//! files, such as one with `@container` or `@media (width <= 700px)`. It also reads `/*` in an
//! unquoted `url(...)` as the start of a comment, where browsers read it as part of the URL.

use crate::LineIndex;
use crate::language_parsers::{
    Comment, CommentsParser, LanguageParser, LanguageParserImpl,
    c_style_multiline_comment_processor,
};
use crate::symbol_path::SymbolPath;
use crate::symbols::{ScalarValue, Symbol, Symbols, SymbolsParser};
use cssparser::{
    AtRuleParser, CowRcStr, DeclarationParser, ParseError, Parser, ParserState,
    QualifiedRuleParser, RuleBodyItemParser, RuleBodyParser, SourcePosition, StyleSheetParser,
    Token,
};
use std::collections::HashMap;
use std::ops::Range;

/// Returns a [`LanguageParser`] for CSS.
pub(super) fn parser() -> impl LanguageParser {
    LanguageParserImpl::new(CssCommentsParser).with_symbols(CssSymbolsParser)
}

/// Finds the `/* */` comments in a CSS file, where cssparser finds them. A comment left open at
/// the end of the file ends there, as CSS says.
struct CssCommentsParser;

impl CommentsParser for CssCommentsParser {
    fn parse<'source>(
        &'source mut self,
        source: &'source str,
    ) -> impl Iterator<Item = Comment> + 'source {
        let lines = LineIndex::new(source);
        comment_ranges(source).map(move |range| {
            #[expect(clippy::string_slice, reason = "cssparser cuts at tokens")]
            let text = &source[range.clone()];
            Comment {
                position_range: lines.range(&range),
                comment_text: c_style_multiline_comment_processor(text),
                source_range: range,
            }
        })
    }
}

/// The byte range of each comment in `source`, in order.
fn comment_ranges(source: &str) -> impl Iterator<Item = Range<usize>> {
    // Where `input` starts in `source`.
    let mut offset = 0;
    let mut input = Parser::new(source);
    std::iter::from_fn(move || {
        loop {
            let start = input.position().byte_index();
            match input.next_including_whitespace_and_comments() {
                Ok(Token::Comment(_)) => {
                    return Some(offset + start..offset + input.position().byte_index());
                }
                // A `Parser` skips a block, comments and all, unless the block is read with
                // `parse_nested_block`. That takes a closure, so the walk could not stop at each
                // comment. A token doesn't depend on the blocks around it, so a new `Parser` that
                // starts right after the block's opening token reads the same tokens. It reads each
                // closing token as a token of its own.
                Ok(
                    Token::Function(_)
                    | Token::ParenthesisBlock
                    | Token::SquareBracketBlock
                    | Token::CurlyBracketBlock,
                ) => {
                    offset += input.position().byte_index();
                    #[expect(clippy::string_slice, reason = "cssparser cuts at tokens")]
                    let rest = &source[offset..];
                    input = Parser::new(rest);
                }
                // Any other token holds no comment and opens no block.
                Ok(_) => {}
                // A `Parser` made with `Parser::new` stops only at the end of its input.
                Err(_) => return None,
            }
        }
    })
}

/// A [`SymbolsParser`] for CSS. Each rule and each at-rule with a block is a symbol, at any
/// depth, and so is each declaration in them. A path is the selector or at-rule of each rule
/// from the top down, then a property, such as `/:root/--brand`.
///
/// A part of the file that CSS says to skip, such as `*zoom: 1`, gives no symbol. Each word in it
/// is unreadable instead.
struct CssSymbolsParser;

impl SymbolsParser for CssSymbolsParser {
    /// Never returns an error. CSS says how to skip each part it can't read, so every file has
    /// symbols.
    fn parse(&mut self, source: &str) -> anyhow::Result<Symbols> {
        // Browsers drop a BOM before they read a file, so it is not part of the first selector.
        let text = source.strip_prefix('\u{feff}').unwrap_or(source);
        let mut walk = Walk {
            source: text,
            path: Vec::new(),
            symbols: Vec::new(),
            skipped: Vec::new(),
        };
        let mut input = Parser::new(text);
        let skipped: Vec<&str> = StyleSheetParser::new(&mut input, &mut walk)
            .filter_map(|rule| rule.err().map(|(_, text, _)| text))
            .collect();
        walk.add_skipped(&skipped);
        let skipped = walk.skipped_words();
        // The walk gives byte ranges in `text`, and a symbol's ranges are in `source`.
        let bom = source.len() - text.len();
        let mut symbols = walk.symbols;
        for symbol in &mut symbols {
            for range in &mut symbol.def_byte_ranges {
                *range = range.start + bom..range.end + bom;
            }
            if let Some(value) = &mut symbol.value {
                value.byte_range = value.byte_range.start + bom..value.byte_range.end + bom;
            }
        }
        Ok(Symbols::with_skipped(symbols, skipped))
    }
}

/// What a walk of a CSS file has found so far.
struct Walk<'s> {
    source: &'s str,
    /// The segments from the top of the file to the rule the walk is in.
    path: Vec<String>,
    symbols: Vec<Symbol>,
    /// The byte range of each part that cssparser skipped.
    skipped: Vec<Range<usize>>,
}

impl Walk<'_> {
    /// Adds the symbol of a rule or an at-rule, and the symbols in its block. The rule starts at
    /// `start`, `head` is its selector, or its name and prelude, and `block` holds its block.
    fn add_rule(&mut self, start: usize, head: &str, block: &mut Parser) {
        let index = self.symbols.len();
        self.path.push(segment(head));
        self.symbols.push(Symbol {
            path: SymbolPath::from_segments(self.path.clone()),
            // Set once the block has been read, when its end is known.
            def_byte_ranges: Vec::new(),
            value: None,
        });
        let skipped: Vec<&str> = RuleBodyParser::new(block, self)
            .filter_map(|item| item.err().map(|(_, text, _)| text))
            .collect();
        self.add_skipped(&skipped);
        // `block` stops before the `}`. A block left open at the end of the file has none.
        let mut end = block.position().byte_index();
        if self.source.as_bytes().get(end) == Some(&b'}') {
            end += 1;
        }
        let definition = start..end;
        self.symbols[index].def_byte_ranges = vec![definition];
        self.path.pop();
    }

    /// Adds `skipped`, the parts of the file that cssparser skipped.
    fn add_skipped(&mut self, skipped: &[&str]) {
        // cssparser gives a skipped part as a slice of the file, without its position.
        self.skipped.extend(skipped.iter().map(|text| {
            let start = text.as_ptr().addr() - self.source.as_ptr().addr();
            start..start + text.len()
        }));
    }

    /// Each word in the skipped parts, with the reason to report for it: the position of a part
    /// that holds it.
    fn skipped_words(&self) -> HashMap<String, String> {
        // Most files skip nothing, and the index reads the whole file.
        if self.skipped.is_empty() {
            return HashMap::new();
        }
        let lines = LineIndex::new(self.source);
        let mut words = HashMap::new();
        for range in &self.skipped {
            let position = lines.position(range.start);
            let text = self.source.get(range.clone()).unwrap_or_default();
            for word in words_in(text) {
                words.entry(word.to_string()).or_insert_with(|| {
                    format!(
                        "file has a syntax error at line {}, column {} near {word}, and the path could resolve to the wrong value",
                        position.line, position.character
                    )
                });
            }
        }
        words
    }
}

impl<'i> QualifiedRuleParser<'i> for Walk<'_> {
    /// Where the selector ends, or `None` when no selector can be what it holds.
    type Prelude = Option<SourcePosition>;
    type QualifiedRule = ();
    type Error = ();

    fn parse_prelude(
        &mut self,
        input: &mut Parser<'i>,
    ) -> Result<Option<SourcePosition>, ParseError<()>> {
        let mut valid = true;
        loop {
            match input.next() {
                // At the top of the file, cssparser reads a `;` or a `}` into the selector after
                // it, as CSS says. No selector can hold one, so browsers skip the rule.
                Ok(Token::Semicolon | Token::CloseCurlyBracket) => valid = false,
                // Any other token can be part of a selector.
                Ok(_) => {}
                Err(_) => return Ok(valid.then(|| input.position())),
            }
        }
    }

    fn parse_block(
        &mut self,
        prelude_end: Option<SourcePosition>,
        start: &ParserState,
        input: &mut Parser<'i>,
    ) -> Result<(), ParseError<()>> {
        match prelude_end {
            Some(end) => {
                let head = input.slice(start.position()..end);
                self.add_rule(start.position().byte_index(), head, input);
                Ok(())
            }
            // For an error from `parse_prelude`, cssparser would give only the selector as the
            // skipped text. For one here, the text holds the block too, which it skips as well.
            None => Err(input.new_error_for_next_token()),
        }
    }
}

impl<'i> AtRuleParser<'i> for Walk<'_> {
    /// Where the prelude ends.
    type Prelude = SourcePosition;
    type AtRule = ();
    type Error = ();

    fn parse_prelude(
        &mut self,
        _name: CowRcStr<'i>,
        input: &mut Parser<'i>,
    ) -> Result<SourcePosition, ParseError<()>> {
        while input.next().is_ok() {}
        Ok(input.position())
    }

    fn rule_without_block(&mut self, _: SourcePosition, _: &ParserState) -> Result<(), ()> {
        // An at-rule without a block, such as `@import`, holds nothing to point at.
        Ok(())
    }

    fn parse_block(
        &mut self,
        prelude_end: SourcePosition,
        start: &ParserState,
        input: &mut Parser<'i>,
    ) -> Result<(), ParseError<()>> {
        let head = input.slice(start.position()..prelude_end);
        self.add_rule(start.position().byte_index(), head, input);
        Ok(())
    }
}

impl<'i> DeclarationParser<'i> for Walk<'_> {
    type Declaration = ();
    type Error = ();

    fn parse_value(
        &mut self,
        name: CowRcStr<'i>,
        input: &mut Parser<'i>,
        start: &ParserState,
    ) -> Result<(), ParseError<()>> {
        let value_start = input.position();
        // Each token of the value, with where it is written. White space and comments are not
        // tokens, so the value leaves out those around it.
        let mut tokens = Vec::new();
        loop {
            input.skip_whitespace();
            let token_start = input.position();
            let token = match input.next() {
                // CSS reads `input:hover { }` in a rule as a nested rule, not as the property
                // `input`: a value with a `{}` block is not a declaration. A custom property
                // can hold any value, so it stays one.
                Ok(Token::CurlyBracketBlock) if !name.starts_with("--") => {
                    return Err(input.new_error_for_next_token());
                }
                Ok(token) => token.clone(),
                Err(_) => break,
            };
            // cssparser skips a block, such as the arguments of a function, only when it reads
            // the next token. Reading it here puts the position at the end of the block. Other
            // tokens have no block.
            if matches!(
                token,
                Token::Function(_)
                    | Token::ParenthesisBlock
                    | Token::SquareBracketBlock
                    | Token::CurlyBracketBlock
            ) {
                input.parse_nested_block(|block| {
                    while block.next().is_ok() {}
                    Ok::<(), ParseError<()>>(())
                })?;
            }
            tokens.push((token_start..input.position(), token));
        }
        let value = match tokens.split_last_chunk::<2>() {
            // CSS allows white space after the `!`, and `important` in any case.
            Some((value, [(_, Token::Delim('!')), (_, Token::Ident(word))]))
                if word.eq_ignore_ascii_case("important") =>
            {
                value
            }
            // Any other value has no `!important`.
            _ => tokens.as_slice(),
        };
        let written = match (value.first(), value.last()) {
            (Some((first, _)), Some((last, _))) => first.start..last.end,
            // An empty value is right after the `:`.
            (None, _) | (_, None) => value_start..value_start,
        };
        let text = match value {
            // cssparser decodes a string by CSS's rules.
            [(_, Token::QuotedString(string))] => string.to_string(),
            // Any other value, strings and all, stays as written.
            _ => input.slice(written.clone()).to_string(),
        };
        let byte_range = written.start.byte_index()..written.end.byte_index();
        let definition_end = tokens
            .last()
            .map_or(value_start, |(range, _)| range.end)
            .byte_index();
        let definition = start.position().byte_index()..definition_end;
        self.symbols.push(Symbol {
            path: SymbolPath::from_segments([self.path.as_slice(), &[name.to_string()]].concat()),
            def_byte_ranges: vec![definition],
            value: Some(ScalarValue { text, byte_range }),
        });
        Ok(())
    }
}

impl<'i> RuleBodyItemParser<'i, (), ()> for Walk<'_> {
    fn parse_declarations(&self) -> bool {
        true
    }

    fn parse_qualified(&self) -> bool {
        true
    }
}

/// `head`, a selector or an at-rule as written, as a path segment: without its comments, and
/// with each run of white space as one space. A string keeps its white space. White space is
/// what CSS says it is: a space, a tab, a line break or a form feed.
fn segment(head: &str) -> String {
    let mut segment = String::with_capacity(head.len());
    // A space is added only before the next character, so none is left at the end. A selector
    // never starts with one.
    let mut space = false;
    let mut quote = None;
    let mut chars = head.chars();
    while let Some(char) = chars.next() {
        if quote.is_none()
            && char == '/'
            && let Some(comment) = chars.as_str().strip_prefix('*')
        {
            // cssparser ends a selector at a `{` outside comments, so every comment in it is
            // closed.
            chars = comment
                .split_once("*/")
                .map_or("", |(_, after)| after)
                .chars();
        } else if quote.is_none() && matches!(char, ' ' | '\t' | '\n' | '\r' | '\u{c}') {
            space = true;
        } else {
            if space {
                segment.push(' ');
            }
            space = false;
            segment.push(char);
            match char {
                // An escape takes the character after it, so `\"` doesn't end a string.
                '\\' => segment.extend(chars.next()),
                '"' | '\'' if quote.is_none() => quote = Some(char),
                _ if quote == Some(char) => quote = None,
                _ => {}
            }
        }
    }
    segment
}

/// The words in `text`, which a property could be: runs of letters, digits and `-`.
fn words_in(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !(c.is_alphanumeric() || c == '-'))
        .filter(|word| !word.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Position;

    #[test]
    fn parses_css_comments_correctly() -> anyhow::Result<()> {
        let blocks: Vec<Comment> = CssCommentsParser
            .parse(
                r#"
            body {
                color: black;
            }
    
            /* This is a CSS comment */
            .header {
                /* This is a multi-line
                 * CSS comment that spans
                 * multiple lines
                 */
                font-size: 16px;
            }
            
            /* Another multi-line
               CSS comment with
               different formatting */
            "#,
            )
            .collect();

        assert_eq!(
            blocks,
            vec![
                Comment {
                    position_range: Position::new(6, 13)..Position::new(6, 40),
                    source_range: 81..108,
                    comment_text: "   This is a CSS comment   ".to_string()
                },
                Comment {
                    position_range: Position::new(8, 17)..Position::new(11, 20),
                    source_range: 147..266,
                    comment_text: "   This is a multi-line\n                   CSS comment that spans\n                   multiple lines\n                   "
                        .to_string()
                },
                Comment {
                    position_range: Position::new(15, 13)..Position::new(17, 39),
                    source_range: 339..431,
                    comment_text: "   Another multi-line\n               CSS comment with\n               different formatting   "
                        .to_string()
                },
            ]
        );

        Ok(())
    }

    /// Each comment in `source`, as written.
    fn comments_in(source: &str) -> anyhow::Result<Vec<String>> {
        Ok(CssCommentsParser
            .parse(source)
            .map(|comment| source[comment.source_range].to_string())
            .collect())
    }

    #[test]
    fn comments_in_nested_blocks_parse_returns_each_in_order() -> anyhow::Result<()> {
        let source = r#"
            @media (min-width: /* a */ 1px) {
                .item[data-color /* b */] {
                    width: calc(1px /* c */ + 2px);
                }
                /* d */
            }
            /* e */
        "#;

        assert_eq!(
            comments_in(source)?,
            ["/* a */", "/* b */", "/* c */", "/* d */", "/* e */"]
        );
        Ok(())
    }

    #[test]
    fn comment_in_an_unquoted_url_parse_returns_no_comment() -> anyhow::Result<()> {
        // Browsers read `/*` in an unquoted `url(...)` as part of the URL. Any other function,
        // even one whose name starts with `url`, holds a comment.
        let source = r#"
            .a { background: url(/* a */); }
            .b { background: URL(/* b */); }
            .c { background: urls(/* c */); }
        "#;

        assert_eq!(comments_in(source)?, ["/* c */"]);
        Ok(())
    }

    #[test]
    fn comment_markers_in_a_string_parse_returns_no_comment() -> anyhow::Result<()> {
        let source = r#"
            .a { content: "/* a */"; quotes: '/* b */'; background: url("/* c */"); }
            /* d */
        "#;

        assert_eq!(comments_in(source)?, ["/* d */"]);
        Ok(())
    }

    #[test]
    fn unclosed_comment_at_the_end_parse_returns_it_to_the_end_of_the_file() -> anyhow::Result<()> {
        let source = ".a { color: red; }\n/* open";
        let comments: Vec<Comment> = CssCommentsParser.parse(source).collect();

        let texts: Vec<&str> = comments
            .iter()
            .map(|comment| comment.comment_text.as_str())
            .collect();
        assert_eq!(texts, ["   open"]);
        assert_eq!(comments[0].source_range.end, source.len());
        Ok(())
    }

    #[test]
    fn deeply_nested_blocks_parse_returns_the_comment_inside() -> anyhow::Result<()> {
        // Deeper than the 75 nested blocks that cssparser's `parse_nested_block` reads.
        let depth = 100;
        let source = format!("{}/* deep */{}", ".a{".repeat(depth), "}".repeat(depth));

        assert_eq!(comments_in(&source)?, ["/* deep */"]);
        Ok(())
    }
}

#[cfg(test)]
mod symbols_tests {
    use super::*;
    use crate::symbol_path::SymbolPath;
    use crate::symbols::code::test_utils::{
        symbol, symbols, syntax_error, text_of, unresolved, value, values,
    };
    use anyhow::Context;

    /// The symbols of `source`, a CSS file.
    fn css(source: &str) -> anyhow::Result<Symbols> {
        CssSymbolsParser.parse(source)
    }

    /// The path with these `segments`, written without escapes.
    fn path(segments: &[&str]) -> SymbolPath {
        SymbolPath::from_segments(segments.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn stylesheet_with_every_css_shape_parse_symbols_returns_every_symbol_in_order()
    -> anyhow::Result<()> {
        let source = r#"@import url("base.css");

/* Keep in sync with theme.ts. */
:root {
  /* The main color. */
  --brand: #5097ba;
  --gap: 8px !important;
  --font: "Inter", sans-serif;
}

.dark { --brand: #ffffff; }

@media (max-width: 768px) {
  .card { padding: 4px; }
}

.card {
  display: -webkit-box;
  display: flex;
  content: "a\"b";
  &:hover { color: blue; }
}

h1, h2 { margin: 0 }

@theme { --color-ink: oklch(20% 0.01 40); }

@keyframes spin { to { rotate: 1turn; } }
"#;

        // <block affects="docs/symbols.md:css-paths, src/skill.md#/blockwatch/Symbols/CSS">
        assert_eq!(
            symbols(&css(source)?, source),
            vec![
                symbol("/:root", text_of(source, ":root", "\n}")?, None),
                symbol("/:root/--brand", "--brand: #5097ba", Some("#5097ba")),
                symbol("/:root/--gap", "--gap: 8px !important", Some("8px")),
                symbol(
                    "/:root/--font",
                    r#"--font: "Inter", sans-serif"#,
                    Some(r#""Inter", sans-serif"#)
                ),
                symbol("/.dark", ".dark { --brand: #ffffff; }", None),
                symbol("/.dark/--brand", "--brand: #ffffff", Some("#ffffff")),
                symbol(
                    "/@media (max-width: 768px)",
                    text_of(source, "@media", "}\n}")?,
                    None
                ),
                symbol(
                    "/@media (max-width: 768px)/.card",
                    ".card { padding: 4px; }",
                    None
                ),
                symbol(
                    "/@media (max-width: 768px)/.card/padding",
                    "padding: 4px",
                    Some("4px")
                ),
                symbol("/.card", text_of(source, ".card {\n", "\n}")?, None),
                symbol(
                    "/.card/display",
                    "display: -webkit-box",
                    Some("-webkit-box")
                ),
                symbol("/.card/display", "display: flex", Some("flex")),
                symbol("/.card/content", r#"content: "a\"b""#, Some(r#"a"b"#)),
                symbol("/.card/&:hover", "&:hover { color: blue; }", None),
                symbol("/.card/&:hover/color", "color: blue", Some("blue")),
                symbol("/h1%2C h2", "h1, h2 { margin: 0 }", None),
                symbol("/h1%2C h2/margin", "margin: 0", Some("0")),
                symbol(
                    "/@theme",
                    "@theme { --color-ink: oklch(20% 0.01 40); }",
                    None
                ),
                symbol(
                    "/@theme/--color-ink",
                    "--color-ink: oklch(20% 0.01 40)",
                    Some("oklch(20% 0.01 40)")
                ),
                symbol(
                    "/@keyframes spin",
                    "@keyframes spin { to { rotate: 1turn; } }",
                    None
                ),
                symbol("/@keyframes spin/to", "to { rotate: 1turn; }", None),
                symbol("/@keyframes spin/to/rotate", "rotate: 1turn", Some("1turn")),
            ]
        );
        // </block>
        Ok(())
    }

    #[test]
    fn declaration_with_a_block_parse_symbols_reads_a_nested_rule_except_in_a_custom_property()
    -> anyhow::Result<()> {
        let source = ".field {
  input:disabled ~ & { opacity: 0.5; }
  --mixin: { color: red };
  .form & { margin: 0; }
}";

        assert_eq!(
            symbols(&css(source)?, source),
            vec![
                symbol("/.field", source, None),
                symbol(
                    "/.field/input:disabled ~0 &",
                    "input:disabled ~ & { opacity: 0.5; }",
                    None
                ),
                symbol(
                    "/.field/input:disabled ~0 &/opacity",
                    "opacity: 0.5",
                    Some("0.5")
                ),
                symbol(
                    "/.field/--mixin",
                    "--mixin: { color: red }",
                    Some("{ color: red }")
                ),
                symbol("/.field/.form &", ".form & { margin: 0; }", None),
                symbol("/.field/.form &/margin", "margin: 0", Some("0")),
            ]
        );
        Ok(())
    }

    #[test]
    fn selector_with_comments_and_line_breaks_parse_symbols_joins_its_white_space()
    -> anyhow::Result<()> {
        let source = r#".card  >
  /* the link */ a {}
[title="a\"  b"]  >  b {}
[data-glob="src/*"] {}
@media screen
  and (min-width: 1px) {}
"#;

        let paths: Vec<SymbolPath> = css(source)?
            .symbols
            .into_iter()
            .map(|symbol| symbol.path)
            .collect();
        assert_eq!(
            paths,
            vec![
                path(&[".card > a"]),
                path(&[r#"[title="a\"  b"] > b"#]),
                path(&[r#"[data-glob="src/*"]"#]),
                path(&["@media screen and (min-width: 1px)"]),
            ]
        );

        // CSS white space is only a space, a tab, a line break and a form feed. An NBSP is part
        // of a name.
        let paths: Vec<SymbolPath> = css(".a\u{a0}b c {}\n.d\t\r\n\u{c}e {}")?
            .symbols
            .into_iter()
            .map(|symbol| symbol.path)
            .collect();
        assert_eq!(paths, vec![path(&[".a\u{a0}b c"]), path(&[".d e"])]);
        Ok(())
    }

    #[test]
    fn selector_written_twice_resolve_returns_ambiguous() -> anyhow::Result<()> {
        let source = ".btn { color: red; }
.btn { color: blue; }";
        let symbols = css(source)?;

        assert_eq!(
            unresolved(&symbols, source, "/.btn")?,
            "ambiguous symbol, defined at 1:1, 2:1"
        );
        assert_eq!(
            unresolved(&symbols, source, "/.btn/color")?,
            "ambiguous symbol, defined at 1:8, 2:8"
        );
        Ok(())
    }

    #[test]
    fn skipped_parts_parse_symbols_makes_their_words_unreadable() -> anyhow::Result<()> {
        // A declaration outside a rule, and a stray `}`, join the selector after them. So
        // browsers skip that whole rule too.
        let source = ".btn { *min-height: 1px; display: block; }
gap: 0; .card { padding: 0; }
} .nav { margin: 0; }";
        let symbols = css(source)?;

        assert_eq!(
            unresolved(&symbols, source, "/.btn/min-height")?,
            syntax_error(1, 8, "min-height")
        );
        assert_eq!(
            unresolved(&symbols, source, "/.card/padding")?,
            syntax_error(2, 1, "padding")
        );
        assert_eq!(
            unresolved(&symbols, source, "/.nav/margin")?,
            syntax_error(3, 1, "margin")
        );
        assert!(symbols.resolve(&path(&[".btn", "display"])).is_ok());
        assert_eq!(unresolved(&symbols, source, "/")?, "symbol not found");
        Ok(())
    }

    #[test]
    fn value_with_white_space_around_it_parse_symbols_returns_its_range_without_it()
    -> anyhow::Result<()> {
        let source = ".a { color:   red  ; }";
        let symbols = css(source)?;

        let value = symbols
            .resolve(&path(&[".a", "color"]))
            .map_err(|error| anyhow::anyhow!("{error:?}"))?
            .value
            .clone()
            .context("a declaration has a value")?;
        assert_eq!(&source[value.byte_range], "red");
        Ok(())
    }

    #[test]
    fn important_at_the_end_parse_symbols_leaves_it_out_of_the_value() -> anyhow::Result<()> {
        let source = r#".a {
  a: 1px !important;
  b: red!important;
  c: blue ! IMPORTANT;
  d: x !important y;
  e: "!important";
  f: red !ie;
  g: "x" !important;
}"#;

        assert_eq!(
            symbols(&css(source)?, source),
            vec![
                symbol("/.a", source, None),
                symbol("/.a/a", "a: 1px !important", Some("1px")),
                symbol("/.a/b", "b: red!important", Some("red")),
                symbol("/.a/c", "c: blue ! IMPORTANT", Some("blue")),
                symbol("/.a/d", "d: x !important y", Some("x !important y")),
                symbol("/.a/e", r#"e: "!important""#, Some("!important")),
                symbol("/.a/f", "f: red !ie", Some("red !ie")),
                symbol("/.a/g", r#"g: "x" !important"#, Some("x")),
            ]
        );
        Ok(())
    }

    #[test]
    fn string_value_parse_symbols_decodes_its_escapes() -> anyhow::Result<()> {
        let source = r#".a {
  a: "\41 b";
  b: "\41  b";
  c: "\0000414";
  d: "\"q\\";
  e: "a\
b";
  f: 'it\'s';
  g: "\0";
}"#;

        assert_eq!(
            values(&css(source)?, source),
            vec![
                ("/.a".to_string(), None),
                value("/.a/a", "Ab"),
                value("/.a/b", "A b"),
                value("/.a/c", "A4"),
                value("/.a/d", r#""q\"#),
                value("/.a/e", "ab"),
                value("/.a/f", "it's"),
                value("/.a/g", "\u{fffd}"),
            ]
        );
        Ok(())
    }

    #[test]
    fn value_that_is_not_one_string_parse_symbols_keeps_it_as_written() -> anyhow::Result<()> {
        let source = r#".a {
  content: "a" "b";
  --x:  a  b ;
}"#;

        assert_eq!(
            values(&css(source)?, source),
            vec![
                ("/.a".to_string(), None),
                value("/.a/content", r#""a" "b""#),
                value("/.a/--x", "a  b"),
            ]
        );
        Ok(())
    }

    #[test]
    fn comments_around_a_value_parse_symbols_leaves_them_out() -> anyhow::Result<()> {
        let source = ".a {
  a: red /* note */;
  b: /* note */ red;
  c: red /* note */ !important;
  d: 0 /* top */ 4px;
  e: rgb(0 0 0) /* black */;
  f: [full-start] /* a line name */;
  --g: (min-width: 1px) /* for scripts */;
}";

        assert_eq!(
            symbols(&css(source)?, source),
            vec![
                symbol("/.a", source, None),
                symbol("/.a/a", "a: red", Some("red")),
                symbol("/.a/b", "b: /* note */ red", Some("red")),
                symbol("/.a/c", "c: red /* note */ !important", Some("red")),
                symbol("/.a/d", "d: 0 /* top */ 4px", Some("0 /* top */ 4px")),
                symbol("/.a/e", "e: rgb(0 0 0)", Some("rgb(0 0 0)")),
                symbol("/.a/f", "f: [full-start]", Some("[full-start]")),
                symbol("/.a/--g", "--g: (min-width: 1px)", Some("(min-width: 1px)")),
            ]
        );
        Ok(())
    }

    #[test]
    fn empty_value_parse_symbols_returns_an_empty_value() -> anyhow::Result<()> {
        let source = ".a { --x:; --y: ; --z: !important; }";
        let parsed = css(source)?;

        assert_eq!(
            symbols(&parsed, source),
            vec![
                symbol("/.a", source, None),
                symbol("/.a/--x", "--x:", Some("")),
                symbol("/.a/--y", "--y:", Some("")),
                symbol("/.a/--z", "--z: !important", Some("")),
            ]
        );
        let empty = parsed
            .resolve(&path(&[".a", "--y"]))
            .map_err(|error| anyhow::anyhow!("{error:?}"))?
            .value
            .clone()
            .context("a declaration has a value")?;
        assert_eq!(&source[..empty.byte_range.start], ".a { --x:; --y:");
        Ok(())
    }

    #[test]
    fn file_with_a_bom_parse_symbols_leaves_it_out_of_the_first_selector() -> anyhow::Result<()> {
        let source = "\u{feff}:root { --brand: red; }";
        let parsed = css(source)?;

        assert_eq!(
            symbols(&parsed, source),
            vec![
                symbol("/:root", ":root { --brand: red; }", None),
                symbol("/:root/--brand", "--brand: red", Some("red")),
            ]
        );
        let value = parsed
            .resolve(&path(&[":root", "--brand"]))
            .map_err(|error| anyhow::anyhow!("{error:?}"))?
            .value
            .clone()
            .context("a declaration has a value")?;
        assert_eq!(&source[value.byte_range], "red");
        Ok(())
    }

    #[test]
    fn unclosed_rule_at_the_end_parse_symbols_ends_it_at_the_end_of_the_file() -> anyhow::Result<()>
    {
        let source = ".a { color: red";

        assert_eq!(
            symbols(&css(source)?, source),
            vec![
                symbol("/.a", ".a { color: red", None),
                symbol("/.a/color", "color: red", Some("red")),
            ]
        );
        Ok(())
    }
}
