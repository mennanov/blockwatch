use crate::language_parsers;
use crate::language_parsers::{
    Comment, CommentsParser, LanguageParser, LanguageParserImpl, TreeSitterCommentsParser,
    blank_preserving_line_breaks, node_text, xml_style_comments_parser,
};
use crate::symbol_path::SymbolPath;
use crate::symbols::{ScalarValue, Symbol, SymbolsParser};
use anyhow::Context;
use markdown::mdast::Node;
use markdown::{Constructs, ParseOptions};
use std::ops::Range;
use tree_sitter::StreamingIterator;

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
        // tree-sitter-md, which finds the comments, nests sections too, but not the way a reader
        // sees them: a setext heading starts no section, and a heading in a quote or a list item
        // starts one.
        let options = ParseOptions {
            constructs: Constructs {
                // Without it, the `---` that closes the front matter makes the line above it a
                // heading.
                frontmatter: true,
                ..Constructs::gfm()
            },
            ..ParseOptions::default()
        };
        let root = markdown::to_mdast(source, &options)
            .expect("only MDX has syntax errors, and these options leave it off");
        Ok(section_symbols(&root, source))
    }
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
        let position = node
            .position()
            .expect("markdown-rs gives every node it parses a position");
        let range = position.start.offset..position.end.offset;
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

/// Parses Markdown `[//]:` comments and the HTML comments embedded in Markdown.
///
/// HTML comments live either in [HTML blocks](https://github.github.com/gfm/#html-block) or in
/// the inline content of other blocks (a comment with text before it on the same line).
struct MdCommentsParser {
    md_tree_sitter_parser: tree_sitter::Parser,
    md_regions_query: tree_sitter::Query,
    inline_tree_sitter_parser: tree_sitter::Parser,
    code_span_query: tree_sitter::Query,
    html_comments_parser: TreeSitterCommentsParser,
}

impl MdCommentsParser {
    fn new() -> anyhow::Result<Self> {
        let markdown_lang: tree_sitter::Language = tree_sitter_md::LANGUAGE.into();
        let md_tree_sitter_parser = language_parsers::tree_sitter_parser(&markdown_lang)?;
        let md_regions_query = tree_sitter::Query::new(
            &markdown_lang,
            "(html_block) @html_block \
             (inline) @inline \
             (pipe_table_cell) @pipe_table_cell \
             (link_reference_definition) @link_reference_definition",
        )
        .context("failed to compile tree-sitter markdown regions query")?;

        let inline_lang: tree_sitter::Language = tree_sitter_md::INLINE_LANGUAGE.into();
        let inline_tree_sitter_parser = language_parsers::tree_sitter_parser(&inline_lang)?;
        let code_span_query = tree_sitter::Query::new(&inline_lang, "(code_span) @code_span")
            .context("failed to compile tree-sitter code span query")?;

        let html_lang = tree_sitter_html::LANGUAGE.into();
        let html_comments_parser = xml_style_comments_parser(&html_lang, "comment")?;
        Ok(Self {
            md_tree_sitter_parser,
            md_regions_query,
            inline_tree_sitter_parser,
            code_span_query,
            html_comments_parser,
        })
    }

