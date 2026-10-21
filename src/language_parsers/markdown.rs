use crate::LineIndex;
use crate::language_parsers::{
    Comment, CommentsParser, LanguageParser, LanguageParserImpl, TreeSitterCommentsParser,
    blank_preserving_line_breaks, xml_style_comments_parser,
};
use crate::symbol_path::SymbolPath;
use crate::symbols::{ScalarValue, Symbol, SymbolsParser};
use markdown::mdast::{Definition, Node};
use markdown::{Constructs, ParseOptions};
use std::ops::Range;

/// Returns a [`LanguageParser`] for Markdown. Its symbols are the sections of a file.
pub(super) fn parser() -> anyhow::Result<impl LanguageParser> {
    Ok(LanguageParserImpl::new(MdCommentsParser::new()?).with_symbols(SectionsParser))
}

/// A [`SymbolsParser`] whose symbols are the sections of a Markdown file, in the order of their
/// headings.
///
/// A section is a heading and everything up to the next heading of the same or a higher level, so
/// it holds its subsections. For each section:
/// - Its path is the text of each heading it is under, the top title included, then its own. The
///   text is what a reader sees: ``## The `x` flag`` is `The x flag`.
/// - Its definition runs from its heading to the end of its last line of text.
/// - Its value is the same text without the heading. It is empty when the heading is all there is.
///
/// Only a heading at the top level of the file starts a section. One in a quote, a list item or an
/// HTML block does not.
struct SectionsParser;

impl SymbolsParser for SectionsParser {
    fn parse(&mut self, source: &str) -> anyhow::Result<Vec<Symbol>> {
        Ok(section_symbols(&parse_markdown(source), source))
    }
}

/// Parses `source` as GitHub-flavored Markdown that can start with frontmatter.
fn parse_markdown(source: &str) -> Node {
    let options = ParseOptions {
        constructs: Constructs {
            // Without it, the `---` that closes the front matter makes the line above it a
            // heading.
            frontmatter: true,
            ..Constructs::gfm()
        },
        ..ParseOptions::default()
    };
    markdown::to_mdast(source, &options)
        .expect("only MDX has syntax errors, and these options leave it off")
}

/// Every node in `root`, `root` included, in source order: each node comes before its children.
pub(super) fn nodes_in_order(root: &Node) -> impl Iterator<Item = &Node> {
    // A stack instead of recursion, so that deeply nested block quotes or lists can't overflow the
    // call stack.
    let mut nodes = vec![root];
    std::iter::from_fn(move || {
        let node = nodes.pop()?;
        if let Some(children) = node.children() {
            nodes.extend(children.iter().rev());
        }
        Some(node)
    })
}

/// The byte range of a node at `position` in the source that markdown-rs parsed.
fn byte_range(position: &markdown::unist::Position) -> Range<usize> {
    position.start.offset..position.end.offset
}

/// A section whose heading has been read, but not the heading that ends it.
struct OpenSection {
    /// The level of its heading, from 1 for `#` to 6 for `######`.
    depth: u8,
    /// The text of each heading it is under, then of its own.
    path: Vec<String>,
    /// The byte range of its heading.
    heading: Range<usize>,
    /// Where the first node after its heading starts, or `None` before that node is read.
    body_start: Option<usize>,
}

impl OpenSection {
    /// The symbol of this section, which ends at `end` in `source`.
    fn into_symbol(self, end: usize, source: &str) -> Symbol {
        let body = match self.body_start {
            Some(start) => start..end,
            None => self.heading.end..self.heading.end,
        };
        #[expect(
            clippy::string_slice,
            reason = "markdown-rs gives offsets of whole characters"
        )]
        let text = source[body.clone()].to_string();
        let definition = self.heading.start..end;
        Symbol {
            path: SymbolPath::from_segments(self.path),
            def_byte_ranges: vec![definition],
            value: Some(ScalarValue {
                text,
                byte_range: body,
            }),
        }
    }
}

