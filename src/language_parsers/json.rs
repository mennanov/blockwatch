use crate::block_parser::{BlocksFromCommentsParser, BlocksParser};
use crate::language_parsers::{CommentsParser, c_style_comments_parser};

/// Returns a [`BlocksParser`] for JSON.
pub(super) fn parser() -> anyhow::Result<impl BlocksParser> {
    Ok(BlocksFromCommentsParser::new(comments_parser()?))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let json_language = tree_sitter_json::LANGUAGE.into();
    let parser = c_style_comments_parser(&json_language, "comment");
    Ok(parser)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Position;
    use crate::language_parsers::Comment;

    #[test]
    fn json_with_comments_parses_correctly() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser()?;

        let blocks: Vec<Comment> = comments_parser
            .parse(
                r#"{
  // Line comment
  "key": "value" /* Block comment */
}
"#,
            )
            .collect();

        assert_eq!(
            blocks,
            vec![
                Comment {
                    position_range: Position::new(2, 3)..Position::new(2, 18),
                    source_range: 4..19,
                    comment_text: "   Line comment".to_string(),
                },
                Comment {
                    position_range: Position::new(3, 18)..Position::new(3, 37),
                    source_range: 37..56,
                    comment_text: "   Block comment   ".to_string(),
                },
            ]
        );

        Ok(())
    }
}
