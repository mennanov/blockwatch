use crate::parsers::{
    BlocksFromCommentsParser, BlocksParser, CommentsParser, python_style_comments_parser,
};
use tree_sitter::Query;

/// Returns a [`BlocksParser`] for Python.
pub(super) fn parser() -> anyhow::Result<impl BlocksParser> {
    Ok(BlocksFromCommentsParser::new(comments_parser()?))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let python_language = tree_sitter_python::LANGUAGE.into();
    let line_comment_query = Query::new(&python_language, "(comment) @comment")?;
    let parser = python_style_comments_parser(python_language, line_comment_query);
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
def main():
    # This is a single line comment
    print("Hello")  # This is an inline comment

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
                    source_start_position: 17,
                    source_end_position: 48,
                    comment_text: "  This is a single line comment".to_string()
                },
                Comment {
                    source_line_number: 4,
                    source_start_position: 69,
                    source_end_position: 96,
                    comment_text: "  This is an inline comment".to_string()
                },
                Comment {
                    source_line_number: 6,
                    source_start_position: 98,
                    source_end_position: 120,
                    comment_text: "  This is a multi-line".to_string()
                },
                Comment {
                    source_line_number: 7,
                    source_start_position: 121,
                    source_end_position: 141,
                    comment_text: "  comment that spans".to_string()
                },
                Comment {
                    source_line_number: 8,
                    source_start_position: 142,
                    source_end_position: 157,
                    comment_text: "  several lines".to_string()
                },
                Comment {
                    source_line_number: 10,
                    source_start_position: 171,
                    source_end_position: 191,
                    comment_text: "  Comment after code".to_string()
                },
            ]
        );

        Ok(())
    }
}
