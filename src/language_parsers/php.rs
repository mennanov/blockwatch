use crate::block_parser::{BlocksFromCommentsParser, BlocksParser};
use crate::language_parsers::{
    CommentsParser, TreeSitterCommentsParser, c_style_multiline_comment_processor,
};
use tree_sitter::Query;

/// Returns a [`BlocksParser`] for PHP.
pub(super) fn parser() -> anyhow::Result<impl BlocksParser> {
    Ok(BlocksFromCommentsParser::new(comments_parser()?))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let php_language = tree_sitter_php::LANGUAGE_PHP.into();
    let block_comment_query = Query::new(&php_language, "(comment) @comment")?;
    let parser = TreeSitterCommentsParser::new(
        &php_language,
        vec![(
            block_comment_query,
            Some(Box::new(|_, comment, _node| {
                Ok(Some(if comment.starts_with("//") {
                    comment.replacen("//", "  ", 1)
                } else if comment.starts_with("#") {
                    comment.replacen("#", " ", 1)
                } else {
                    c_style_multiline_comment_processor(comment)
                }))
            })),
        )],
    );
    Ok(parser)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Position, language_parsers::Comment};

    #[test]
    fn parses_php_comments_correctly() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser()?;

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
                    position_range: Position::new(2, 13)..Position::new(2, 52),
                    source_range: 18..57,
                    comment_text: "   This is a single-line comment in PHP".to_string()
                },
                Comment {
                    position_range: Position::new(4, 13)..Position::new(7, 16),
                    source_range: 75..185,
                    comment_text:
                        "  \n               This is a multi-line comment.\n               It spans multiple lines in PHP.\n               "
                            .to_string()
                },
                Comment {
                    position_range: Position::new(10, 37)..Position::new(10, 71),
                    source_range: 257..291,
                    comment_text: "  Prints a message to the console.".to_string()
                },
                Comment {
                    position_range: Position::new(12, 17)..Position::new(15, 20),
                    source_range: 313..416,
                    comment_text: "   Another comment\n                   split into\n                   multiple lines.\n                   ".to_string()
                },
                Comment {
                    position_range: Position::new(20, 34)..Position::new(20, 52),
                    source_range: 523..541,
                    comment_text: "  inlined comment ".to_string()
                },
            ]
        );

        Ok(())
    }
}
