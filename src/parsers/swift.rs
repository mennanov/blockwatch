use crate::parsers::{
    BlocksFromCommentsParser, BlocksParser, CommentsParser, c_style_line_and_block_comments_parser,
};
use tree_sitter::Query;

/// Returns a [`BlocksParser`] for Swift.
pub(super) fn parser() -> anyhow::Result<Box<dyn BlocksParser>> {
    Ok(Box::new(BlocksFromCommentsParser::new(comments_parser()?)))
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
    use crate::parsers::Comment;

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
                    source_line_number: 2,
                    source_start_position: 13,
                    source_end_position: 55,
                    comment_text: "This is a single-line comment in Swift.".to_string()
                },
                Comment {
                    source_line_number: 5,
                    source_start_position: 103,
                    source_end_position: 215,
                    comment_text:
                        "\nThis is a multi-line comment.\nIt spans multiple lines in Swift.\n"
                            .to_string()
                },
                Comment {
                    source_line_number: 11,
                    source_start_position: 286,
                    source_end_position: 321,
                    comment_text: "Prints a message to the console.".to_string()
                },
                Comment {
                    source_line_number: 13,
                    source_start_position: 343,
                    source_end_position: 446,
                    comment_text: "Another comment\nsplit into\nmultiple lines.\n".to_string()
                }
            ]
        );

        Ok(())
    }
}
