use crate::block_parser::{BlocksFromCommentsParser, BlocksParser};
use crate::language_parsers::{
    Comment, CommentsParser, TreeSitterCommentsParser, blank_preserving_line_breaks,
    comment_from_node, offset_comment, xml_style_comments_parser,
};
use tree_sitter::StreamingIterator;

/// Returns a [`BlocksParser`] for Markdown.
pub(super) fn parser() -> anyhow::Result<impl BlocksParser> {
    Ok(BlocksFromCommentsParser::new(MdCommentsParser::new()))
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
    fn new() -> Self {
        let markdown_lang: tree_sitter::Language = tree_sitter_md::LANGUAGE.into();
        let mut md_tree_sitter_parser = tree_sitter::Parser::new();
        md_tree_sitter_parser
            .set_language(&markdown_lang)
            .expect("Error setting Tree-sitter language");
        let md_regions_query = tree_sitter::Query::new(
            &markdown_lang,
            "(html_block) @html_block \
             (inline) @inline \
             (link_reference_definition) @link_reference_definition",
        )
        .unwrap();

        let inline_lang: tree_sitter::Language = tree_sitter_md::INLINE_LANGUAGE.into();
        let mut inline_tree_sitter_parser = tree_sitter::Parser::new();
        inline_tree_sitter_parser
            .set_language(&inline_lang)
            .expect("Error setting Tree-sitter language");
        let code_span_query =
            tree_sitter::Query::new(&inline_lang, "(code_span) @code_span").unwrap();

        let html_lang = tree_sitter_html::LANGUAGE.into();
        let html_comments_parser = xml_style_comments_parser(&html_lang, "comment");
        Self {
            md_tree_sitter_parser,
            md_regions_query,
            inline_tree_sitter_parser,
            code_span_query,
            html_comments_parser,
        }
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
                .captures
                .first()
                .expect("Empty Tree-sitter region query match")
                .node;
            let region = &contents[node.byte_range()];

            // `[//]:` comments are direct nodes of the Markdown tree, so their positions are
            // already the source positions.
            if node.kind() == "link_reference_definition" {
                if let Some(text) = link_reference_definition_comment_text(region) {
                    comments.push(comment_from_node(&node, text));
                }
                continue;
            }

            // The remaining regions carry HTML comments, which require a `<!--`.
            if !region.contains("<!--") {
                continue;
            }
            // Inline regions may contain code spans whose contents must not be mistaken for
            // comments, but a code span requires a backtick; without one the region is parsed
            // as is.
            let html_comments: Vec<Comment> = if node.kind() == "inline" && region.contains('`') {
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
                offset_comment(&mut comment, &node);
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
                .captures
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

/// Extracts the content of a Markdown `[//]:` comment (a link reference definition used as a
/// comment), blanking the `[//]:` prefix and the title delimiters so the length is preserved.
/// Returns `None` for a link reference definition that is not a `[//]:` comment.
fn link_reference_definition_comment_text(comment: &str) -> Option<String> {
    let prefix_idx = comment.find("[//]:")?;
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

    let mut result = String::with_capacity(comment.len());
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
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks::Block;
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
        let blocks = parser.parse(content)?;

        assert_eq!(
            blocks,
            vec![Block::new(
                HashMap::from([("name".to_string(), "inline_block".to_string())]),
                Position::new(4, 16)..=Position::new(4, 42),
                test_utils::substr_range(content, " and\nmore content here\nending text "),
                Position::new(4, 47)..Position::new(6, 13),
            )]
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
        let blocks = parser.parse(content)?;

        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].attributes["name"], "mixed");

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
        let blocks = parser.parse(content)?;

        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].attributes["name"], "a");
        assert!(blocks[0].content(content).contains("First block content"));
        assert_eq!(blocks[1].attributes["name"], "b");
        assert!(blocks[1].content(content).contains("Second block content"));

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
