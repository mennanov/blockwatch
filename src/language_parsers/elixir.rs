use crate::block_parser::{BlocksFromCommentsParser, BlocksParser};
use crate::language_parsers::{CommentsParser, python_style_comments_parser};

/// Returns a [`BlocksParser`] for Elixir.
pub(super) fn parser() -> anyhow::Result<impl BlocksParser> {
    Ok(BlocksFromCommentsParser::new(comments_parser()?))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let elixir_language = tree_sitter_elixir::LANGUAGE.into();
    let parser = python_style_comments_parser(&elixir_language, "comment");
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
# This is a line comment
defmodule Greeter do
  # Indented comment
  def hello do
    :world # inline comment
  end
end
"#,
            )
            .collect();

        assert_eq!(
            comments,
            vec![
                Comment {
                    position_range: Position::new(2, 1)..Position::new(2, 25),
                    source_range: 1..25,
                    comment_text: "  This is a line comment".to_string()
                },
                Comment {
                    position_range: Position::new(4, 3)..Position::new(4, 21),
                    source_range: 49..67,
                    comment_text: "  Indented comment".to_string()
                },
                Comment {
                    position_range: Position::new(6, 12)..Position::new(6, 28),
                    source_range: 94..110,
                    comment_text: "  inline comment".to_string()
                },
            ]
        );

        Ok(())
    }
}