/// The symbols of the sections in `root`, the syntax tree of `source`, in the order of their
/// headings. [`SectionsParser`] describes them.
pub(super) fn section_symbols(root: &Node, source: &str) -> Vec<Symbol> {
    let mut sections = Sections::new(source);
    for node in root.children().map_or(&[][..], Vec::as_slice) {
        sections.read(node);
    }
    sections.into_symbols()
}

/// The sections of a Markdown file, found by reading the top-level nodes of its syntax tree in
/// order.
struct Sections<'s> {
    /// The text of the file.
    source: &'s str,
    /// The symbols of the sections that have ended, in the order they ended.
    symbols: Vec<Symbol>,
    /// The sections that the next node is in, the outermost first.
    open: Vec<OpenSection>,
    /// Where the last node read ends.
    end: usize,
}

impl<'s> Sections<'s> {
    fn new(source: &'s str) -> Self {
        Self {
            source,
            symbols: Vec::new(),
            open: Vec::new(),
            end: 0,
        }
    }

    /// Reads `node`, the next top-level node of the file.
    fn read(&mut self, node: &Node) {
        let range = byte_range(
            node.position()
                .expect("markdown-rs gives every node it parses a position"),
        );
        match heading_depth(node) {
            Some(depth) => {
                self.close(depth);
                // A heading one level down is the first node of its parent's body too.
                self.start_body(range.start);
                self.open(depth, node.to_string(), range.clone());
            }
            None => self.start_body(range.start),
        }
        self.end = range.end;
    }

    /// Ends each open section whose heading is at `depth` or deeper.
    fn close(&mut self, depth: u8) {
        // A section ends where the last node before the heading that ends it ends, so the blank
        // lines before that heading are not part of it.
        while let Some(section) = self.open.pop_if(|section| section.depth >= depth) {
            self.symbols
                .push(section.into_symbol(self.end, self.source));
        }
    }

    /// Starts the body of the innermost open section at `start`, unless it has started already.
    fn start_body(&mut self, start: usize) {
        if let Some(section) = self.open.last_mut()
            && section.body_start.is_none()
        {
            section.body_start = Some(start);
        }
    }

    /// Opens a section inside the innermost open one. Its heading, at `depth`, reads `text` and is
    /// written at `heading`.
    fn open(&mut self, depth: u8, text: String, heading: Range<usize>) {
        let mut path = self
            .open
            .last()
            .map_or_else(Vec::new, |parent| parent.path.clone());
        path.push(text);
        self.open.push(OpenSection {
            depth,
            path,
            heading,
            body_start: None,
        });
    }

    /// The symbols of every section, in the order of their headings.
    fn into_symbols(mut self) -> Vec<Symbol> {
        // `#` is the highest level, so this ends every section.
        self.close(1);
        // A section's symbol is made when the section ends, so a subsection's comes first.
        self.symbols
            .sort_by_key(|symbol| symbol.def_byte_ranges[0].start);
        self.symbols
    }
}

/// The level of `node` if it is a heading, from 1 for `#` to 6 for `######`. `None` for any other
/// node.
fn heading_depth(node: &Node) -> Option<u8> {
    match node {
        Node::Heading(heading) => Some(heading.depth),
        // Both kinds of Markdown heading, `#` and underlined, are `Node::Heading`, so a new kind
        // of node would not be a heading either.
        _ => None,
    }
}

/// Finds the `[//]:` comments and the HTML comments in Markdown.
///
/// An HTML comment is read only where Markdown keeps HTML as it is: in an
/// [HTML block](https://spec.commonmark.org/0.31.2/#html-blocks), or in
/// [raw HTML](https://spec.commonmark.org/0.31.2/#raw-html) in a line of text. So a `<!--` in code
/// is not a comment.
struct MdCommentsParser {
    html_comments_parser: TreeSitterCommentsParser,
}

impl MdCommentsParser {
    fn new() -> anyhow::Result<Self> {
        let html_lang = tree_sitter_html::LANGUAGE.into();
        Ok(Self {
            html_comments_parser: xml_style_comments_parser(&html_lang, "comment")?,
        })
    }

