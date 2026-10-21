//! Finds the comments in MDX files. MDX is Markdown with JSX, and its comments are JavaScript
//! comments: `{/* ... */}` in the text, and `//` or `/* */` inside its `{...}` expressions, its
//! JSX attributes and its `import` and `export` blocks. No MDX grammar for tree-sitter is on
//! crates.io, so the `markdown` crate finds those spans, and tree-sitter-javascript finds the
//! comments in each one.

use crate::LineIndex;
use crate::block_parser::Blocks;
use crate::blocks::Block;
use crate::language_parsers::markdown::{nodes_in_order, section_symbols};
use crate::language_parsers::{
    Comment, CommentsParser, LanguageParser, TreeSitterCommentsParser, c_style_comments_parser,
};
use crate::symbols::Symbol;
use markdown::mdast::{
    AttributeContent, AttributeValue, AttributeValueExpression, MdxFlowExpression, MdxJsxAttribute,
    MdxJsxExpressionAttribute, MdxJsxFlowElement, MdxJsxTextElement, MdxTextExpression, MdxjsEsm,
    Node, Stop,
};
use markdown::{Constructs, MdxSignal, ParseOptions};
use std::ops::Range;

/// Returns a [`LanguageParser`] for MDX. Its symbols are the sections of a file, as in Markdown.
pub(super) fn parser() -> anyhow::Result<impl LanguageParser> {
    MdxParser::new()
}

/// Finds the comments in MDX source.
///
/// The JavaScript is not checked, so two cases end early:
///
/// - A `{...}` expression ends at the `}` that balances its `{`, even when that `}` is in a
///   string or a comment.
/// - An `import` or `export` block ends at its first blank line.
struct MdxParser {
    js_comments_parser: TreeSitterCommentsParser,
}

impl MdxParser {
    fn new() -> anyhow::Result<Self> {
        Ok(Self {
            // MDX compiles to an ES module, where `<!--` doesn't start a comment.
            js_comments_parser: c_style_comments_parser(
                &tree_sitter_javascript::LANGUAGE.into(),
                "comment",
            )?,
        })
    }

    /// Returns the comments in `source`, in source order.
    ///
    /// # Errors
    /// Returns an error if `source` is not valid MDX.
    fn comments(&mut self, source: &str) -> anyhow::Result<Vec<Comment>> {
        let tree = parse_mdx(source)?;
        let lines = LineIndex::new(source);
        let mut comments = Vec::new();
        for javascript in javascript_ranges(&tree) {
            let start_row = lines.position(javascript.start).line - 1;
            #[expect(
                clippy::string_slice,
                reason = "markdown-rs gives offsets of whole characters"
            )]
            let javascript_source = &source[javascript.clone()];
            for mut comment in self.js_comments_parser.parse(javascript_source) {
                comment.shift_into_source(start_row, javascript.start, source);
                widen_lone_comment_to_braces(&mut comment, &javascript, source, &lines);
                comments.push(comment);
            }
        }
        Ok(comments)
    }
}

impl LanguageParser for MdxParser {
    fn parse_blocks<'a>(
        &'a mut self,
        contents: &'a str,
    ) -> Box<dyn Iterator<Item = anyhow::Result<Block>> + 'a> {
        match self.comments(contents) {
            Ok(comments) => Box::new(Blocks::new(comments.into_iter(), contents)),
            Err(error) => Box::new(std::iter::once(Err(error))),
        }
    }

    fn parse_symbols(&mut self, contents: &str) -> anyhow::Result<Vec<Symbol>> {
        Ok(section_symbols(&parse_mdx(contents)?, contents))
    }
}

/// Parses `source` as MDX that can start with frontmatter.
///
/// # Errors
/// Returns an error if `source` is not valid MDX.
fn parse_mdx(source: &str) -> anyhow::Result<Node> {
    let options = ParseOptions {
        constructs: Constructs {
            frontmatter: true,
            ..Constructs::mdx()
        },
        // Without this hook, markdown-rs reads `import` and `export` lines as text.
        mdx_esm_parse: Some(Box::new(|_| MdxSignal::Ok)),
        ..ParseOptions::mdx()
    };
    markdown::to_mdast(source, &options).map_err(|message| anyhow::anyhow!("{message}"))
}

/// Returns the byte ranges of the JavaScript in `tree`, in source order. An empty expression, such
/// as `{}`, has no range.
fn javascript_ranges(tree: &Node) -> impl Iterator<Item = Range<usize>> + '_ {
    nodes_in_order(tree)
        .flat_map(javascript_spans)
        .filter_map(|(value, stops)| source_range(value, stops))
}

/// Returns the byte range in the source of `value`, whose slices start where `stops` say, or
/// `None` if `value` is empty and so has no slices.
fn source_range(value: &str, stops: &[Stop]) -> Option<Range<usize>> {
    let &(_, start) = stops.first()?;
    // `value` leaves out the line prefixes of its container, such as the `> ` of a block quote, so
    // it is not a slice of the source. Its last slice runs to its end.
    let &(last_value_offset, last_source_offset) = stops.last()?;
    Some(start..last_source_offset + value.len() - last_value_offset)
}

