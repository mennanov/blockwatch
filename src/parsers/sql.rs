use crate::parsers::{
    BlocksFromCommentsParser, BlocksParser, CommentsParser, TreeSitterCommentsParser,
};
use tree_sitter::Query;

/// Returns a [`BlocksParser`] for SQL.
pub(crate) fn parser() -> anyhow::Result<Box<dyn BlocksParser>> {
    Ok(Box::new(BlocksFromCommentsParser::new(comments_parser()?)))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let sql_language = tree_sitter_sequel::LANGUAGE.into();
    let line_comment_query = Query::new(&sql_language, "(comment) @comment")?;
    let block_comment_query = Query::new(&sql_language, "(marginalia) @comment")?;
    let parser = TreeSitterCommentsParser::<fn(usize, &str) -> Option<String>>::new(
        sql_language,
        vec![
            (
                line_comment_query,
                Some(|_, comment| Some(comment.strip_prefix("--").unwrap().trim().to_string())),
            ),
            (
                block_comment_query,
                Some(|_, comment| {
                    Some(
                        comment
                            .strip_prefix("/*")
                            .unwrap()
                            .lines()
                            .map(|line| line.trim_start().trim_end_matches("*/").trim())
                            .collect::<Vec<_>>()
                            .join("\n"),
                    )
                }),
            ),
        ],
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
SELECT * FROM users
-- This is a single line comment
WHERE id = 1;  -- This is an inline comment

-- This is a multi-line
-- comment that spans
-- several lines

/* This is a block comment 
that spans multiple lines
*/

SELECT COUNT(*) FROM orders; /* Inline block comment */
"#,
        )?;

        assert_eq!(
            blocks,
            vec![
                (3, "This is a single line comment".to_string()),
                (4, "This is an inline comment".to_string()),
                (6, "This is a multi-line".to_string()),
                (7, "comment that spans".to_string()),
                (8, "several lines".to_string()),
                (
                    10,
                    "This is a block comment\nthat spans multiple lines\n".to_string()
                ),
                (14, "Inline block comment".to_string()),
            ]
        );

        Ok(())
    }
}
