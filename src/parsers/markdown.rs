use crate::parsers::{
    BlocksFromCommentsParser, BlocksParser, CommentsParser, TreeSitterCommentsParser,
};
use tree_sitter::Query;

/// Returns a [`BlocksParser`] for Markdown.
pub(crate) fn parser() -> anyhow::Result<Box<dyn BlocksParser>> {
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
    let parser = TreeSitterCommentsParser::<fn(usize, &str) -> Option<String>>::new(
        markdown_lang,
        vec![(
            block_comment_query,
            Some(|capture_idx, comment| {
                if capture_idx != 1 {
                    return None;
                }
                Some(
                    comment
                        .trim_start()
                        .strip_prefix("[//]:")
                        .expect("Expected a block comment to start with '[//]:'")
                        .trim()
                        .trim_start_matches("#")
                        .trim_start()
                        .trim_start_matches("(")
                        .trim_end_matches(")")
                        .trim()
                        .to_string(),
                )
            }),
        )],
    );
    Ok(parser)
}

#[cfg(test)]
mod tests {
    use super::*;

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
                (5, "This is a markdown comment".to_string()),
                (6, "Another markdown comment".to_string()),
                (10, "Third comment with\nmultiple lines\nin it".to_string()),
            ]
        );

        Ok(())
    }
}
