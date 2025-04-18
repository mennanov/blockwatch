use crate::parsers::{
    BlocksFromCommentsParser, BlocksParser, CommentsParser, TreeSitterCommentsParser,
};
use tree_sitter::Query;

/// Returns a [`BlocksParser`] for HTML.
pub(crate) fn parser() -> anyhow::Result<Box<dyn BlocksParser>> {
    Ok(Box::new(BlocksFromCommentsParser::new(comments_parser()?)))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let html_language = tree_sitter_html::LANGUAGE.into();
    let comment_query = Query::new(&html_language, "(comment) @comment")?;
    let parser = TreeSitterCommentsParser::<fn(usize, &str) -> Option<String>>::new(
        html_language,
        vec![(
            comment_query,
            Some(|_, comment| {
                Some(
                    comment
                        .strip_prefix("<!--")
                        .unwrap_or(comment)
                        .strip_suffix("-->")
                        .unwrap_or(comment)
                        .lines()
                        .map(|line| line.trim())
                        .collect::<Vec<_>>()
                        .join("\n"),
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
    fn parses_html_comments_correctly() -> anyhow::Result<()> {
        let comments_parser = comments_parser()?;

        let blocks = comments_parser.parse(
            r#"<!DOCTYPE html>
            <!-- Simple comment -->
            <div>
                <!-- Another comment -->
                <p>Some text</p>
                <!--
                Multi-line comment
                with multiple lines
                -->
            </div>
            <!-- Final comment -->
            "#,
        )?;

        assert_eq!(
            blocks,
            vec![
                (2, "Simple comment".to_string()),
                (4, "Another comment".to_string()),
                (6, "\nMulti-line comment\nwith multiple lines\n".to_string()),
                (11, "Final comment".to_string()),
            ]
        );

        Ok(())
    }
}
