use crate::parsers::{
    BlocksFromCommentsParser, BlocksParser, CommentsParser, TreeSitterCommentsParser,
};
use tree_sitter::Query;

/// Returns a [`BlocksParser`] for C++.
pub(super) fn parser() -> anyhow::Result<Box<dyn BlocksParser>> {
    Ok(Box::new(BlocksFromCommentsParser::new(comments_parser()?)))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let cpp_language = tree_sitter_cpp::LANGUAGE.into();
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
    fn parses_cpp_comments_correctly() -> anyhow::Result<()> {
        let comments_parser = comments_parser()?;

        let blocks = comments_parser.parse(
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
        )?;

        assert_eq!(
            blocks,
            vec![
                Comment {
                    source_line_number: 2,
                    source_start_position: 13,
                    source_end_position: 53,
                    comment_text: "This is a single-line comment in C++.".to_string()
                },
                Comment {
                    source_line_number: 5,
                    source_start_position: 99,
                    source_end_position: 202,
                    comment_text: "\nThis is a multi-line comment.\nIt spans multiple lines.\n"
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
                    source_start_position: 339,
                    source_end_position: 435,
                    comment_text: "This is another\nmulti-line\ncomment.\n".to_string()
                },
            ]
        );

        Ok(())
    }
}
