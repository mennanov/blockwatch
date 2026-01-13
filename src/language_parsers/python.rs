use crate::block_parser::{BlocksFromCommentsParser, BlocksParser};
use crate::language_parsers::{CommentsParser, python_style_comments_parser};
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
    use crate::{Position, language_parsers::Comment};

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
                    start_position: Position::new(3, 5),
                    end_position: Position::new(3, 36),
                    source_range: 17..48,
                    comment_text: "  This is a single line comment".to_string()
                },
                Comment {
                    start_position: Position::new(4, 21),
                    end_position: Position::new(4, 48),
                    source_range: 69..96,
                    comment_text: "  This is an inline comment".to_string()
                },
                Comment {
                    start_position: Position::new(6, 1),
                    end_position: Position::new(6, 23),
                    source_range: 98..120,
                    comment_text: "  This is a multi-line".to_string()
                },
                Comment {
                    start_position: Position::new(7, 1),
                    end_position: Position::new(7, 21),
                    source_range: 121..141,
                    comment_text: "  comment that spans".to_string()
                },
                Comment {
                    start_position: Position::new(8, 1),
                    end_position: Position::new(8, 16),
                    source_range: 142..157,
                    comment_text: "  several lines".to_string()
                },
                Comment {
                    start_position: Position::new(10, 13),
                    end_position: Position::new(10, 33),
                    source_range: 171..191,
                    comment_text: "  Comment after code".to_string()
                },
            ]
        );

        Ok(())
    }
}
