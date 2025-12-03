use crate::block_parser::{BlocksFromCommentsParser, BlocksParser};
use crate::language_parsers;
use crate::language_parsers::CommentsParser;
use tree_sitter::Query;

/// Returns a [`BlocksParser`] for C.
pub(super) fn parser() -> anyhow::Result<impl BlocksParser> {
    Ok(BlocksFromCommentsParser::new(comments_parser()?))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let cpp_language = tree_sitter_c::LANGUAGE.into();
    let comment_query = Query::new(&cpp_language, "(comment) @comment")?;
    let parser = language_parsers::c_style_comments_parser(cpp_language, comment_query);
    Ok(parser)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::language_parsers::Comment;

    #[test]
    fn parses_c_comments_correctly() -> anyhow::Result<()> {
        let comments_parser = comments_parser()?;

        let blocks = comments_parser.parse(
            r#"
            // This is a single-line comment in C.
            #include <stdio.h>

            /*
             * This is a multi-line comment.
             * It spans multiple lines in C.
             */

            int main() {
                printf("Hello, C!\n"); // Prints a message to the console.

                /* Another comment
                 * split into
                 * multiple lines.
                 */
                 
                return 0;
            }
            "#,
        )?;

        assert_eq!(
            blocks,
            vec![
                Comment {
                    source_line_number: 2,
                    source_start_position: 13,
                    source_end_position: 51,
                    comment_text: "   This is a single-line comment in C.".to_string()
                },
                Comment {
                    source_line_number: 5,
                    source_start_position: 96,
                    source_end_position: 204,
                    comment_text:
                        "  \n               This is a multi-line comment.\n               It spans multiple lines in C.\n               "
                            .to_string()
                },
                Comment {
                    source_line_number: 11,
                    source_start_position: 270,
                    source_end_position: 305,
                    comment_text: "   Prints a message to the console.".to_string()
                },
                Comment {
                    source_line_number: 13,
                    source_start_position: 323,
                    source_end_position: 426,
                    comment_text: "   Another comment\n                   split into\n                   multiple lines.\n                   ".to_string()
                },
            ]
        );

        Ok(())
    }
}
