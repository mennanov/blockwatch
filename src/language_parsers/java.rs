use crate::block_parser::{BlocksFromCommentsParser, BlocksParser};
use crate::language_parsers::{CommentsParser, c_style_line_and_block_comments_parser};
use tree_sitter::Query;

/// Returns a [`BlocksParser`] for Java.
pub(super) fn parser() -> anyhow::Result<impl BlocksParser> {
    Ok(BlocksFromCommentsParser::new(comments_parser()?))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let java_language = tree_sitter_java::LANGUAGE.into();
    let line_comment_query = Query::new(&java_language, "(line_comment) @comment")?;
    let block_comment_query = Query::new(&java_language, "(block_comment) @comment")?;
    let parser = c_style_line_and_block_comments_parser(
        &java_language,
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
    fn parses_comments_correctly() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser()?;

        let blocks = comments_parser.parse(
            r#"
        /**
         * This is a simple Java program demonstrating different types of comments.
         * 
         * @version 1.0
         */
        public class CommentExample {
        
            public static void main(String[] args) {
                // This is a single-line comment.
                System.out.println("Hello, World!"); // Prints a message to the console.
        
                /*
                 * This is a multi-line comment.
                 * It can span multiple lines.
                 */
                int number = 42; /* Assigning a value to the variable */
        
                /** This is a single-line doc-comment. */
                printMessage();
            }
        
            /**
             * Prints a sample message to the console.
             */
            public static void printMessage() {
                System.out.println("/**This is a method with a Javadoc comment.*/");
            }
        }
        "#,
        )?;

        assert_eq!(
            blocks,
            vec![
                Comment {
                    position_range: Position::new(2, 9)..Position::new(6, 12),
                    source_range: 9..144,
                    comment_text: "   \n           This is a simple Java program demonstrating different types of comments.\n           \n           @version 1.0\n           ".to_string()
                },
                Comment {
                    position_range: Position::new(10, 17)..Position::new(10, 50),
                    source_range: 261..294,
                    comment_text: "   This is a single-line comment.".to_string()
                },
                Comment {
                    position_range: Position::new(11, 54)..Position::new(11, 89),
                    source_range: 348..383,
                    comment_text: "   Prints a message to the console.".to_string()
                },
                Comment {
                    position_range: Position::new(13, 17)..Position::new(16, 20),
                    source_range: 409..527,
                    comment_text: "  \n                   This is a multi-line comment.\n                   It can span multiple lines.\n                   ".to_string()
                },
                Comment {
                    position_range: Position::new(17, 34)..Position::new(17, 73),
                    source_range: 561..600,
                    comment_text: "   Assigning a value to the variable   ".to_string()
                },
                Comment {
                    position_range: Position::new(19, 17)..Position::new(19, 58),
                    source_range: 626..667,
                    comment_text: "    This is a single-line doc-comment.   ".to_string()
                },
                Comment {
                    position_range: Position::new(23, 13)..Position::new(25, 16),
                    source_range: 735..809,
                    comment_text: "   \n               Prints a sample message to the console.\n               ".to_string()
                }
            ]
        );

        Ok(())
    }
}
