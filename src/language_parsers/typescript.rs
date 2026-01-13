use crate::block_parser::{BlocksFromCommentsParser, BlocksParser};
use crate::language_parsers;
use crate::language_parsers::CommentsParser;
use tree_sitter::Query;

/// Returns a [`BlocksParser`] for TypeScript.
pub(super) fn parser() -> anyhow::Result<impl BlocksParser> {
    Ok(BlocksFromCommentsParser::new(comments_parser()?))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let ts_language = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
    let block_comment_query = Query::new(&ts_language, "(comment) @comment")?;
    let parser = language_parsers::c_style_comments_parser(ts_language, block_comment_query);
    Ok(parser)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Position, language_parsers::Comment};

    #[test]
    fn parses_typescript_comments_correctly() -> anyhow::Result<()> {
        let comments_parser = comments_parser()?;

        let blocks = comments_parser.parse(
            r#"
            /**
             * This is a TypeScript class example with comments.
             *
             * @class Example
             */
            class Example {
                // This is a single-line comment in TypeScript.
                private value: number;

                /*
                 * This is a multi-line comment
                 * that spans across several lines.
                 */
                constructor(value: number) {
                    this.value = value; /* Inline multi-line comment */
                }

                // Method to get the value
                public getValue(): number {
                    return this.value; // Inline comment next to a return statement
                }
            }
            "#,
        )?;

        assert_eq!(
            blocks,
            vec![
                Comment {
                    position_range: Position::new(2, 13)..Position::new(6, 16),
                    source_range: 13..142,
                    comment_text:
                        "   \n               This is a TypeScript class example with comments.\n              \n               @class Example\n               "
                            .to_string()
                },
                Comment {
                    position_range: Position::new(8, 17)..Position::new(8, 64),
                    source_range: 187..234,
                    comment_text: "   This is a single-line comment in TypeScript.".to_string()
                },
                Comment {
                    position_range: Position::new(11, 17)..Position::new(14, 20),
                    source_range: 291..413,
                    comment_text:
                        "  \n                   This is a multi-line comment\n                   that spans across several lines.\n                   "
                            .to_string()
                },
                Comment {
                    position_range: Position::new(16, 41)..Position::new(16, 72),
                    source_range: 499..530,
                    comment_text: "   Inline multi-line comment   ".to_string()
                },
                Comment {
                    position_range: Position::new(19, 17)..Position::new(19, 43),
                    source_range: 566..592,
                    comment_text: "   Method to get the value".to_string()
                },
                Comment {
                    position_range: Position::new(21, 40)..Position::new(21, 84),
                    source_range: 676..720,
                    comment_text: "   Inline comment next to a return statement".to_string()
                }
            ]
        );

        Ok(())
    }
}