    /// Extracts the `[//]:` comments and the HTML comments (from `html_block` and `inline`
    /// regions) from a single parse of the Markdown tree.
    fn parse_comments(&mut self, contents: &str) -> Vec<Comment> {
        let tree = self.md_tree_sitter_parser.parse(contents, None).unwrap();
        let mut query_cursor = tree_sitter::QueryCursor::new();
        let mut matches = query_cursor.matches(
            &self.md_regions_query,
            tree.root_node(),
            contents.as_bytes(),
        );
        let mut comments = Vec::new();
        while let Some(query_match) = matches.next() {
            let node = query_match
                .captures()
                .first()
                .expect("Empty Tree-sitter region query match")
                .node;
            let region = node_text(&node, contents);

            // `[//]:` comments are direct nodes of the Markdown tree, so their positions are
            // already the source positions.
            if node.kind() == "link_reference_definition" {
                if let Some(text) = link_reference_definition_comment_text(&node, contents) {
                    comments.push(Comment::from_node(&node, contents, text));
                }
                continue;
            }

            // The remaining regions carry HTML comments, which require a `<!--`.
            if !region.contains("<!--") {
                continue;
            }
            // Inline regions may contain code spans whose contents must not be mistaken for
            // comments, but a code span requires a backtick; without one the region is parsed
            // as is. A table cell carries inline content too, code spans included.
            let html_comments: Vec<Comment> =
                if matches!(node.kind(), "inline" | "pipe_table_cell") && region.contains('`') {
                    let view = Self::inline_html_view(
                        &mut self.inline_tree_sitter_parser,
                        &self.code_span_query,
                        region,
                    );
                    self.html_comments_parser.parse(&view).collect()
                } else {
                    self.html_comments_parser.parse(region).collect()
                };
            for mut comment in html_comments {
                // The comment's positions are relative to the region; shift them to the source.
                comment.shift_into_source(node.start_position().row, node.start_byte(), contents);
                comments.push(comment);
            }
        }
        comments
    }

    /// Returns a copy of the inline `region` with its code spans blanked to whitespace (line
    /// breaks kept), so that a `<!--` inside inline code is not mistaken for an HTML comment.
    /// The copy is byte-for-byte aligned with the region, so comment positions parsed from it
    /// are valid in the region.
    fn inline_html_view(
        inline_tree_sitter_parser: &mut tree_sitter::Parser,
        code_span_query: &tree_sitter::Query,
        region: &str,
    ) -> String {
        let mut view = region.as_bytes().to_vec();
        let tree = inline_tree_sitter_parser.parse(region, None).unwrap();
        let mut query_cursor = tree_sitter::QueryCursor::new();
        let mut matches =
            query_cursor.matches(code_span_query, tree.root_node(), region.as_bytes());
        while let Some(query_match) = matches.next() {
            let range = query_match
                .captures()
                .first()
                .expect("Empty Tree-sitter code_span query match")
                .node
                .byte_range();
            blank_preserving_line_breaks(&mut view[range]);
        }
        String::from_utf8(view).expect("view is built from the valid-UTF-8 region")
    }
}

impl CommentsParser for MdCommentsParser {
    fn parse<'source>(
        &'source mut self,
        contents: &'source str,
    ) -> impl Iterator<Item = Comment> + 'source {
        // `[//]:` and HTML comments come out of the merged parse interleaved by region; sort them
        // into a single source-ordered stream so a block can span comments of different kinds.
        let mut comments = self.parse_comments(contents);
        comments.sort_by_key(|comment| comment.source_range.start);
        comments.into_iter()
    }
}

/// Returns the text of `node`, a link reference definition in `contents`, with everything but its
/// title blanked, so the length is preserved. Returns `None` when `node` is not a `[//]:` comment,
/// or has no title.
fn link_reference_definition_comment_text(
    node: &tree_sitter::Node,
    contents: &str,
) -> Option<String> {
    let mut cursor = node.walk();
    let mut label = None;
    let mut title = None;
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "link_label" => label = Some(child),
            "link_title" => title = Some(child),
            _ => {}
        }
    }
    if node_text(&label?, contents) != "[//]" {
        return None;
    }
    // A title is read from the tree, not found in the text, because its delimiters, such as `'`,
    // can also appear in the destination.
    let title = title?;
    let mut text = contents.as_bytes()[node.byte_range()].to_vec();
    blank_preserving_line_breaks(&mut text[..title.start_byte() - node.start_byte()]);
    blank_preserving_line_breaks(&mut text[title.end_byte() - node.start_byte()..]);
    Some(String::from_utf8(text).expect("only whole characters are blanked"))
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
}
