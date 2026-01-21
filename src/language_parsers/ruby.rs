use crate::block_parser::{BlocksFromCommentsParser, BlocksParser};
use crate::language_parsers::{CommentsParser, python_style_comments_parser};

/// Returns a [`BlocksParser`] for Ruby.
pub(super) fn parser() -> anyhow::Result<impl BlocksParser> {
    Ok(BlocksFromCommentsParser::new(comments_parser()?))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let ruby_language = tree_sitter_ruby::LANGUAGE.into();
    let parser = python_style_comments_parser(&ruby_language, "comment");
    Ok(parser)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Position, language_parsers::Comment};

    #[test]
    fn parses_comments_correctly() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser()?;

        let blocks: Vec<Comment> = comments_parser
            .parse(
                r#"
def main
    # This is a single line comment
    puts "Hello, # this is not a comment"  # This is an inline comment

# This is a multi-line
# comment that spans
# several lines

value = 42  # Comment after code
"#,
            )
            .collect();

        assert_eq!(
            blocks,
            vec![
                Comment {
                    position_range: Position::new(3, 5)..Position::new(3, 36),
                    source_range: 14..45,
                    comment_text: "  This is a single line comment".to_string()
                },
                Comment {
                    position_range: Position::new(4, 44)..Position::new(4, 71),
                    source_range: 89..116,
                    comment_text: "  This is an inline comment".to_string()
                },
                Comment {
                    position_range: Position::new(6, 1)..Position::new(6, 23),
                    source_range: 118..140,
                    comment_text: "  This is a multi-line".to_string()
                },
                Comment {
                    position_range: Position::new(7, 1)..Position::new(7, 21),
                    source_range: 141..161,
                    comment_text: "  comment that spans".to_string()
                },
                Comment {
                    position_range: Position::new(8, 1)..Position::new(8, 16),
                    source_range: 162..177,
                    comment_text: "  several lines".to_string()
                },
                Comment {
                    position_range: Position::new(10, 13)..Position::new(10, 33),
                    source_range: 191..211,
                    comment_text: "  Comment after code".to_string()
                }
            ]
        );

        Ok(())
    }
}
