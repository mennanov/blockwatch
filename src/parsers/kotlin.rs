use crate::parsers::{
    BlocksFromCommentsParser, BlocksParser, CommentsParser, TreeSitterCommentsParser,
};
use tree_sitter::Query;

/// Returns a [`BlocksParser`] for Kotlin.
pub(crate) fn parser() -> anyhow::Result<Box<dyn BlocksParser>> {
    Ok(Box::new(BlocksFromCommentsParser::new(comments_parser()?)))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let kotlin_language = tree_sitter_kotlin_ng::LANGUAGE.into();
    let line_comment_query = Query::new(&kotlin_language, "(line_comment) @comment")?;
    let multi_line_comment_query = Query::new(&kotlin_language, "(block_comment) @comment")?;
    let parser = TreeSitterCommentsParser::<fn(usize, &str) -> Option<String>>::new(
        kotlin_language,
        vec![
            (
                line_comment_query,
                Some(|_, comment| Some(comment.strip_prefix("//").unwrap().trim().to_string())),
            ),
            (
                multi_line_comment_query,
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
                                    .trim()
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
    fn parses_kotlin_comments_correctly() -> anyhow::Result<()> {
        let comments_parser = comments_parser()?;

        let blocks = comments_parser.parse(
            r#"
            // This is a single-line comment in Kotlin
            fun main() {
                
            /*
             * This is a multi-line comment.
             * It spans multiple lines in Kotlin.
             */
            
                println("Hello, Kotlin!") // Prints a message
                
                /* Another comment
                 * split into
                 * multiple lines
                 */
                 
                return
            }
            "#,
        )?;

        assert_eq!(
            blocks,
            vec![
                (2, "This is a single-line comment in Kotlin".to_string()),
                (
                    5,
                    "\nThis is a multi-line comment.\nIt spans multiple lines in Kotlin.\n"
                        .to_string()
                ),
                (10, "Prints a message".to_string()),
                (
                    12,
                    "Another comment\nsplit into\nmultiple lines\n".to_string()
                )
            ]
        );

        Ok(())
    }
}
