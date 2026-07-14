use crate::block_parser::{BlocksFromCommentsParser, BlocksParser};
use crate::language_parsers::{CommentsParser, python_style_comments_parser};

/// Returns a [`BlocksParser`] for GraphQL.
pub(super) fn parser() -> anyhow::Result<impl BlocksParser> {
    Ok(BlocksFromCommentsParser::new(comments_parser()?))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let graphql_language = tree_sitter_graphql::LANGUAGE.into();
    let parser = python_style_comments_parser(&graphql_language, "comment");
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
type Query {
  # Indented comment
  hello: String # inline comment
}
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
                    source_range: 41..59,
                    comment_text: "  Indented comment".to_string()
                },
                Comment {
                    position_range: Position::new(5, 17)..Position::new(5, 33),
                    source_range: 76..92,
                    comment_text: "  inline comment".to_string()
                },
            ]
        );

        Ok(())
    }
}
