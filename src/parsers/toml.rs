use crate::parsers::{
    BlocksFromCommentsParser, BlocksParser, CommentsParser, TreeSitterCommentsParser,
};
use tree_sitter::Query;

/// Returns a [`BlocksParser`] for Toml.
pub(super) fn parser() -> anyhow::Result<Box<dyn BlocksParser>> {
    Ok(Box::new(BlocksFromCommentsParser::new(comments_parser()?)))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let toml_language = tree_sitter_toml_ng::LANGUAGE.into();
    let line_comment_query = Query::new(&toml_language, "(comment) @comment")?;
    let parser = TreeSitterCommentsParser::<fn(usize, &str) -> Option<String>>::new(
        toml_language,
        vec![(
            line_comment_query,
            Some(|_, comment| Some(comment.strip_prefix('#').unwrap().trim().to_string())),
        )],
    );
    Ok(parser)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsers::Comment;

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
                    source_line_number: 2,
                    source_start_position: 1,
                    source_end_position: 22,
                    comment_text: "This is a TOML file".to_string()
                },
                Comment {
                    source_line_number: 3,
                    source_start_position: 46,
                    source_end_position: 62,
                    comment_text: "Inline comment".to_string()
                },
                Comment {
                    source_line_number: 5,
                    source_start_position: 71,
                    source_end_position: 88,
                    comment_text: "Owner's details".to_string()
                },
                Comment {
                    source_line_number: 6,
                    source_start_position: 117,
                    source_end_position: 141,
                    comment_text: "Another inline comment".to_string()
                },
                Comment {
                    source_line_number: 7,
                    source_start_position: 174,
                    source_end_position: 202,
                    comment_text: "Date of birth with comment".to_string()
                },
                Comment {
                    source_line_number: 8,
                    source_start_position: 203,
                    source_end_position: 216,
                    comment_text: "End of file".to_string()
                }
            ]
        );

        Ok(())
    }
}
