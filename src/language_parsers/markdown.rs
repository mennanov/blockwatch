use crate::block_parser::{BlocksFromCommentsParser, BlocksParser, parse_blocks_from_comments};
use crate::blocks::Block;
use crate::language_parsers::{Comment, CommentsParser, TreeSitterCommentsParser};
use anyhow::Context;
use itertools::Itertools;
use tree_sitter::StreamingIterator;

/// Returns a [`BlocksParser`] for Markdown.
pub(super) fn parser() -> anyhow::Result<impl BlocksParser> {
    let md_blocks_parser = BlocksFromCommentsParser::new(markdown_comments_parser()?);
    Ok(MdParser::new(md_blocks_parser))
}

/// Parses Markdown and HTML comments from Markdown.
///
/// HTML comments are parsed from valid [HTML blocks](https://github.github.com/gfm/#html-block).
struct MdParser<C: CommentsParser> {
    md_blocks_parser: BlocksFromCommentsParser<C>,
    md_tree_sitter_parser: tree_sitter::Parser,
    md_html_blocks_query: tree_sitter::Query,
    html_comments_parser: TreeSitterCommentsParser,
}

impl<C: CommentsParser> MdParser<C> {
    fn new(md_parser: BlocksFromCommentsParser<C>) -> Self {
        let mut md_tree_sitter_parser = tree_sitter::Parser::new();
        let markdown_lang = tree_sitter_md::LANGUAGE.into();
        md_tree_sitter_parser
            .set_language(&markdown_lang)
            .expect("Error setting Tree-sitter language");
        let md_html_blocks_query =
            tree_sitter::Query::new(&markdown_lang, "(html_block) @html_block").unwrap();

        let html_lang = tree_sitter_html::LANGUAGE.into();
        let html_comment_query = tree_sitter::Query::new(&html_lang, "(comment) @comment").unwrap();
        let html_comments_parser =
            TreeSitterCommentsParser::new(&html_lang, vec![(html_comment_query, None)]);
        Self {
            md_blocks_parser: md_parser,
            md_tree_sitter_parser,
            md_html_blocks_query,
            html_comments_parser,
        }
    }

    fn parse_html_blocks(&mut self, contents: &str) -> anyhow::Result<Vec<Block>> {
        let html_comments = self.parse_html_comments(contents)?;
        parse_blocks_from_comments(html_comments.iter())
    }

    fn parse_html_comments(&mut self, contents: &str) -> anyhow::Result<Vec<Comment>> {
        let tree = self.md_tree_sitter_parser.parse(contents, None).unwrap();
        let root_node = tree.root_node();
        let mut query_cursor = tree_sitter::QueryCursor::new();
        let mut matches =
            query_cursor.matches(&self.md_html_blocks_query, root_node, contents.as_bytes());
        let mut all_html_comments = Vec::new();
        while let Some(query_match) = matches.next() {
            let capture = query_match
                .captures
                .first()
                .context("Empty Tree-sitter html_block query match")?;
            let node = capture.node;
            let html_block = &contents[node.start_byte()..node.end_byte()];

            let mut html_comments = self.html_comments_parser.parse(html_block)?;
            for comment in &mut html_comments {
                comment.position_range.start.line += node.start_position().row;
                comment.position_range.end.line += node.start_position().row;
                comment.source_range.start += node.start_byte();
                comment.source_range.end += node.start_byte();
            }
            all_html_comments.extend(html_comments);
        }
        Ok(all_html_comments)
    }
}

impl<C: CommentsParser> BlocksParser for MdParser<C> {
    fn parse(&mut self, contents: &str) -> anyhow::Result<Vec<Block>> {
        let md_blocks = self.md_blocks_parser.parse(contents)?;
        let html_blocks = self.parse_html_blocks(contents)?;

        Ok(md_blocks.into_iter().merge(html_blocks).collect())
    }
}

fn markdown_comments_parser() -> anyhow::Result<impl CommentsParser> {
    let markdown_lang = tree_sitter_md::LANGUAGE.into();
    let block_comment_query = tree_sitter::Query::new(
        &markdown_lang,
        r#"(link_reference_definition
             (link_label) @comment_marker
             (#eq? @comment_marker "[//]")
         ) @comment"#,
    )?;

    let parser = TreeSitterCommentsParser::new(
        &markdown_lang,
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
    use crate::{Position, test_utils};
    use std::collections::HashMap;

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
                Block::new(
                    HashMap::from([("name".to_string(), "html_block3".to_string())]),
                    Position::new(17, 6)..=Position::new(17, 31),
                    test_utils::substr_range(
                        content,
                        "\nNot wrapped in HTML tags on multiple lines\n"
                    ),
                    Position::new(17, 36)..Position::new(19, 1),
                ),
            ]
        );

        Ok(())
    }
}
