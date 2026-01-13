use crate::block_parser::{BlocksFromCommentsParser, BlocksParser};
use crate::language_parsers;
use crate::language_parsers::CommentsParser;
use tree_sitter::Query;

/// Returns a [`BlocksParser`] for JavaScript.
pub(super) fn parser() -> anyhow::Result<impl BlocksParser> {
    Ok(BlocksFromCommentsParser::new(comments_parser()?))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let js_language = tree_sitter_javascript::LANGUAGE.into();
    let block_comment_query = Query::new(&js_language, "(comment) @comment")?;
    let parser = language_parsers::c_style_comments_parser(js_language, block_comment_query);
    Ok(parser)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Position, language_parsers::Comment};

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
                Comment {
                    start_position: Position::new(2, 13),
                    end_position: Position::new(6, 16),
                    source_range: 13..156,
                    comment_text: "   \n               This is a JavaScript function demonstration with comments.\n              \n               @author Author name\n               ".to_string()
                },
                Comment {
                    start_position: Position::new(8, 17),
                    end_position: Position::new(8, 64),
                    source_range: 206..253,
                    comment_text: "   This is a single-line comment in JavaScript."
                        .to_string()
                },
                Comment {
                    start_position: Position::new(9, 52),
                    end_position: Position::new(9, 88),
                    source_range: 305..341,
                    comment_text: "   Inline comment after a statement.".to_string()
                },
                Comment {
                    start_position: Position::new(11, 17),
                    end_position: Position::new(14, 20),
                    source_range: 359..479,
                    comment_text: "  \n                   This is a multi-line comment.\n                   It also spans multiple lines.\n                   "
                        .to_string()
                },
                Comment {
                    start_position: Position::new(15, 34),
                    end_position: Position::new(15, 65),
                    source_range: 513..544,
                    comment_text: "   Inline multi-line comment   ".to_string()
                },
            ]
        );

        Ok(())
    }
}
