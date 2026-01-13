use crate::block_parser::{BlocksFromCommentsParser, BlocksParser};
use crate::language_parsers::{CommentsParser, python_style_comments_parser};
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
    use crate::{Position, language_parsers::Comment};

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
                    start_position: Position::new(2, 1),
                    end_position: Position::new(2, 20),
                    source_range: 1..20,
                    comment_text: "  This is a comment".to_string()
                },
                Comment {
                    start_position: Position::new(6, 1),
                    end_position: Position::new(6, 18),
                    source_range: 59..76,
                    comment_text: "  Another comment".to_string()
                },
                Comment {
                    start_position: Position::new(7, 1),
                    end_position: Position::new(7, 26),
                    source_range: 77..102,
                    comment_text: "  spanning multiple lines".to_string()
                },
            ]
        );

        Ok(())
    }
}
