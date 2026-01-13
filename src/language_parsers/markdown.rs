use crate::Position;
use crate::block_parser::{BlocksFromCommentsParser, BlocksParser};
use crate::blocks::Block;
use crate::language_parsers::{CommentsParser, TreeSitterCommentsParser, html};
use anyhow::Context;
use itertools::Itertools;
use std::ops::RangeInclusive;
use tree_sitter::{Query, StreamingIterator};

/// Returns a [`BlocksParser`] for Markdown.
pub(super) fn parser() -> anyhow::Result<impl BlocksParser> {
    let md_blocks_parser = BlocksFromCommentsParser::new(markdown_comments_parser()?);
    let html_blocks_parser = html::parser()?;
    Ok(MdParser::new(md_blocks_parser, html_blocks_parser))
}

/// Parses Markdown and HTML comments from Markdown.
///
/// HTML comments are parsed from valid [HTML blocks](https://github.github.com/gfm/#html-block).
struct MdParser<C: CommentsParser, HtmlParser: BlocksParser> {
    md_parser: BlocksFromCommentsParser<C>,
    html_parser: HtmlParser,
}

impl<C: CommentsParser, HtmlParser: BlocksParser> MdParser<C, HtmlParser> {
    fn new(md_parser: BlocksFromCommentsParser<C>, html_parser: HtmlParser) -> Self {
        Self {
            md_parser,
            html_parser,
        }
    }

    fn parse_html_blocks(&self, contents: &str) -> anyhow::Result<Vec<Block>> {
        let mut parser = tree_sitter::Parser::new();
        let markdown_lang = tree_sitter_md::LANGUAGE.into();
        parser
            .set_language(&markdown_lang)
            .expect("Error setting Tree-sitter language");
        let tree = parser.parse(contents, None).unwrap();
        let root_node = tree.root_node();
        let query = Query::new(&markdown_lang, "(html_block) @html_block").unwrap();
        let mut query_cursor = tree_sitter::QueryCursor::new();
        let mut matches = query_cursor.matches(&query, root_node, contents.as_bytes());
        let mut html_blocks = Vec::new();
        while let Some(query_match) = matches.next() {
            let capture = query_match
                .captures
                .first()
                .context("Empty Tree-sitter html_block query match")?;
            let node = capture.node;
            let html_block = &contents[node.start_byte()..node.end_byte()];
            let blocks = self
                .html_parser
                .parse(html_block)?
                .into_iter()
                .map(|block| {
                    Self::block_with_offsets(block, node.start_position().row, node.start_byte())
                });
            html_blocks.extend(blocks);
        }
        Ok(html_blocks)
    }

    fn block_with_offsets(mut block: Block, line_offset: usize, byte_offset: usize) -> Block {
        block.start_tag_position_range = RangeInclusive::new(
            Position::new(
                line_offset + block.start_tag_position_range.start().line,
                block.start_tag_position_range.start().character,
            ),
            Position::new(
                line_offset + block.start_tag_position_range.end().line,
                block.start_tag_position_range.end().character,
            ),
        );
        block.content_bytes_range.start += byte_offset;
        block.content_bytes_range.end += byte_offset;
        block.content_position_range.start.line += line_offset;
        block.content_position_range.end.line += line_offset;
        block
    }
}

impl<C: CommentsParser, HtmlParser: BlocksParser> BlocksParser for MdParser<C, HtmlParser> {
    fn parse(&self, contents: &str) -> anyhow::Result<Vec<Block>> {
        let md_blocks = self.md_parser.parse(contents)?;
        let html_blocks = self.parse_html_blocks(contents)?;

        Ok(md_blocks.into_iter().merge(html_blocks).collect())
    }
}