/// Widens `comment` to cover the braces around `expression`, the byte range of the JavaScript it
/// was found in, when the expression holds nothing but this comment. A block's content then starts
/// after the `}` of its start tag and ends before the `{` of its end tag.
fn widen_lone_comment_to_braces(
    comment: &mut Comment,
    expression: &Range<usize>,
    source: &str,
    lines: &LineIndex,
) {
    #[expect(
        clippy::string_slice,
        reason = "markdown-rs and tree-sitter give these ranges"
    )]
    let before = &source[expression.start..comment.source_range.start];
    #[expect(
        clippy::string_slice,
        reason = "markdown-rs and tree-sitter give these ranges"
    )]
    let after = &source[comment.source_range.end..expression.end];
    // Only a `{...}` expression can hold nothing but a comment: an `import` or `export` block
    // always holds code.
    if !before.trim().is_empty() || !after.trim().is_empty() {
        return;
    }
    // The `}` can come after the end of `expression`: markdown-rs leaves out the indent or `> `
    // that starts the last line, when nothing else is on it.
    #[expect(clippy::string_slice, reason = "markdown-rs gives `expression`")]
    let close = expression.end
        + source[expression.end..]
            .find('}')
            .expect("a `{...}` expression ends with `}`");
    #[expect(clippy::string_slice, reason = "`close` is the index of a `}`")]
    let closing = &source[comment.source_range.end..close];
    comment.comment_text = format!(" {before}{}{closing} ", comment.comment_text);
    comment.source_range = expression.start - 1..close + 1;
    comment.position_range = lines.range(&comment.source_range);
}

