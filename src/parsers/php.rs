use crate::parsers::{
    BlocksFromCommentsParser, BlocksParser, CommentsParser, TreeSitterCommentsParser,
};
use tree_sitter::Query;

/// Returns a [`BlocksParser`] for PHP.
pub(crate) fn parser() -> anyhow::Result<Box<dyn BlocksParser>> {
    Ok(Box::new(BlocksFromCommentsParser::new(comments_parser()?)))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let php_language = tree_sitter_php::LANGUAGE_PHP.into();
    let block_comment_query = Query::new(&php_language, "(comment) @comment")?;
    let parser = TreeSitterCommentsParser::<fn(usize, &str) -> Option<String>>::new(
        php_language,
        vec![(
            block_comment_query,
            Some(|_, comment| {
                if let Some(comment) = comment.strip_prefix("//") {
                    Some(comment.trim().to_string())
                } else if let Some(comment) = comment.strip_prefix("#") {
                    Some(comment.trim().to_string())
                } else {
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
    fn parses_php_comments_correctly() -> anyhow::Result<()> {
        let comments_parser = comments_parser()?;

        let blocks = comments_parser.parse(
            r#"<?php
            // This is a single-line comment in PHP
    
            /*
             * This is a multi-line comment.
             * It spans multiple lines in PHP.
             */
    
            function main() {
                echo "Hello, PHP!"; # Prints a message to the console.
    
                /* Another comment
                 * split into
                 * multiple lines.
                 */
                 
                return 0;
            }
            ?>
            <h1>This is an <?php # inlined comment ?> example</h1>
            <p>The header above will say 'This is an  example'.</p>
            "#,
        )?;

        assert_eq!(
            blocks,
            vec![
                Comment {
                    source_line_number: 2,
                    source_start_position: 18,
                    source_end_position: 57,
                    comment_text: "This is a single-line comment in PHP".to_string()
                },
                Comment {
                    source_line_number: 4,
                    source_start_position: 75,
                    source_end_position: 185,
                    comment_text:
                        "\nThis is a multi-line comment.\nIt spans multiple lines in PHP.\n"
                            .to_string()
                },
                Comment {
                    source_line_number: 10,
                    source_start_position: 257,
                    source_end_position: 291,
                    comment_text: "Prints a message to the console.".to_string()
                },
                Comment {
                    source_line_number: 12,
                    source_start_position: 313,
                    source_end_position: 416,
                    comment_text: "Another comment\nsplit into\nmultiple lines.\n".to_string()
                },
                Comment {
                    source_line_number: 20,
                    source_start_position: 523,
                    source_end_position: 541,
                    comment_text: "inlined comment".to_string()
                },
            ]
        );

        Ok(())
    }
}
