use crate::block_parser::{BlocksFromCommentsParser, BlocksParser};
use crate::language_parsers::{CommentsParser, python_style_comments_parser};
use tree_sitter::Query;

/// Returns a [`BlocksParser`] for Toml.
pub(super) fn parser() -> anyhow::Result<impl BlocksParser> {
    Ok(BlocksFromCommentsParser::new(comments_parser()?))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let toml_language = tree_sitter_toml_ng::LANGUAGE.into();
    let line_comment_query = Query::new(&toml_language, "(comment) @comment")?;
    let parser = python_style_comments_parser(toml_language, line_comment_query);
    Ok(parser)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Position, language_parsers::Comment};

    #[test]
    fn parses_toml_comments_correctly() -> anyhow::Result<()> {
        let comments_parser = comments_parser()?;

        let blocks = comments_parser.parse(
            r#"
# This is a TOML file
title = "TOML Example" # Inline comment
[owner]
# Owner's details
name = "Tom Preston-Werner" # Another inline comment
dob = 1979-05-27T07:32:00-08:00 # Date of birth with comment
# End of file
"#,
        )?;

        assert_eq!(
            blocks,
            vec![
                Comment {
                    position_range: Position::new(2, 1)..Position::new(2, 22),
                    source_range: 1..22,
                    comment_text: "  This is a TOML file".to_string()
                },
                Comment {
                    position_range: Position::new(3, 24)..Position::new(3, 40),
                    source_range: 46..62,
                    comment_text: "  Inline comment".to_string()
                },
                Comment {
                    position_range: Position::new(5, 1)..Position::new(5, 18),
                    source_range: 71..88,
                    comment_text: "  Owner's details".to_string()
                },
                Comment {
                    position_range: Position::new(6, 29)..Position::new(6, 53),
                    source_range: 117..141,
                    comment_text: "  Another inline comment".to_string()
                },
                Comment {
                    position_range: Position::new(7, 33)..Position::new(7, 61),
                    source_range: 174..202,
                    comment_text: "  Date of birth with comment".to_string()
                },
                Comment {
                    position_range: Position::new(8, 1)..Position::new(8, 14),
                    source_range: 203..216,
                    comment_text: "  End of file".to_string()
                }
            ]
        );

        Ok(())
    }
}
