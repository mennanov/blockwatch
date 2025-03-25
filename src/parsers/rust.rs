use crate::parsers::{CommentsParser, TreeSitterCommentsParser};
use tree_sitter::Query;

pub(crate) fn parser() -> anyhow::Result<impl CommentsParser> {
    let rust_language = tree_sitter_rust::LANGUAGE.into();
    let line_comment_query = Query::new(&rust_language, "(line_comment) @comment")?;
    let block_comment_query = Query::new(&rust_language, "(block_comment) @comment")?;
    let parser = TreeSitterCommentsParser::<fn(&str) -> String>::new(
        rust_language,
        vec![
            (
                line_comment_query,
                Some(|comment| {
                    comment
                        .strip_prefix("//")
                        .unwrap()
                        .trim_start_matches("!")
                        .trim_start_matches("/")
                        .trim()
                        .to_string()
                }),
            ),
            (
                block_comment_query,
                Some(|comment| {
                    comment
                        .strip_prefix("/*")
                        .expect("Expected a block comment to start with '/*'")
                        .lines()
                        .map(|line| {
                            line.trim_start()
                                .trim_start_matches("*")
                                .trim()
                                .trim_end_matches("/")
                                .trim_end_matches("*")
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
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
    fn parses_rust_comments_correctly() -> anyhow::Result<()> {
        let parser = parser()?;

        let blocks = parser.parse(
            r#"
        //! This is a crate-level documentation comment.
        //! It provides an overview of the module or library.
        
        /// This function adds two numbers.
        ///
        /// Returns the sum of `a` and `b`.
        fn add(a: i32, b: i32) -> i32 {
            a + b
        }
        
        fn main() {
            // This is a single-line comment.
            println!("Hello, Rust!");
        
            /* 
               This is a block comment.
               It can span multiple lines.
            */
            
            let x = 10;
            let y = 20;
            
            println!("{} + {} = {}", x, y, add(x, y)); // Using the add function.
        }
        "#,
        )?;

        assert_eq!(
            blocks,
            vec![
                (
                    2,
                    "This is a crate-level documentation comment.".to_string()
                ),
                (
                    3,
                    "It provides an overview of the module or library.".to_string()
                ),
                (5, "This function adds two numbers.".to_string()),
                (6, "".to_string()),
                (7, "Returns the sum of `a` and `b`.".to_string()),
                (13, "This is a single-line comment.".to_string()),
                (
                    16,
                    "\nThis is a block comment.\nIt can span multiple lines.\n".to_string()
                ),
                (24, "Using the add function.".to_string()),
            ]
        );

        Ok(())
    }
}
