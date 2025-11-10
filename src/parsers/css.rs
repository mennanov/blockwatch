use crate::parsers::{
    BlocksFromCommentsParser, BlocksParser, CommentsParser, TreeSitterCommentsParser,
    c_style_multiline_comment_processor,
};
use tree_sitter::Query;

/// Returns a [`BlocksParser`] for CSS.
pub(super) fn parser() -> anyhow::Result<Box<dyn BlocksParser>> {
    Ok(Box::new(BlocksFromCommentsParser::new(comments_parser()?)))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let css_language = tree_sitter_css::LANGUAGE.into();
    let multi_line_comment_query = Query::new(&css_language, "(comment) @comment")?;
    let parser = TreeSitterCommentsParser::<fn(usize, &str) -> Option<String>>::new(
        css_language,
        vec![(
            multi_line_comment_query,
            Some(|_, comment| Some(c_style_multiline_comment_processor(comment))),
        )],
    );
    Ok(parser)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsers::Comment;

    #[test]
    fn parses_css_comments_correctly() -> anyhow::Result<()> {
        let comments_parser = comments_parser()?;

        let blocks = comments_parser.parse(
            r#"
            body {
                color: black;
            }
    
            /* This is a CSS comment */
            .header {
                /* This is a multi-line
                 * CSS comment that spans
                 * multiple lines
                 */
                font-size: 16px;
            }
            
            /* Another multi-line
               CSS comment with
               different formatting */
            "#,
        )?;

        assert_eq!(
            blocks,
            vec![
                Comment {
                    source_line_number: 6,
                    source_start_position: 81,
                    source_end_position: 108,
                    comment_text: "   This is a CSS comment   ".to_string()
                },
                Comment {
                    source_line_number: 8,
                    source_start_position: 147,
                    source_end_position: 266,
                    comment_text: "   This is a multi-line\n                   CSS comment that spans\n                   multiple lines\n                   "
                        .to_string()
                },
                Comment {
                    source_line_number: 15,
                    source_start_position: 339,
                    source_end_position: 431,
                    comment_text: "   Another multi-line\n               CSS comment with\n               different formatting   "
                        .to_string()
                },
            ]
        );

        Ok(())
    }
}
