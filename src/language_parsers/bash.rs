use crate::block_parser::{BlocksFromCommentsParser, BlocksParser};
use crate::language_parsers::{CommentsParser, TreeSitterCommentsParser};
use tree_sitter::Query;

/// Returns a [`BlocksParser`] for Bash.
pub(super) fn parser() -> anyhow::Result<impl BlocksParser> {
    Ok(BlocksFromCommentsParser::new(comments_parser()?))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let bash_language = tree_sitter_bash::LANGUAGE.into();
    let comment_query = Query::new(&bash_language, "(comment) @comment")?;
    let parser = TreeSitterCommentsParser::new(
        bash_language,
        vec![(
            comment_query,
            Some(Box::new(|_, comment, _node| {
                if comment.starts_with("#!") {
                    Ok(None)
                } else {
                    Ok(Some(comment.replacen("#", " ", 1)))
                }
            })),
        )],
    );
    Ok(parser)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::language_parsers::Comment;

    #[test]
    fn parses_comments_correctly() -> anyhow::Result<()> {
        let comments_parser = comments_parser()?;

        let blocks = comments_parser.parse(
            r#"#!/bin/bash
# This is a single line comment
echo "Hello"  # This is an inline comment

# This is a multi-line
# comment that spans
# several lines

VALUE=42  # Comment after code
"#,
        )?;

        assert_eq!(
            blocks,
            vec![
                Comment {
                    source_line_number: 2,
                    source_start_position: 12,
                    source_end_position: 43,
                    comment_text: "  This is a single line comment".to_string()
                },
                Comment {
                    source_line_number: 3,
                    source_start_position: 58,
                    source_end_position: 85,
                    comment_text: "  This is an inline comment".to_string()
                },
                Comment {
                    source_line_number: 5,
                    source_start_position: 87,
                    source_end_position: 109,
                    comment_text: "  This is a multi-line".to_string()
                },
                Comment {
                    source_line_number: 6,
                    source_start_position: 110,
                    source_end_position: 130,
                    comment_text: "  comment that spans".to_string()
                },
                Comment {
                    source_line_number: 7,
                    source_start_position: 131,
                    source_end_position: 146,
                    comment_text: "  several lines".to_string()
                },
                Comment {
                    source_line_number: 9,
                    source_start_position: 158,
                    source_end_position: 178,
                    comment_text: "  Comment after code".to_string()
                },
            ]
        );

        Ok(())
    }
}
