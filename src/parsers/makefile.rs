use crate::parsers::{
    BlocksFromCommentsParser, BlocksParser, CommentsParser, python_style_comments_parser,
};
use tree_sitter::Query;

/// Returns a [`BlocksParser`] for Makefile.
pub(super) fn parser() -> anyhow::Result<impl BlocksParser> {
    Ok(BlocksFromCommentsParser::new(comments_parser()?))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let language = tree_sitter_make::LANGUAGE.into();
    let comment_query = Query::new(&language, "(comment) @comment")?;
    let parser = python_style_comments_parser(language, comment_query);
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
# This is a comment
all:
	@echo "Hello" # Inline comment

# Another comment
# spanning multiple lines
"#,
        )?;

        assert_eq!(
            blocks,
            vec![
                Comment {
                    source_line_number: 2,
                    source_start_position: 1,
                    source_end_position: 20,
                    comment_text: "  This is a comment".to_string()
                },
                Comment {
                    source_line_number: 6,
                    source_start_position: 59,
                    source_end_position: 76,
                    comment_text: "  Another comment".to_string()
                },
                Comment {
                    source_line_number: 7,
                    source_start_position: 77,
                    source_end_position: 102,
                    comment_text: "  spanning multiple lines".to_string()
                },
            ]
        );

        Ok(())
    }
}
