use crate::parsers;
use crate::parsers::{BlocksFromCommentsParser, BlocksParser, CommentsParser};
use tree_sitter::Query;

/// Returns a [`BlocksParser`] for TypeScript.
pub(super) fn parser() -> anyhow::Result<Box<dyn BlocksParser>> {
    Ok(Box::new(BlocksFromCommentsParser::new(comments_parser()?)))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let ts_language = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
    let block_comment_query = Query::new(&ts_language, "(comment) @comment")?;
    let parser = parsers::c_style_comments_parser(ts_language, block_comment_query);
    Ok(parser)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsers::Comment;

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
                    source_line_number: 2,
                    source_start_position: 13,
                    source_end_position: 142,
                    comment_text:
                        "   \n               This is a TypeScript class example with comments.\n              \n               @class Example\n               "
                            .to_string()
                },
                Comment {
                    source_line_number: 8,
                    source_start_position: 187,
                    source_end_position: 234,
                    comment_text: "   This is a single-line comment in TypeScript.".to_string()
                },
                Comment {
                    source_line_number: 11,
                    source_start_position: 291,
                    source_end_position: 413,
                    comment_text:
                        "  \n                   This is a multi-line comment\n                   that spans across several lines.\n                   "
                            .to_string()
                },
                Comment {
                    source_line_number: 16,
                    source_start_position: 499,
                    source_end_position: 530,
                    comment_text: "   Inline multi-line comment   ".to_string()
                },
                Comment {
                    source_line_number: 19,
                    source_start_position: 566,
                    source_end_position: 592,
                    comment_text: "   Method to get the value".to_string()
                },
                Comment {
                    source_line_number: 21,
                    source_start_position: 676,
                    source_end_position: 720,
                    comment_text: "   Inline comment next to a return statement".to_string()
                }
            ]
        );

        Ok(())
    }
}
