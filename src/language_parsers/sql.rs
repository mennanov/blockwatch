use crate::block_parser::{BlocksFromCommentsParser, BlocksParser};
use crate::language_parsers::{
    CommentsParser, TreeSitterCommentsParser, c_style_multiline_comment_processor,
};

/// Returns a [`BlocksParser`] for SQL.
pub(super) fn parser() -> anyhow::Result<impl BlocksParser> {
    Ok(BlocksFromCommentsParser::new(comments_parser()?))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let sql_language = tree_sitter_sequel::LANGUAGE.into();
    let parser = TreeSitterCommentsParser::new(
        &sql_language,
        Box::new(|node, source_code| match node.kind() {
            "comment" => Some(source_code[node.byte_range()].replacen("--", "  ", 1)),
            "marginalia" => Some(c_style_multiline_comment_processor(
                &source_code[node.byte_range()],
            )),
            _ => None,
        }),
    );
    Ok(parser)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Position, language_parsers::Comment};

    #[test]
    fn parses_comments_correctly() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser()?;

        let blocks: Vec<Comment> = comments_parser
            .parse(
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
            )
            .collect();

        assert_eq!(
            blocks,
            vec![
                Comment {
                    position_range: Position::new(3, 1)..Position::new(3, 33),
                    source_range: 21..53,
                    comment_text: "   This is a single line comment".to_string()
                },
                Comment {
                    position_range: Position::new(4, 16)..Position::new(4, 44),
                    source_range: 69..97,
                    comment_text: "   This is an inline comment".to_string()
                },
                Comment {
                    position_range: Position::new(6, 1)..Position::new(6, 24),
                    source_range: 99..122,
                    comment_text: "   This is a multi-line".to_string()
                },
                Comment {
                    position_range: Position::new(7, 1)..Position::new(7, 22),
                    source_range: 123..144,
                    comment_text: "   comment that spans".to_string()
                },
                Comment {
                    position_range: Position::new(8, 1)..Position::new(8, 17),
                    source_range: 145..161,
                    comment_text: "   several lines".to_string()
                },
                Comment {
                    position_range: Position::new(10, 1)..Position::new(12, 3),
                    source_range: 163..219,
                    comment_text: "   This is a block comment \nthat spans multiple lines\n  "
                        .to_string()
                },
                Comment {
                    position_range: Position::new(14, 30)..Position::new(14, 56),
                    source_range: 250..276,
                    comment_text: "   Inline block comment   ".to_string()
                }
            ]
        );

        Ok(())
    }
}
