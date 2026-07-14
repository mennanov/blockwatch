use crate::block_parser::{BlocksFromCommentsParser, BlocksParser};
use crate::language_parsers::{CommentsParser, python_style_comments_parser};

/// Returns a [`BlocksParser`] for Starlark (Bazel).
pub(super) fn parser() -> anyhow::Result<impl BlocksParser> {
    Ok(BlocksFromCommentsParser::new(comments_parser()?))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let starlark_language = tree_sitter_starlark::LANGUAGE.into();
    let parser = python_style_comments_parser(&starlark_language, "comment");
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
cc_library(
    # Indented comment
    name = "foo", # inline comment
)
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
                    position_range: Position::new(4, 5)..Position::new(4, 23),
                    source_range: 42..60,
                    comment_text: "  Indented comment".to_string()
                },
                Comment {
                    position_range: Position::new(5, 19)..Position::new(5, 35),
                    source_range: 79..95,
                    comment_text: "  inline comment".to_string()
                },
            ]
        );

        Ok(())
    }
}