    /// Returns the comments in `source`, in source order.
    fn comments(&mut self, source: &str) -> Vec<Comment> {
        // Most Markdown files hold no comment, and markdown-rs is slow on some text, such as a
        // long paragraph with many `<?`.
        if !source.contains("<!--") && !source.contains("[//]:") {
            return Vec::new();
        }
        let root = parse_markdown(source);
        let lines = LineIndex::new(source);
        let mut comments = Vec::new();
        for node in nodes_in_order(&root) {
            match node {
                Node::Html(_) => {
                    comments.extend(
                        self.html_comments(
                            byte_range(
                                node.position()
                                    .expect("markdown-rs gives every node it parses a position"),
                            ),
                            source,
                            &lines,
                        ),
                    );
                }
                Node::Definition(definition) => {
                    comments.extend(link_reference_comment(definition, source, &lines));
                }
                // Raw HTML that Markdown keeps as HTML is always `Node::Html`, and every link
                // reference definition is `Node::Definition`. A `<!--` in the alt text of an image
                // or in the title of a link is part of an attribute, not a comment.
                _ => {}
            }
        }
        comments
    }

    /// Returns the HTML comments in `html`, the byte range of an HTML block or of raw HTML in
    /// `source`, in source order.
    fn html_comments(
        &mut self,
        html: Range<usize>,
        source: &str,
        lines: &LineIndex,
    ) -> Vec<Comment> {
        #[expect(
            clippy::string_slice,
            reason = "markdown-rs gives offsets of whole characters"
        )]
        let region = &source[html.clone()];
        // Most raw HTML is a tag such as `<br>`, which can't hold a comment, so it isn't parsed.
        if !region.contains("<!--") {
            return Vec::new();
        }
        let start_row = lines.position(html.start).line - 1;
        self.html_comments_parser
            .parse(region)
            .map(|mut comment| {
                comment.shift_into_source(start_row, html.start, source);
                comment
            })
            .collect()
    }
}

impl CommentsParser for MdCommentsParser {
    fn parse<'source>(
        &'source mut self,
        contents: &'source str,
    ) -> impl Iterator<Item = Comment> + 'source {
        self.comments(contents).into_iter()
    }
}

/// Returns the comment in `definition`, a link reference definition in `source`: its text up to
/// the end of its line, with everything before the title blanked, so the length is kept. Returns
/// `None` when `definition` is not a `[//]:` comment, or has no title.
fn link_reference_comment(
    definition: &Definition,
    source: &str,
    lines: &LineIndex,
) -> Option<Comment> {
    if definition.label.as_deref() != Some("//") || definition.title.is_none() {
        return None;
    }
    let definition_range = byte_range(
        definition
            .position
            .as_ref()
            .expect("markdown-rs gives every node it parses a position"),
    );
    // Nothing else can be on the line of a definition, so the comment takes the line break too,
    // and a block's content starts on the next line.
    let range = definition_range.start..line_end(source.as_bytes(), definition_range.end);
    let mut text = source.as_bytes()[range.clone()].to_vec();
    let title_start = title_start(&text[..definition_range.len()]);
    blank_preserving_line_breaks(&mut text[..title_start]);
    Some(Comment {
        position_range: lines.range(&range),
        source_range: range,
        comment_text: String::from_utf8(text).expect("only whole characters are blanked"),
    })
}

/// The byte offset right after the first line break in `source` at or after `offset`, or the end
/// of `source` when no line break follows. As in CommonMark, a line break is `\r\n`, `\n` or `\r`.
fn line_end(source: &[u8], offset: usize) -> usize {
    let rest = &source[offset..];
    let length = match rest.iter().position(|&byte| byte == b'\n' || byte == b'\r') {
        Some(line_break) if rest[line_break..].starts_with(b"\r\n") => line_break + 2,
        Some(line_break) => line_break + 1,
        None => rest.len(),
    };
    offset + length
}

