use crate::parsers::{
    BlocksFromCommentsParser, BlocksParser, CommentsParser, TreeSitterCommentsParser,
    c_style_multiline_comment_processor,
};
use tree_sitter::Query;

/// Returns a [`BlocksParser`] for SQL.
pub(super) fn parser() -> anyhow::Result<Box<dyn BlocksParser>> {
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
                Some(|_, comment| Some(comment.replacen("--", "  ", 1))),
            ),
            (
                block_comment_query,
                Some(|_, comment| Some(c_style_multiline_comment_processor(comment))),
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
                Comment {
                    source_line_number: 3,
                    source_start_position: 21,
                    source_end_position: 53,
                    comment_text: "   This is a single line comment".to_string()
                },
                Comment {
                    source_line_number: 4,
                    source_start_position: 69,
                    source_end_position: 97,
                    comment_text: "   This is an inline comment".to_string()
                },
                Comment {
                    source_line_number: 6,
                    source_start_position: 99,
                    source_end_position: 122,
                    comment_text: "   This is a multi-line".to_string()
                },
                Comment {
                    source_line_number: 7,
                    source_start_position: 123,
                    source_end_position: 144,
                    comment_text: "   comment that spans".to_string()
                },
                Comment {
                    source_line_number: 8,
                    source_start_position: 145,
                    source_end_position: 161,
                    comment_text: "   several lines".to_string()
                },
                Comment {
                    source_line_number: 10,
                    source_start_position: 163,
                    source_end_position: 219,
                    comment_text: "   This is a block comment \nthat spans multiple lines\n  "
                        .to_string()
                },
                Comment {
                    source_line_number: 14,
                    source_start_position: 250,
                    source_end_position: 276,
                    comment_text: "   Inline block comment   ".to_string()
                }
            ]
        );

        Ok(())
    }
}
