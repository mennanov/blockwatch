use crate::parsers::{
    BlocksFromCommentsParser, BlocksParser, CommentsParser, TreeSitterCommentsParser,
};
use tree_sitter::Query;

/// Returns a [`BlocksParser`] for JavaScript.
pub(crate) fn parser() -> anyhow::Result<Box<dyn BlocksParser>> {
    Ok(Box::new(BlocksFromCommentsParser::new(comments_parser()?)))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let js_language = tree_sitter_javascript::LANGUAGE.into();
    let block_comment_query = Query::new(&js_language, "(comment) @comment")?;
    let parser = TreeSitterCommentsParser::<fn(usize, &str) -> Option<String>>::new(
        js_language,
        vec![(
            block_comment_query,
            Some(|_, comment| {
                if comment.starts_with("//") {
                    Some(comment.strip_prefix("//").unwrap().trim().to_string())
                } else {
                    Some(
                        comment
                            .strip_prefix("/*")
                            .unwrap()
                            .lines()
                            .map(|line| {
                                line.trim_start()
                                    .trim_start_matches("*")
                                    .trim()
                                    .trim_end_matches("/")
                                    .trim_end_matches("*")
                                    .trim()
                            })
                            .collect::<Vec<_>>()
                            .join("\n"),
                    )
                }
            }),
        )],
    );
    Ok(parser)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_comments_correctly() -> anyhow::Result<()> {
        let comments_parser = comments_parser()?;

        let blocks = comments_parser.parse(
            r#"
            /**
             * This is a JavaScript function demonstration with comments.
             *
             * @author Author name
             */
            function example() {
                // This is a single-line comment in JavaScript.
                console.log("Hello, JavaScript!"); // Inline comment after a statement.

                /*
                 * This is a multi-line comment.
                 * It also spans multiple lines.
                 */
                let number = 42; /* Inline multi-line comment */
            }
            "#,
        )?;

        assert_eq!(
            blocks,
            vec![
                (2, "\nThis is a JavaScript function demonstration with comments.\n\n@author Author name\n".to_string()),
                (8, "This is a single-line comment in JavaScript.".to_string()),
                (9, "Inline comment after a statement.".to_string()),
                (11, "\nThis is a multi-line comment.\nIt also spans multiple lines.\n".to_string()),
                (15, "Inline multi-line comment".to_string())
            ]
        );

        Ok(())
    }
}