fn markdown_comments_parser() -> anyhow::Result<impl CommentsParser> {
    let markdown_lang = tree_sitter_md::LANGUAGE.into();
    let block_comment_query = Query::new(
        &markdown_lang,
        r#"(link_reference_definition
             (link_label) @comment_marker
             (#eq? @comment_marker "[//]")
         ) @comment"#,
    )?;

    let parser = TreeSitterCommentsParser::new(
        markdown_lang,
        vec![(
            block_comment_query,
            Some(Box::new(|capture_idx, comment, _node| {
                if capture_idx != 1 {
                    return Ok(None);
                }
                let mut result = String::with_capacity(comment.len());
                let prefix_idx = comment
                    .find("[//]:")
                    .expect("comment is expected to start with '[//]:'");

                let start_search = prefix_idx + 5;
                let open_idx = comment[start_search..]
                    .find(|c| ['(', '"', '\''].contains(&c))
                    .map(|i| i + start_search)
                    .expect("comment is expected to have a title delimiter");

                let open_char = comment.chars().nth(open_idx).unwrap();
                let close_char = match open_char {
                    '(' => ')',
                    '"' => '"',
                    '\'' => '\'',
                    _ => unreachable!(),
                };

                let close_idx = comment
                    .rfind(close_char)
                    .expect("comment is expected to end with matching delimiter");

                result.push_str(&comment[..prefix_idx]);
                // Replace "[//]:" with spaces.
                result.push_str("     ");
                // Replace everything before the open delimiter with spaces (including the delimiter).
                result.push_str(" ".repeat(open_idx - (prefix_idx + 5) + 1).as_str());
                // Copy the comment's content.
                result.push_str(&comment[open_idx + 1..close_idx]);
                // Replace the close delimiter with a space.
                result.push(' ');
                if close_idx + 1 < comment.len() {
                    // Copy the rest of the comment after the close delimiter.
                    result.push_str(&comment[close_idx + 1..]);
                }
                Ok(Some(result))
            })),
        )],
    );
    Ok(parser)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils;
    use std::collections::HashMap;

    #[test]
    fn parses_markdown_blocks_correctly() -> anyhow::Result<()> {
        let parser = parser()?;

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
        let blocks = parser.parse(content)?;

        assert_eq!(
            blocks,
            vec![
                Block::new(
                    HashMap::from([("name".to_string(), "md_block".to_string())]),
                    Position::new(5, 10)..=Position::new(5, 32),
                    test_utils::substr_range(content, "Some text here\n\n"),
                    Position::new(6, 1)..Position::new(8, 1),
                ),
                Block::new(
                    HashMap::from([("name".to_string(), "md_block_2".to_string())]),
                    Position::new(10, 10)..=Position::new(10, 34),
                    test_utils::substr_range(
                        content,
                        "Some text here 2\n\n[//]: # (<block name=\"md_block_3\">)\nSome text here 3\n\n[//]: # (</block>)\n"
                    ),
                    Position::new(11, 1)..Position::new(17, 1),
                ),
                Block::new(
                    HashMap::from([("name".to_string(), "md_block_3".to_string())]),
                    Position::new(13, 10)..=Position::new(13, 34),
                    test_utils::substr_range(content, "Some text here 3\n\n"),
                    Position::new(14, 1)..Position::new(16, 1),
                )
            ]
        );

        Ok(())
    }

    #[test]
    fn parses_html_blocks_correctly() -> anyhow::Result<()> {
        let parser = parser()?;

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
"#;
        let blocks = parser.parse(content)?;

        assert_eq!(
            blocks,
            vec![
                Block::new(
                    HashMap::from([("name".to_string(), "html_block".to_string())]),
                    Position::new(5, 6)..=Position::new(5, 30),
                    test_utils::substr_range(content, "\nSome html content\n"),
                    Position::new(5, 35)..Position::new(7, 1),
                ),
                Block::new(
                    HashMap::from([("name".to_string(), "md_block".to_string())]),
                    Position::new(10, 10)..=Position::new(10, 32),
                    test_utils::substr_range(content, "Some markdown content\n\n"),
                    Position::new(11, 1)..Position::new(13, 1),
                ),
                Block::new(
                    HashMap::from([("name".to_string(), "html_block2".to_string())]),
                    Position::new(15, 6)..=Position::new(15, 31),
                    test_utils::substr_range(content, "Not wrapped in HTML tags"),
                    Position::new(15, 36)..Position::new(15, 60),
                ),
            ]
        );

        Ok(())
    }
}