/// Returns the JavaScript that `node` itself holds, not counting its children: each span's value
/// and its stops, in source order. Each stop maps an offset in the value to an offset in the
/// source, at the start of a slice.
fn javascript_spans(node: &Node) -> Vec<(&str, &[Stop])> {
    match node {
        Node::MdxjsEsm(MdxjsEsm { value, stops, .. })
        | Node::MdxFlowExpression(MdxFlowExpression { value, stops, .. })
        | Node::MdxTextExpression(MdxTextExpression { value, stops, .. }) => {
            vec![(value, stops)]
        }
        Node::MdxJsxFlowElement(MdxJsxFlowElement { attributes, .. })
        | Node::MdxJsxTextElement(MdxJsxTextElement { attributes, .. }) => attributes
            .iter()
            .filter_map(|attribute| match attribute {
                AttributeContent::Expression(MdxJsxExpressionAttribute { value, stops })
                | AttributeContent::Property(MdxJsxAttribute {
                    value:
                        Some(AttributeValue::Expression(AttributeValueExpression { value, stops })),
                    ..
                }) => Some((value.as_str(), stops.as_slice())),
                AttributeContent::Property(MdxJsxAttribute {
                    value: None | Some(AttributeValue::Literal(_)),
                    ..
                }) => None,
            })
            .collect(),
        // These hold no JavaScript of their own. Code, text and frontmatter can hold comment
        // markers, but those are not JavaScript comments.
        Node::Root(_)
        | Node::Blockquote(_)
        | Node::FootnoteDefinition(_)
        | Node::List(_)
        | Node::Toml(_)
        | Node::Yaml(_)
        | Node::Break(_)
        | Node::InlineCode(_)
        | Node::InlineMath(_)
        | Node::Delete(_)
        | Node::Emphasis(_)
        | Node::FootnoteReference(_)
        | Node::Html(_)
        | Node::Image(_)
        | Node::ImageReference(_)
        | Node::Link(_)
        | Node::LinkReference(_)
        | Node::Strong(_)
        | Node::Text(_)
        | Node::Code(_)
        | Node::Math(_)
        | Node::Heading(_)
        | Node::Table(_)
        | Node::ThematicBreak(_)
        | Node::TableRow(_)
        | Node::TableCell(_)
        | Node::ListItem(_)
        | Node::Definition(_)
        | Node::Paragraph(_) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Position;

    #[test]
    fn invalid_mdx_is_an_error() -> anyhow::Result<()> {
        let mut parser = parser()?;

        // MDX 2 and later have no HTML comments.
        let error = parser
            .parse_blocks("<!-- <block> -->\nText\n<!-- </block> -->\n")
            .collect::<anyhow::Result<Vec<_>>>()
            .unwrap_err();

        assert_eq!(
            error.to_string(),
            "1:2: Unexpected character `!` (U+0021) before name, expected a character that can \
             start a name, such as a letter, `$`, or `_` (note: to create a comment in MDX, use \
             `{/* text */}`) (markdown-rs:unexpected-character)"
        );

        Ok(())
    }

    #[test]
    fn parses_comments_correctly() -> anyhow::Result<()> {
        let comments = MdxParser::new()?.comments(
            r#"import {Chart} from './chart.js' // after an import
export const meta = {
  /* in an export */
  title: 'Café',
}

# Title

{/* flow */}

Text é {/* in text */} and `{/* code span */}`.

<Tabs>
  {// in a child
  }
</Tabs>

{"/* in a string */"}

{<!-- y
}

{}

```js
{/* in a fenced code block */}
```
"#,
        )?;

        assert_eq!(
            comments,
            vec![
                Comment {
                    position_range: Position::new(1, 34)..Position::new(1, 52),
                    source_range: 33..51,
                    comment_text: "   after an import".to_string()
                },
                Comment {
                    position_range: Position::new(3, 3)..Position::new(3, 21),
                    source_range: 76..94,
                    comment_text: "   in an export   ".to_string()
                },
                Comment {
                    position_range: Position::new(9, 1)..Position::new(9, 13),
                    source_range: 125..137,
                    comment_text: "    flow    ".to_string()
                },
                Comment {
                    position_range: Position::new(11, 8)..Position::new(11, 23),
                    source_range: 147..162,
                    comment_text: "    in text    ".to_string()
                },
                Comment {
                    position_range: Position::new(14, 3)..Position::new(15, 4),
                    source_range: 198..216,
                    comment_text: "    in a child\n   ".to_string()
                },
            ]
        );

        Ok(())
    }

    #[test]
    fn block_content_leaves_out_the_braces_around_its_tags() -> anyhow::Result<()> {
        let mut parser = parser()?;
        let content = "{/* <block> */}\n- blue\n- red\n{/* </block> */}\n";

        let blocks = parser
            .parse_blocks(content)
            .collect::<anyhow::Result<Vec<_>>>()?;

        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].content.text(content), "\n- blue\n- red\n");

        Ok(())
    }

    #[test]
    fn comment_in_jsx_attribute_is_found() -> anyhow::Result<()> {
        let comments = MdxParser::new()?.comments(
            r#"<Items
  values={[
    // a
    'red',
  ]}
  {...props /* b */}
/>

Text <Badge value={/* c */ 1} />

<Note title="/* not a comment */" hidden />
"#,
        )?;

        assert_eq!(
            comments,
            vec![
                Comment {
                    position_range: Position::new(3, 5)..Position::new(3, 9),
                    source_range: 23..27,
                    comment_text: "   a".to_string()
                },
                Comment {
                    position_range: Position::new(6, 13)..Position::new(6, 20),
                    source_range: 56..63,
                    comment_text: "   b   ".to_string()
                },
                Comment {
                    position_range: Position::new(9, 20)..Position::new(9, 27),
                    source_range: 88..95,
                    comment_text: "   c   ".to_string()
                },
            ]
        );

        Ok(())
    }

    #[test]
    fn comment_in_list_item_spans_its_indented_lines() -> anyhow::Result<()> {
        let comments = MdxParser::new()?.comments(
            "- {/* a
  b */}
",
        )?;

        assert_eq!(
            comments,
            vec![Comment {
                position_range: Position::new(1, 3)..Position::new(2, 8),
                source_range: 2..15,
                comment_text: "    a\n  b    ".to_string()
            }]
        );

        Ok(())
    }

    #[test]
    fn frontmatter_is_skipped() -> anyhow::Result<()> {
        // Frontmatter is YAML, where `<3` is plain text. Read as MDX, it would open a JSX tag.
        let comments = MdxParser::new()?.comments(
            "---
title: <3 # not a comment
---

{/* after the frontmatter */}
",
        )?;

        assert_eq!(
            comments,
            vec![Comment {
                position_range: Position::new(5, 1)..Position::new(5, 30),
                source_range: 35..64,
                comment_text: "    after the frontmatter    ".to_string()
            }]
        );

        Ok(())
    }

    // <block affects="docs/symbols.md:markdown-paths, src/skill.md#/blockwatch/Symbols/Markdown">
    #[test]
    fn document_with_esm_and_jsx_parse_symbols_returns_its_sections() -> anyhow::Result<()> {
        let source = r"import {Badge} from './badge.js'

# Colors

<Note>
## Inside JSX
</Note>

## Red <Badge>new</Badge> {version}

Warm.
";

        let paths: Vec<String> = parser()?
            .parse_symbols(source)?
            .iter()
            .map(|symbol| symbol.path.to_string())
            .collect();

        assert_eq!(paths, ["/Colors", "/Colors/Red new version"]);
        Ok(())
    }

    #[test]
    fn invalid_mdx_parse_symbols_returns_error() -> anyhow::Result<()> {
        // MDX 2 and later have no HTML comments.
        let error = parser()?
            .parse_symbols("# Colors\n\n<!-- a comment -->\n")
            .unwrap_err();

        assert!(
            error.to_string().contains("Unexpected character `!`"),
            "unexpected error: {error}"
        );
        Ok(())
    }
    // </block>
}
