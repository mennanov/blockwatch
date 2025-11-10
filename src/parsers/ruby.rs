use crate::parsers::{
    BlocksFromCommentsParser, BlocksParser, CommentsParser, python_style_comments_parser,
};
use tree_sitter::Query;

/// Returns a [`BlocksParser`] for Ruby.
pub(super) fn parser() -> anyhow::Result<Box<dyn BlocksParser>> {
    Ok(Box::new(BlocksFromCommentsParser::new(comments_parser()?)))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let ruby_language = tree_sitter_ruby::LANGUAGE.into();
    let line_comment_query = Query::new(&ruby_language, "(comment) @comment")?;
    let parser = python_style_comments_parser(ruby_language, line_comment_query);
    Ok(parser)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsers::Comment;

    #[test]
    fn parses_comments_correctly() -> anyhow::Result<()> {
        let comments_parser = comments_parser()?;

        let blocks = comments_parser.parse(
            r#"
        def main
            # This is a single line comment
            puts "Hello, # this is not a comment"  # This is an inline comment

# This is a multi-line
# comment that spans
# several lines

value = 42  # Comment after code
"#,
        )?;

        assert_eq!(
            blocks,
            vec![
                Comment {
                    source_line_number: 3,
                    source_start_position: 30,
                    source_end_position: 61,
                    comment_text: "  This is a single line comment".to_string()
                },
                Comment {
                    source_line_number: 4,
                    source_start_position: 113,
                    source_end_position: 140,
                    comment_text: "  This is an inline comment".to_string()
                },
                Comment {
                    source_line_number: 6,
                    source_start_position: 142,
                    source_end_position: 164,
                    comment_text: "  This is a multi-line".to_string()
                },
                Comment {
                    source_line_number: 7,
                    source_start_position: 165,
                    source_end_position: 185,
                    comment_text: "  comment that spans".to_string()
                },
                Comment {
                    source_line_number: 8,
                    source_start_position: 186,
                    source_end_position: 201,
                    comment_text: "  several lines".to_string()
                },
                Comment {
                    source_line_number: 10,
                    source_start_position: 215,
                    source_end_position: 235,
                    comment_text: "  Comment after code".to_string()
                }
            ]
        );

        Ok(())
    }
}