/// The byte offset of the opening delimiter of the title in `definition`, the text of a link
/// reference definition that ends with a title, such as `(text)`, `"text"` or `'text'`.
///
/// # Panics
/// Panics if `definition` doesn't end with a title.
fn title_start(definition: &[u8]) -> usize {
    // markdown-rs gives the title decoded, so where it is in the source has to be found here.
    let close = definition.len() - 1;
    let open_delimiter = match definition[close] {
        b')' => b'(',
        quote => quote,
    };
    // The title can hold its opening delimiter only after a backslash, as in `(a\(b)`.
    definition[..close]
        .windows(2)
        .rposition(|pair| pair[1] == open_delimiter && pair[0] != b'\\')
        .expect("a title starts with its opening delimiter")
        + 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks::{Block, BlockSeverity, Content, Declaration};
    use crate::{Position, test_utils};
    use std::collections::HashMap;

    /// The symbols `source` derives, as `(path, value)` pairs in the order they were derived.
    fn derived_symbols(source: &str) -> anyhow::Result<Vec<(SymbolPath, String)>> {
        Ok(parser()?
            .parse_symbols(source)?
            .into_iter()
            .map(|symbol| {
                let value = symbol.value.expect("every section has a value").text;
                (symbol.path, value)
            })
            .collect())
    }

    /// The same pairs, with each path parsed as a reference writes it, so a space is a space.
    fn expected_symbols(symbols: &[(&str, &str)]) -> anyhow::Result<Vec<(SymbolPath, String)>> {
        symbols
            .iter()
            .map(|(path, value)| Ok((SymbolPath::parse(path)?, value.to_string())))
            .collect()
    }

    /// The `name` of each block in `content`, in the order of their start tags.
    fn block_names(content: &str) -> anyhow::Result<Vec<String>> {
        let mut parser = parser()?;
        parser
            .parse_blocks(content)
            .map(|block| Ok(block?.name_display().to_string()))
            .collect()
    }

    // <block affects="docs/symbols.md:markdown-paths, src/skill.md#/blockwatch/Symbols/Markdown">
    #[test]
    fn nested_sections_parse_symbols_returns_each_section_with_its_body() -> anyhow::Result<()> {
        let source = r"Text before the first heading.

# Colors

Intro.

## Red

Warm.

### Dark red

Deep.

## Blue

#### Navy

Deep blue.

# Shapes
";

        assert_eq!(
            derived_symbols(source)?,
            expected_symbols(&[
                (
                    "/Colors",
                    r"Intro.

## Red

Warm.

### Dark red

Deep.

## Blue

#### Navy

Deep blue."
                ),
                (
                    "/Colors/Red",
                    r"Warm.

### Dark red

Deep."
                ),
                ("/Colors/Red/Dark red", "Deep."),
                (
                    "/Colors/Blue",
                    r"#### Navy

Deep blue."
                ),
                ("/Colors/Blue/Navy", "Deep blue."),
                ("/Shapes", ""),
            ])?
        );
        Ok(())
    }

    #[test]
    fn section_parse_symbols_returns_byte_ranges_without_the_blank_lines_after_it()
    -> anyhow::Result<()> {
        let source = r"# Café

Crème.


# Tea
";

        let symbols = parser()?.parse_symbols(source)?;

        assert_eq!(
            symbols[0].def_byte_ranges,
            vec![test_utils::substr_range(source, "# Café\n\nCrème.")]
        );
        assert_eq!(
            symbols[0].value,
            Some(ScalarValue {
                text: "Crème.".to_string(),
                byte_range: test_utils::substr_range(source, "Crème."),
            })
        );
        let tea_end = test_utils::substr_range(source, "# Tea").end;
        assert_eq!(
            symbols[1].def_byte_ranges,
            vec![test_utils::substr_range(source, "# Tea")]
        );
        assert_eq!(
            symbols[1].value,
            Some(ScalarValue {
                text: String::new(),
                byte_range: tea_end..tea_end,
            })
        );
        Ok(())
    }

    #[test]
    fn heading_with_markup_parse_symbols_returns_the_text_a_reader_sees() -> anyhow::Result<()> {
        let source = r"## The `--diff` flag

## *Red* **and** ~~blue~~ ##

## [Shapes](shapes.md) \#1
";

        assert_eq!(
            derived_symbols(source)?,
            expected_symbols(&[
                ("/The --diff flag", ""),
                ("/Red and blue", ""),
                ("/Shapes #1", ""),
            ])?
        );
        Ok(())
    }

    #[test]
    fn setext_heading_parse_symbols_ends_the_deeper_section_above_it() -> anyhow::Result<()> {
        let source = r"# Colors

### Red

Warm.

Blue
----

Cool.
";

        assert_eq!(
            derived_symbols(source)?,
            expected_symbols(&[
                (
                    "/Colors",
                    r"### Red

Warm.

Blue
----

Cool."
                ),
                ("/Colors/Red", "Warm."),
                ("/Colors/Blue", "Cool."),
            ])?
        );
        Ok(())
    }

    #[test]
    fn heading_lookalikes_parse_symbols_returns_no_section_for_them() -> anyhow::Result<()> {
        let source = r"---
title: Colors
---

# Colors

```sh
# Comment
```

> ## Quoted

- ## Listed

<div>
## Inside HTML
</div>
";

        assert_eq!(
            derived_symbols(source)?,
            expected_symbols(&[(
                "/Colors",
                r"```sh
# Comment
```

> ## Quoted

- ## Listed

<div>
## Inside HTML
</div>"
            )])?
        );
        Ok(())
    }

    #[test]
    fn same_heading_twice_parse_symbols_returns_one_symbol_each() -> anyhow::Result<()> {
        let source = r"## Example

One.

## Example

Two.
";

        assert_eq!(
            derived_symbols(source)?,
            expected_symbols(&[("/Example", "One."), ("/Example", "Two.")])?
        );
        Ok(())
    }
    // </block>

    #[test]
    fn parses_markdown_blocks_correctly() -> anyhow::Result<()> {
        let mut parser = parser()?;

        let content = r#"
# Header
[foo]: /url "title"

[//]: # (<block name="md_block">)
Some text here

[//]: # (</block>)

[//]: # (<block name="md_block_2">)
Some text here 2

[//]: # (<block name="md_block_3">)
Some text here 3

[//]: # (</block>)
[//]: # (</block>)
"#;
        let blocks = parser
            .parse_blocks(content)
            .collect::<anyhow::Result<Vec<_>>>()?;

        assert_eq!(
            blocks,
            vec![
                Block {
                    attributes: HashMap::from([("name".to_string(), "md_block".to_string())]),
                    start_tag_position_range: Position::new(5, 10)..Position::new(5, 33),
                    content: Content::source(
                        test_utils::substr_range(content, "Some text here\n\n"),
                        Position::new(6, 1)..Position::new(8, 1),
                    ),
                    declaration: Declaration::Tags,
                    severity: BlockSeverity::Error,
                },
                Block {
                    attributes: HashMap::from([("name".to_string(), "md_block_2".to_string())]),
                    start_tag_position_range: Position::new(10, 10)..Position::new(10, 35),
                    content: Content::source(
                        test_utils::substr_range(
                            content,
                            "Some text here 2\n\n[//]: # (<block name=\"md_block_3\">)\nSome text here 3\n\n[//]: # (</block>)\n"
                        ),
                        Position::new(11, 1)..Position::new(17, 1),
                    ),
                    declaration: Declaration::Tags,
                    severity: BlockSeverity::Error,
                },
                Block {
                    attributes: HashMap::from([("name".to_string(), "md_block_3".to_string())]),
                    start_tag_position_range: Position::new(13, 10)..Position::new(13, 35),
                    content: Content::source(
                        test_utils::substr_range(content, "Some text here 3\n\n"),
                        Position::new(14, 1)..Position::new(16, 1),
                    ),
                    declaration: Declaration::Tags,
                    severity: BlockSeverity::Error,
                }
            ]
        );

        Ok(())
    }

    #[test]
    fn parses_link_reference_comment_with_non_ascii_destination() -> anyhow::Result<()> {
        let mut parser = parser()?;
        let content = "\
[//]: /café (<block name=\"unicode_title\">)
Text

[//]: /café (</block>)
";
        let blocks = parser
            .parse_blocks(content)
            .collect::<anyhow::Result<Vec<_>>>()?;

        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].attributes["name"], "unicode_title");
        assert_eq!(blocks[0].content.text(content), "Text\n\n");

        Ok(())
    }

    #[test]
    fn link_reference_comment_with_quote_in_destination_reads_title() -> anyhow::Result<()> {
        let mut parser = parser()?;
        let content = r#"[//]: it's (<block name="a">)
Text

[//]: # (</block>)
"#;
        let blocks = parser
            .parse_blocks(content)
            .collect::<anyhow::Result<Vec<_>>>()?;

        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].attributes["name"], "a");
        assert_eq!(blocks[0].content.text(content), "Text\n\n");
        Ok(())
    }

    #[test]
    fn link_reference_comment_with_title_on_next_line_reports_tag_line() -> anyhow::Result<()> {
        let mut parser = parser()?;
        let content = r#"[//]: #
(<block name="a">)
Text

[//]: # (</block>)
"#;
        let blocks = parser
            .parse_blocks(content)
            .collect::<anyhow::Result<Vec<_>>>()?;

        assert_eq!(
            blocks[0].start_tag_position_range,
            Position::new(2, 2)..Position::new(2, 18)
        );
        Ok(())
    }

    #[test]
    fn link_reference_without_title_or_with_another_label_is_ignored() -> anyhow::Result<()> {
        let mut parser = parser()?;
        let content = r#"<!-- <block name="a"> -->
Text

[//]: #
[other]: /url (<block name="b">)

<!-- </block> -->
"#;
        let blocks = parser
            .parse_blocks(content)
            .collect::<anyhow::Result<Vec<_>>>()?;

        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].attributes["name"], "a");
        Ok(())
    }

    #[test]
    fn link_reference_comment_with_each_title_delimiter_parse_blocks_reads_title()
    -> anyhow::Result<()> {
        let content = r#"[//]: # (<block name="a">)
[//]: # '<block name="b">'
[//]: # "<block name='c'>"

Text

[//]: # "</block>"
[//]: # '</block>'
[//]: # (</block>)
"#;

        assert_eq!(block_names(content)?, ["a", "b", "c"]);
        Ok(())
    }

    #[test]
    fn link_reference_comment_with_escaped_parentheses_parse_blocks_reads_whole_title()
    -> anyhow::Result<()> {
        let content = r#"[//]: # (<block name="a\(b\)">)
Text

[//]: # (</block>)
"#;

        assert_eq!(block_names(content)?, [r"a\(b\)"]);
        Ok(())
    }

    #[test]
    fn link_reference_comment_with_each_line_ending_parse_blocks_returns_content_from_next_line()
    -> anyhow::Result<()> {
        for line_ending in ["\n", "\r\n", "\r"] {
            let mut parser = parser()?;
            let content = format!(
                r#"[//]: # (<block name="a">){line_ending}Text{line_ending}{line_ending}[//]: # (</block>){line_ending}"#
            );

            let blocks = parser
                .parse_blocks(&content)
                .collect::<anyhow::Result<Vec<_>>>()?;

            assert_eq!(
                blocks[0].content.text(&content),
                format!("Text{line_ending}{line_ending}"),
                "{line_ending:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn inline_comment_after_a_code_span_holding_non_ascii_reports_a_character_column()
    -> anyhow::Result<()> {
        let mut parser = parser()?;
        // Code spans are blanked byte by byte before the HTML pass, which turns one multi-byte
        // character into several spaces; the column must still count the source's characters.
        let content = "`café` <!-- <block name=\"x\"> -->\nText\n\n<!-- </block> -->\n";

        let blocks = parser
            .parse_blocks(content)
            .collect::<anyhow::Result<Vec<_>>>()?;

        assert_eq!(
            blocks[0].start_tag_position_range.start,
            Position::new(1, 13)
        );
        Ok(())
    }

    #[test]
    fn parses_inline_html_comments_correctly() -> anyhow::Result<()> {
        let mut parser = parser()?;

        // An HTML comment with other text before it on the same line is inline HTML, not an
        // `html_block`. A `<!--` inside inline code is literal text, not a comment.
        let content = r#"
# Header

Some text <!-- <block name="inline_block"> --> and
more content here
ending text <!-- </block> --> tail.

Inline code `<!-- <block name="ignored"> -->` is not a comment.
"#;
        let blocks = parser
            .parse_blocks(content)
            .collect::<anyhow::Result<Vec<_>>>()?;

        assert_eq!(
            blocks,
            vec![Block {
                attributes: HashMap::from([("name".to_string(), "inline_block".to_string())]),
                start_tag_position_range: Position::new(4, 16)..Position::new(4, 43),
                content: Content::source(
                    test_utils::substr_range(content, " and\nmore content here\nending text "),
                    Position::new(4, 47)..Position::new(6, 13),
                ),
                declaration: Declaration::Tags,
                severity: BlockSeverity::Error,
            }]
        );

        Ok(())
    }

    #[test]
    fn parses_blocks_spanning_block_level_and_inline_html_comments() -> anyhow::Result<()> {
        let mut parser = parser()?;

        let content = r#"
<!-- <block name="mixed"> -->
Some content.
Closing text <!-- </block> --> tail.
"#;
        let blocks = parser
            .parse_blocks(content)
            .collect::<anyhow::Result<Vec<_>>>()?;

        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].attributes["name"], "mixed");

        Ok(())
    }

    #[test]
    fn parses_blocks_opened_and_closed_inside_table_cells() -> anyhow::Result<()> {
        let mut parser = parser()?;

        // GitHub-flavored Markdown renders HTML comments inside table cells, so a table is a
        // place a reader can reasonably put a marker and a table of values is exactly the
        // shape a `keep-sorted` rule is written for.
        let content = r#"| item |
| --- |
| <!-- <block name="table_rows"> --> first |
| second |
| <!-- </block> --> third |
"#;
        let blocks = parser
            .parse_blocks(content)
            .collect::<anyhow::Result<Vec<_>>>()?;

        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].attributes["name"], "table_rows");
        assert_eq!(
            blocks[0].start_tag_position_range.start,
            Position::new(3, 8)
        );

        Ok(())
    }

    #[test]
    fn comments_in_block_quote_parse_blocks_returns_the_text_between_them() -> anyhow::Result<()> {
        let mut parser = parser()?;
        let content = r#"> <!-- <block name="a"> -->
> Text
> <!-- </block> -->
"#;

        let blocks = parser
            .parse_blocks(content)
            .collect::<anyhow::Result<Vec<_>>>()?;

        assert_eq!(blocks[0].content.text(content), "\n> Text\n> ");
        Ok(())
    }

    #[test]
    fn parses_blocks_spanning_link_reference_and_html_comment_syntaxes() -> anyhow::Result<()> {
        let mut parser = parser()?;

        // The `[//]:` and `<!--` comments are merged into one source-ordered stream, so a block
        // can open in one syntax and close in the other, in either direction.
        let content = r#"
[//]: # (<block name="a">)

First block content.

<!-- </block> -->

<!-- <block name="b"> -->

Second block content.

[//]: # (</block>)
"#;
        let blocks = parser
            .parse_blocks(content)
            .collect::<anyhow::Result<Vec<_>>>()?;

        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].attributes["name"], "a");
        assert!(
            blocks[0]
                .content
                .text(content)
                .contains("First block content")
        );
        assert_eq!(blocks[1].attributes["name"], "b");
        assert!(
            blocks[1]
                .content
                .text(content)
                .contains("Second block content")
        );

        Ok(())
    }

    #[test]
    fn parses_html_blocks_correctly() -> anyhow::Result<()> {
        let mut parser = parser()?;

        let content = r#"
# Header

<div>
<!-- <block name="html_block"> -->
Some html content
<!-- </block> -->
</div>

[//]: # (<block name="md_block">)
Some markdown content

[//]: # (</block>)

<!-- <block name="html_block2"> -->Not wrapped in HTML tags<!-- </block> -->

<!-- <block name="html_block3"> -->
Not wrapped in HTML tags on multiple lines
<!-- </block> -->
"#;
        let blocks = parser
            .parse_blocks(content)
            .collect::<anyhow::Result<Vec<_>>>()?;

        assert_eq!(
            blocks,
            vec![
                Block {
                    attributes: HashMap::from([("name".to_string(), "html_block".to_string())]),
                    start_tag_position_range: Position::new(5, 6)..Position::new(5, 31),
                    content: Content::source(
                        test_utils::substr_range(content, "\nSome html content\n"),
                        Position::new(5, 35)..Position::new(7, 1),
                    ),
                    declaration: Declaration::Tags,
                    severity: BlockSeverity::Error,
                },
                Block {
                    attributes: HashMap::from([("name".to_string(), "md_block".to_string())]),
                    start_tag_position_range: Position::new(10, 10)..Position::new(10, 33),
                    content: Content::source(
                        test_utils::substr_range(content, "Some markdown content\n\n"),
                        Position::new(11, 1)..Position::new(13, 1),
                    ),
                    declaration: Declaration::Tags,
                    severity: BlockSeverity::Error,
                },
                Block {
                    attributes: HashMap::from([("name".to_string(), "html_block2".to_string())]),
                    start_tag_position_range: Position::new(15, 6)..Position::new(15, 32),
                    content: Content::source(
                        test_utils::substr_range(content, "Not wrapped in HTML tags"),
                        Position::new(15, 36)..Position::new(15, 60),
                    ),
                    declaration: Declaration::Tags,
                    severity: BlockSeverity::Error,
                },
                Block {
                    attributes: HashMap::from([("name".to_string(), "html_block3".to_string())]),
                    start_tag_position_range: Position::new(17, 6)..Position::new(17, 32),
                    content: Content::source(
                        test_utils::substr_range(
                            content,
                            "\nNot wrapped in HTML tags on multiple lines\n",
                        ),
                        Position::new(17, 36)..Position::new(19, 1),
                    ),
                    declaration: Declaration::Tags,
                    severity: BlockSeverity::Error,
                },
            ]
        );

        Ok(())
    }

    #[test]
    fn code_block_with_fence_lookalike_parse_blocks_returns_the_block_after_it()
    -> anyhow::Result<()> {
        // A closing fence is indented by at most 3 spaces, where a tab counts as 4, and only
        // spaces can follow it. So none of these lines ends the code block.
        for lookalike in ["    ```", "\t```", "text```", "``` text"] {
            let content = format!(
                r#"```
{lookalike}
```

<!-- <block name="a"> -->
Text
<!-- </block> -->
"#
            );

            assert_eq!(block_names(&content)?, ["a"], "{lookalike:?}");
        }
        Ok(())
    }

    #[test]
    fn code_block_closed_by_fence_indented_three_spaces_parse_blocks_returns_only_the_block_after_it()
    -> anyhow::Result<()> {
        let content = r#"```
<!-- <block name="in_code"> -->
   ```
<!-- <block name="a"> -->
Text
<!-- </block> -->
"#;

        assert_eq!(block_names(content)?, ["a"]);
        Ok(())
    }

    #[test]
    fn comment_outside_raw_html_parse_blocks_returns_no_block_from_it() -> anyhow::Result<()> {
        // In HTML, the alt text of an image and the title of a link are attribute values, and an
        // escaped `<` is text, so none of these is a comment.
        let content = r#"<!-- <block name="a"> -->
![Alt <!-- <block name="in_alt"> -->](image.png)
[Link](page.md "<!-- <block name='in_title'> -->")
\<!-- <block name="escaped"> -->
<!-- </block> -->
"#;

        assert_eq!(block_names(content)?, ["a"]);
        Ok(())
    }

    #[test]
    fn frontmatter_parse_blocks_returns_no_block_from_it() -> anyhow::Result<()> {
        // Frontmatter is YAML, where `<!--` starts no comment.
        let content = r#"---
title: <!-- <block name="in_frontmatter"> -->
---

<!-- <block name="a"> -->
Text
<!-- </block> -->
"#;

        assert_eq!(block_names(content)?, ["a"]);
        Ok(())
    }
}
