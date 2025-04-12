use crate::parsers::{
    BlocksFromCommentsParser, BlocksParser, CommentsParser, TreeSitterCommentsParser,
};
use tree_sitter::Query;

/// Returns a [`BlocksParser`] for Java.
pub(crate) fn parser() -> anyhow::Result<Box<dyn BlocksParser>> {
    Ok(Box::new(BlocksFromCommentsParser::new(comments_parser()?)))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let java_language = tree_sitter_java::LANGUAGE.into();
    let line_comment_query = Query::new(&java_language, "(line_comment) @comment")?;
    let block_comment_query = Query::new(&java_language, "(block_comment) @comment")?;
    let parser = TreeSitterCommentsParser::<fn(usize, &str) -> Option<String>>::new(
        java_language,
        vec![
            (
                line_comment_query,
                Some(|_, comment| Some(comment.strip_prefix("//").unwrap().trim().to_string())),
            ),
            (
                block_comment_query,
                Some(|_, comment| {
                    Some(
                        comment
                            .strip_prefix("/*")
                            .unwrap()
                            .lines()
                            .map(|line| {
                                line.trim_start()
                                    .trim_start_matches("*")
                                    .trim()
                                    .trim_end_matches("/")
                                    .trim_end_matches("*")
                            })
                            .collect::<Vec<_>>()
                            .join("\n"),
                    )
                }),
            ),
        ],
    );
    Ok(parser)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_comments_correctly() -> anyhow::Result<()> {
        let comments_parser = comments_parser()?;

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
                (2, "\nThis is a simple Java program demonstrating different types of comments.\n\n@version 1.0\n".to_string()),
                (10, "This is a single-line comment.".to_string()),
                (11, "Prints a message to the console.".to_string()),
                (13, "\nThis is a multi-line comment.\nIt can span multiple lines.\n".to_string()),
                (17, "Assigning a value to the variable ".to_string()),
                (19, "This is a single-line doc-comment. ".to_string()),
                (23, "\nPrints a sample message to the console.\n".to_string())
            ]
        );

        Ok(())
    }
}
