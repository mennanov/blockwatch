use crate::block_parser::{BlocksFromCommentsParser, BlocksParser};
use crate::language_parsers;
use crate::language_parsers::CommentsParser;

/// Returns a [`BlocksParser`] for Scala.
pub(super) fn parser() -> anyhow::Result<impl BlocksParser> {
    Ok(BlocksFromCommentsParser::new(comments_parser()?))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let scala_language = tree_sitter_scala::LANGUAGE.into();
    let parser = language_parsers::c_style_line_and_block_comments_parser(
        &scala_language,
        "comment",
        "block_comment",
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

        let comments: Vec<Comment> = comments_parser
            .parse(
                r#"
// This is a single line comment
object Main {
  val x = 42 // inline comment

  /* This is a block comment
  spanning multiple lines
  */
}
/// Triple slash comment.
"#,
            )
            .collect();

        assert_eq!(
            comments,
            vec![
                Comment {
                    position_range: Position::new(2, 1)..Position::new(2, 33),
                    source_range: 1..33,
                    comment_text: "   This is a single line comment".to_string()
                },
                Comment {
                    position_range: Position::new(4, 14)..Position::new(4, 31),
                    source_range: 61..78,
                    comment_text: "   inline comment".to_string()
                },
                Comment {
                    position_range: Position::new(6, 3)..Position::new(8, 5),
                    source_range: 82..139,
                    comment_text: "   This is a block comment\n  spanning multiple lines\n    "
                        .to_string()
                },
                Comment {
                    position_range: Position::new(10, 1)..Position::new(10, 26),
                    source_range: 142..167,
                    comment_text: "  / Triple slash comment.".to_string()
                },
            ]
        );

        Ok(())
    }
}
