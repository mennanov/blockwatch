use crate::languages::{
    Comment, CommentsParser, LanguageParser, LanguageParserImpl, TreeSitterCommentsParser,
    blank_preserving_line_breaks, xml_style_comments_parser,
};
use crate::position::LineIndex;
use crate::symbols::SymbolPath;
use crate::symbols::{ScalarValue, Symbol, Symbols, SymbolsParser};
use markdown::mdast::{Definition, Node};
use markdown::{Constructs, ParseOptions};
use std::ops::Range;

/// Returns a [`LanguageParser`] for Markdown. Its symbols are the sections of a file.
pub(super) fn parser() -> impl LanguageParser {
    LanguageParserImpl::new(MdCommentsParser::new()).with_symbols(SectionsParser)
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
    fn parse(&mut self, source: &str) -> anyhow::Result<Symbols> {
        Ok(section_symbols(&parse_markdown(source), source).into())
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
///
/// # Panics
/// Panics if `position` is `None`, which markdown-rs never gives a node it parsed.
fn byte_range(position: Option<&markdown::unist::Position>) -> Range<usize> {
    let position = position.expect("markdown-rs gives every node it parses a position");
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
        let range = byte_range(node.position());
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
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "Both kinds of Markdown heading, `#` and underlined, are `Node::Heading`, so a new kind \
              of node would not be a heading either."
)]
fn heading_depth(node: &Node) -> Option<u8> {
    match node {
        Node::Heading(heading) => Some(heading.depth),
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
    fn new() -> Self {
        let html_lang = tree_sitter_html::LANGUAGE.into();
        Self {
            html_comments_parser: xml_style_comments_parser(&html_lang, "comment"),
        }
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
            #[expect(
                clippy::wildcard_enum_match_arm,
                reason = "Raw HTML that Markdown keeps as HTML is always `Node::Html`, and every link \
                          reference definition is `Node::Definition`. A `<!--` in the alt text of an \
                          image or in the title of a link is part of an attribute, not a comment."
            )]
            match node {
                Node::Html(_) => {
                    comments.extend(self.html_comments(
                        byte_range(node.position()),
                        source,
                        &lines,
                    ));
                }
                Node::Definition(definition) => {
                    comments.extend(link_reference_comment(definition, source, &lines));
                }
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
/// the end of its line, with everything but the text of the title blanked, so the length is kept.
/// Returns `None` when `definition` is not a `[//]:` comment, or has no title.
fn link_reference_comment(
    definition: &Definition,
    source: &str,
    lines: &LineIndex,
) -> Option<Comment> {
    if definition.label.as_deref() != Some("//") || definition.title.is_none() {
        return None;
    }
    let definition_range = byte_range(definition.position.as_ref());
    // Nothing else can be on the line of a definition, so the comment takes the line break too,
    // and a block's content starts on the next line.
    let range = definition_range.start..line_end(source.as_bytes(), definition_range.end);
    let mut text = source.as_bytes()[range.clone()].to_vec();
    let title_start = title_start(&text[..definition_range.len()]);
    // The delimiters around the title are blanked too. Otherwise a tag that wraps over several
    // `[//]:` lines would hold a `)` and a `(` at each line break.
    blank_preserving_line_breaks(&mut text[..=title_start]);
    blank_preserving_line_breaks(&mut text[definition_range.len() - 1..definition_range.len()]);
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
    use crate::languages::comments::test_utils::comment_sources;
    use crate::position::{Position, test_utils};

    /// The symbols `source` derives, as `(path, value)` pairs in the order they were derived.
    fn derived_symbols(source: &str) -> anyhow::Result<Vec<(SymbolPath, String)>> {
        Ok(parser()
            .parse_symbols(source)?
            .symbols
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

    /// The text of each comment in `content`, without the blanks around it. So a `[//]:` comment
    /// gives its title.
    fn comment_titles(content: &str) -> anyhow::Result<Vec<String>> {
        Ok(parser()
            .parse_comments(content)?
            .map(|comment| comment.comment_text.trim().to_string())
            .collect())
    }

    // <block name="markdown-section-tests" affects="docs/symbols.md:markdown-paths">
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

        let symbols = parser().parse_symbols(source)?.symbols;

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
    fn link_reference_comment_parse_comments_returns_its_line_with_only_the_title_kept()
    -> anyhow::Result<()> {
        let content = "Text\n\n[//]: # (title)\nAfter\n";

        let comments: Vec<Comment> = parser().parse_comments(content)?.collect();

        assert_eq!(
            comments,
            [Comment {
                position_range: Position::new(3, 1)..Position::new(4, 1),
                source_range: 6..22,
                comment_text: "         title \n".to_string(),
            }]
        );
        Ok(())
    }

    #[test]
    fn html_comments_in_html_blocks_parse_comments_returns_each() -> anyhow::Result<()> {
        let content = r#"
# Header

<div>
<!-- in a div -->
</div>

<!-- first -->Text<!-- second -->

<!-- alone -->
"#;

        assert_eq!(
            comment_sources(&mut parser(), content)?,
            [
                "<!-- in a div -->",
                "<!-- first -->",
                "<!-- second -->",
                "<!-- alone -->"
            ]
        );
        Ok(())
    }

    #[test]
    fn inline_html_comments_parse_comments_returns_each() -> anyhow::Result<()> {
        // An HTML comment with other text before it on the same line is inline HTML, not an
        // HTML block. A `<!--` inside inline code is literal text, not a comment.
        let content = r#"<!-- block level -->

Some text <!-- inline --> and
more text <!-- inline too --> tail.

Inline code `<!-- in code -->` is not a comment.
"#;

        assert_eq!(
            comment_sources(&mut parser(), content)?,
            [
                "<!-- block level -->",
                "<!-- inline -->",
                "<!-- inline too -->"
            ]
        );
        Ok(())
    }

    #[test]
    fn link_reference_and_html_comments_parse_comments_returns_them_in_source_order()
    -> anyhow::Result<()> {
        let content = r#"[//]: # (first)

<!-- second -->

<!-- third -->

[//]: # (fourth)
"#;

        assert_eq!(
            comment_sources(&mut parser(), content)?,
            [
                "[//]: # (first)\n",
                "<!-- second -->",
                "<!-- third -->",
                "[//]: # (fourth)\n"
            ]
        );
        Ok(())
    }

    #[test]
    fn link_reference_comment_with_each_title_delimiter_parse_comments_returns_only_the_title()
    -> anyhow::Result<()> {
        let content = r#"[//]: # (paren)
[//]: # 'single'
[//]: # "double"
"#;

        assert_eq!(comment_titles(content)?, ["paren", "single", "double"]);
        Ok(())
    }

    #[test]
    fn link_reference_comment_with_escaped_parentheses_parse_comments_returns_whole_title()
    -> anyhow::Result<()> {
        assert_eq!(comment_titles(r"[//]: # (a\(b\))")?, [r"a\(b\)"]);
        Ok(())
    }

    #[test]
    fn link_reference_comment_with_non_ascii_destination_parse_comments_returns_title()
    -> anyhow::Result<()> {
        assert_eq!(comment_titles("[//]: /café (title)\n")?, ["title"]);
        Ok(())
    }

    #[test]
    fn link_reference_comment_with_quote_in_destination_parse_comments_returns_title()
    -> anyhow::Result<()> {
        assert_eq!(comment_titles("[//]: it's (title)\n")?, ["title"]);
        Ok(())
    }

    #[test]
    fn link_reference_comment_with_title_on_next_line_parse_comments_keeps_the_line_break()
    -> anyhow::Result<()> {
        let content = "[//]: #\n(title)\n";

        let texts: Vec<String> = parser()
            .parse_comments(content)?
            .map(|comment| comment.comment_text)
            .collect();

        assert_eq!(texts, ["       \n title \n"]);
        Ok(())
    }

    #[test]
    fn link_reference_comment_with_each_line_ending_parse_comments_returns_the_line_ending_too()
    -> anyhow::Result<()> {
        for line_ending in ["\n", "\r\n", "\r"] {
            let content = format!("[//]: # (title){line_ending}Text{line_ending}");

            assert_eq!(
                comment_sources(&mut parser(), &content)?,
                [format!("[//]: # (title){line_ending}")],
                "{line_ending:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn link_reference_without_title_or_with_another_label_parse_comments_returns_no_comment()
    -> anyhow::Result<()> {
        let content = r#"[//]: #
[other]: /url (title)
[foo]: /url "title"
"#;

        assert_eq!(comment_sources(&mut parser(), content)?, Vec::<&str>::new());
        Ok(())
    }

    #[test]
    fn inline_comment_after_a_code_span_holding_non_ascii_reports_a_character_column()
    -> anyhow::Result<()> {
        // Code spans are blanked byte by byte before the HTML pass, which turns one multi-byte
        // character into several spaces; the column must still count the source's characters.
        let content = "`café` <!-- comment -->\n";

        let comments: Vec<Comment> = parser().parse_comments(content)?.collect();

        assert_eq!(comments[0].position_range.start, Position::new(1, 8));
        Ok(())
    }

    #[test]
    fn comments_in_table_cells_parse_comments_returns_each_at_its_column() -> anyhow::Result<()> {
        // GitHub-flavored Markdown renders HTML comments inside table cells, so a table is a
        // place a reader can reasonably put a marker and a table of values is exactly the
        // shape a `keep-sorted` rule is written for.
        let content = r#"| item |
| --- |
| <!-- first --> one |
| two |
| <!-- second --> three |
"#;

        let comments: Vec<Comment> = parser().parse_comments(content)?.collect();

        let sources: Vec<&str> = comments
            .iter()
            .map(|comment| &content[comment.source_range.clone()])
            .collect();
        assert_eq!(sources, ["<!-- first -->", "<!-- second -->"]);
        assert_eq!(comments[0].position_range.start, Position::new(3, 3));
        Ok(())
    }

    #[test]
    fn comments_in_block_quote_parse_comments_returns_each_without_the_quote_marker()
    -> anyhow::Result<()> {
        let content = r#"> <!-- first -->
> Text
> <!-- second -->
"#;

        assert_eq!(
            comment_sources(&mut parser(), content)?,
            ["<!-- first -->", "<!-- second -->"]
        );
        Ok(())
    }

    #[test]
    fn code_block_with_fence_lookalike_parse_comments_returns_the_comment_after_it()
    -> anyhow::Result<()> {
        // A closing fence is indented by at most 3 spaces, where a tab counts as 4, and only
        // spaces can follow it. So none of these lines ends the code block.
        for lookalike in ["    ```", "\t```", "text```", "``` text"] {
            let content = format!(
                r#"```
{lookalike}
```

<!-- after -->
"#
            );

            assert_eq!(
                comment_sources(&mut parser(), &content)?,
                ["<!-- after -->"],
                "{lookalike:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn code_block_closed_by_fence_indented_three_spaces_parse_comments_returns_only_the_comment_after_it()
    -> anyhow::Result<()> {
        let content = r#"```
<!-- in code -->
   ```
<!-- after -->
"#;

        assert_eq!(comment_sources(&mut parser(), content)?, ["<!-- after -->"]);
        Ok(())
    }

    #[test]
    fn comment_outside_raw_html_parse_comments_returns_no_comment_for_it() -> anyhow::Result<()> {
        // In HTML, the alt text of an image and the title of a link are attribute values, and an
        // escaped `<` is text, so none of these is a comment.
        let content = r#"<!-- before -->
![Alt <!-- in alt -->](image.png)
[Link](page.md "<!-- in title -->")
\<!-- escaped -->
<!-- after -->
"#;

        assert_eq!(
            comment_sources(&mut parser(), content)?,
            ["<!-- before -->", "<!-- after -->"]
        );
        Ok(())
    }

    #[test]
    fn frontmatter_parse_comments_returns_no_comment_for_it() -> anyhow::Result<()> {
        // Frontmatter is YAML, where `<!--` starts no comment.
        let content = r#"---
title: <!-- in frontmatter -->
---

<!-- after -->
"#;

        assert_eq!(comment_sources(&mut parser(), content)?, ["<!-- after -->"]);
        Ok(())
    }
}
