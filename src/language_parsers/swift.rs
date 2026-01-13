use crate::block_parser::{BlocksFromCommentsParser, BlocksParser};
use crate::language_parsers::{CommentsParser, c_style_line_and_block_comments_parser};
use tree_sitter::Query;

/// Returns a [`BlocksParser`] for Swift.
pub(super) fn parser() -> anyhow::Result<impl BlocksParser> {
    Ok(BlocksFromCommentsParser::new(comments_parser()?))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let swift_language = tree_sitter_swift::LANGUAGE.into();
    let line_comment_query = Query::new(&swift_language, "(comment) @comment")?;
    let block_comment_query = Query::new(&swift_language, "(multiline_comment) @comment")?;
    let parser = c_style_line_and_block_comments_parser(
        swift_language,
        line_comment_query,
        block_comment_query,
    );
    Ok(parser)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Position, language_parsers::Comment};

    #[test]
    fn parses_swift_comments_correctly() -> anyhow::Result<()> {
        let comments_parser = comments_parser()?;

        let blocks = comments_parser.parse(
            r#"
            // This is a single-line comment in Swift.
            import Foundation
    
            /*
             * This is a multi-line comment.
             * It spans multiple lines in Swift.
             */
    
            func main() {
                print("Hello, Swift!") // Prints a message to the console.
    
                /* Another comment
                 * split into
                 * multiple lines.
                 */
                 
                return
            }
            "#,
        )?;

        assert_eq!(
            blocks,
            vec![
                Comment {
                    position_range: Position::new(2, 13)..Position::new(2, 55),
                    source_range: 13..55,
                    comment_text: "   This is a single-line comment in Swift.".to_string()
                },
                Comment {
                    position_range: Position::new(5, 13)..Position::new(8, 16),
                    source_range: 103..215,
                    comment_text:
                        "  \n               This is a multi-line comment.\n               It spans multiple lines in Swift.\n               "
                            .to_string()
                },
                Comment {
                    position_range: Position::new(11, 40)..Position::new(11, 75),
                    source_range: 286..321,
                    comment_text: "   Prints a message to the console.".to_string()
                },
                Comment {
                    position_range: Position::new(13, 17)..Position::new(16, 20),
                    source_range: 343..446,
                    comment_text: "   Another comment\n                   split into\n                   multiple lines.\n                   ".to_string()
                }
            ]
        );

        Ok(())
    }
}
