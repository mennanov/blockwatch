use crate::block_parser::{BlocksFromCommentsParser, BlocksParser};
use crate::language_parsers;
use crate::language_parsers::CommentsParser;

/// Returns a [`BlocksParser`] for C++.
pub(super) fn parser() -> anyhow::Result<impl BlocksParser> {
    Ok(BlocksFromCommentsParser::new(comments_parser()?))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let cpp_language = tree_sitter_cpp::LANGUAGE.into();
    let parser = language_parsers::c_style_comments_parser(&cpp_language, "comment");
    Ok(parser)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Position, language_parsers::Comment};

    #[test]
    fn parses_cpp_comments_correctly() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser()?;

        let blocks: Vec<Comment> = comments_parser
            .parse(
                r#"
            // This is a single-line comment in C++.
            #include <iostream>

            /*
             * This is a multi-line comment.
             * It spans multiple lines.
             */

            int main() {
                std::cout << "Hello, C++!" << std::endl; // Prints a message to the console.

                /* This is another
                 * multi-line
                 * comment.
                 */
                
                return 0;
            }
            "#,
            )
            .collect();

        assert_eq!(
            blocks,
            vec![
                Comment {
                    position_range: Position::new(2, 13)..Position::new(2, 53),
                    source_range: 13..53,
                    comment_text: "   This is a single-line comment in C++.".to_string()
                },
                Comment {
                    position_range: Position::new(5, 13)..Position::new(8, 16),
                    source_range: 99..202,
                    comment_text: "  \n               This is a multi-line comment.\n               It spans multiple lines.\n               "
                        .to_string()
                },
                Comment {
                    position_range: Position::new(11, 58)..Position::new(11, 93),
                    source_range: 286..321,
                    comment_text: "   Prints a message to the console.".to_string()
                },
                Comment {
                    position_range: Position::new(13, 17)..Position::new(16, 20),
                    source_range: 339..435,
                    comment_text: "   This is another\n                   multi-line\n                   comment.\n                   ".to_string()
                },
            ]
        );

        Ok(())
    }
}
