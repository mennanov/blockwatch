use crate::parsers::{
    BlocksFromCommentsParser, BlocksParser, CommentsParser, TreeSitterCommentsParser,
};
use tree_sitter::Query;

/// Returns a [`BlocksParser`] for C.
pub(super) fn parser() -> anyhow::Result<Box<dyn BlocksParser>> {
    Ok(Box::new(BlocksFromCommentsParser::new(comments_parser()?)))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let cpp_language = tree_sitter_c::LANGUAGE.into();
    let block_comment_query = Query::new(&cpp_language, "(comment) @comment")?;
    let parser = TreeSitterCommentsParser::<fn(usize, &str) -> Option<String>>::new(
        cpp_language,
        vec![(
            block_comment_query,
            Some(|_, comment| {
                if comment.starts_with("//") {
                    Some(comment.strip_prefix("//").unwrap().trim().to_string())
                } else {
                    Some(
                        comment
                            .strip_prefix("/*")
                            .unwrap()
                            .lines()
                            .map(|line| {
                                line.trim_start()
                                    .trim_start_matches('*')
                                    .trim()
                                    .trim_end_matches('/')
                                    .trim_end_matches('*')
                                    .trim()
                            })
                            .collect::<Vec<_>>()
                            .join("\n"),
                    )
                }
            }),
        )],
    );
    Ok(parser)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsers::Comment;

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
                    comment_text: "This is a single-line comment in C.".to_string()
                },
                Comment {
                    source_line_number: 5,
                    source_start_position: 96,
                    source_end_position: 204,
                    comment_text:
                        "\nThis is a multi-line comment.\nIt spans multiple lines in C.\n"
                            .to_string()
                },
                Comment {
                    source_line_number: 11,
                    source_start_position: 270,
                    source_end_position: 305,
                    comment_text: "Prints a message to the console.".to_string()
                },
                Comment {
                    source_line_number: 13,
                    source_start_position: 323,
                    source_end_position: 426,
                    comment_text: "Another comment\nsplit into\nmultiple lines.\n".to_string()
                },
            ]
        );

        Ok(())
    }
}
