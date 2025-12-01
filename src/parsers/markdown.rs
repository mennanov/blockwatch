use crate::parsers::{
    BlocksFromCommentsParser, BlocksParser, CommentsParser, TreeSitterCommentsParser, html,
};
use tree_sitter::Query;

/// Returns a [`BlocksParser`] for Markdown.
pub(super) fn parser() -> anyhow::Result<Box<dyn BlocksParser>> {
    Ok(Box::new(BlocksFromCommentsParser::new(comments_parser()?)))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let markdown_lang = tree_sitter_md::LANGUAGE.into();
    let block_comment_query = Query::new(
        &markdown_lang,
        r#"(link_reference_definition
             (link_label) @comment_marker
             (#eq? @comment_marker "[//]")
         ) @comment"#,
    )?;
    let html_comments_parser = html::parser()?;
    let html_comment_query = Query::new(&markdown_lang, r#"(html_block) @comment"#)?;
    let parser = TreeSitterCommentsParser::new(
        markdown_lang,
        vec![
            (
                block_comment_query,
                Some(|capture_idx, comment| {
                    if capture_idx != 1 {
                        return None;
                    }
                    let mut result = String::with_capacity(comment.len());
                    let prefix_idx = comment
                        .find("[//]:")
                        .expect("comment is expected to start with '[//]:'");
                    let open_idx = comment
                        .find("(")
                        .expect("comment is expected to start with '('");
                    let close_idx = comment
                        .rfind(")")
                        .expect("comment is expected to end with ')'");
                    result.push_str(&comment[..prefix_idx]);
                    // Replace "[//]:" with spaces.
                    result.push_str("     ");
                    // Replace everything before "(" with spaces (including the "(").
                    result.push_str(" ".repeat(open_idx - (prefix_idx + 5) + 1).as_str());
                    // Copy the comment's content.
                    result.push_str(&comment[open_idx + 1..close_idx]);
                    // Replace ")" with a space.
                    result.push(' ');
                    if close_idx + 1 < comment.len() {
                        result.push_str(&comment[close_idx + 1..]);
                    }
                    Some(result)
                }),
            ),
            (
                html_comment_query,
                Some(|_, comment| {
                    println!("HTML comment: {}", comment);
                    let comments = html_comments_parser.parse(comment);
                    return None;
                }),
            ),
        ],
    );
    Ok(parser)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsers::Comment;

    #[test]
    fn parses_markdown_comments_correctly() -> anyhow::Result<()> {
        let comments_parser = comments_parser()?;

        let blocks = comments_parser.parse(
            r#"
# Header
[foo]: /url "title"

[//]: # (This is a markdown comment)
[//]: # (Another markdown comment )

Some text here

[//]: # (Third comment with
multiple lines
in it)"#,
        )?;

        assert_eq!(
            blocks,
            vec![
                Comment {
                    source_line_number: 5,
                    source_start_position: 31,
                    source_end_position: 68,
                    comment_text: "         This is a markdown comment \n".to_string()
                },
                Comment {
                    source_line_number: 6,
                    source_start_position: 68,
                    source_end_position: 104,
                    comment_text: "         Another markdown comment  \n".to_string()
                },
                Comment {
                    source_line_number: 10,
                    source_start_position: 121,
                    source_end_position: 170,
                    comment_text: "         Third comment with\nmultiple lines\nin it ".to_string()
                },
            ]
        );

        Ok(())
    }

    #[test]
    fn parses_html_style_comments_correctly() -> anyhow::Result<()> {
        let comments_parser = comments_parser()?;

        let blocks = comments_parser.parse(
            r#"
# This is a Markdown header

<div>
<!-- This is an html comment -->
</div>
<!-- Another html comment -->

Some text here

<!-- Third comment with
multiple lines
in it -->"#,
        )?;

        assert_eq!(
            blocks,
            vec![
                Comment {
                    source_line_number: 5,
                    source_start_position: 36,
                    source_end_position: 67,
                    comment_text: "      This is an html comment     \n".to_string()
                },
                Comment {
                    source_line_number: 7,
                    source_start_position: 76,
                    source_end_position: 104,
                    comment_text: "      Another html comment     \n".to_string()
                },
                Comment {
                    source_line_number: 11,
                    source_start_position: 123,
                    source_end_position: 170,
                    comment_text: "      Third comment with\nmultiple lines\nin it     "
                        .to_string()
                },
            ]
        );

        Ok(())
    }
}
