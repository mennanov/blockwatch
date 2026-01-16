use crate::block_parser::{BlocksFromCommentsParser, BlocksParser};
use crate::language_parsers::{CommentsParser, c_style_line_and_block_comments_parser};
use tree_sitter::Query;

/// Returns a [`BlocksParser`] for Kotlin.
pub(super) fn parser() -> anyhow::Result<impl BlocksParser> {
    Ok(BlocksFromCommentsParser::new(comments_parser()?))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let kotlin_language = tree_sitter_kotlin_ng::LANGUAGE.into();
    let line_comment_query = Query::new(&kotlin_language, "(line_comment) @comment")?;
    let block_comment_query = Query::new(&kotlin_language, "(block_comment) @comment")?;
    let parser = c_style_line_and_block_comments_parser(
        &kotlin_language,
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
    fn parses_kotlin_comments_correctly() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser()?;

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
                Comment {
                    position_range: Position::new(2, 13)..Position::new(2, 55),
                    source_range: 13..55,
                    comment_text: "   This is a single-line comment in Kotlin".to_string()
                },
                Comment {
                    position_range: Position::new(5, 13)..Position::new(8, 16),
                    source_range: 110..223,
                    comment_text:
                        "  \n               This is a multi-line comment.\n               It spans multiple lines in Kotlin.\n               "
                            .to_string()
                },
                Comment {
                    position_range: Position::new(10, 43)..Position::new(10, 62),
                    source_range: 279..298,
                    comment_text: "   Prints a message".to_string()
                },
                Comment {
                    position_range: Position::new(12, 17)..Position::new(15, 20),
                    source_range: 332..434,
                    comment_text: "   Another comment\n                   split into\n                   multiple lines\n                   ".to_string()
                },
            ]
        );

        Ok(())
    }
}
