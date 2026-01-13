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
    use crate::{Position, language_parsers::Comment};

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
                    start_position: Position::new(2, 13),
                    end_position: Position::new(2, 51),
                    source_range: 13..51,
                    comment_text: "   This is a single-line comment in C.".to_string()
                },
                Comment {
                    start_position: Position::new(5, 13),
                    end_position: Position::new(8, 16),
                    source_range: 96..204,
                    comment_text:
                        "  \n               This is a multi-line comment.\n               It spans multiple lines in C.\n               "
                            .to_string()
                },
                Comment {
                    start_position: Position::new(11, 40),
                    end_position: Position::new(11, 75),
                    source_range: 270..305,
                    comment_text: "   Prints a message to the console.".to_string()
                },
                Comment {
                    start_position: Position::new(13, 17),
                    end_position: Position::new(16, 20),
                    source_range: 323..426,
                    comment_text: "   Another comment\n                   split into\n                   multiple lines.\n                   ".to_string()
                },
            ]
        );

        Ok(())
    }
}
