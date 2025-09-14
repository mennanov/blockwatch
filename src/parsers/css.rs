use crate::parsers::{
    BlocksFromCommentsParser, BlocksParser, CommentsParser, TreeSitterCommentsParser,
};
use tree_sitter::Query;

/// Returns a [`BlocksParser`] for CSS.
pub(crate) fn parser() -> anyhow::Result<Box<dyn BlocksParser>> {
    Ok(Box::new(BlocksFromCommentsParser::new(comments_parser()?)))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let css_language = tree_sitter_css::LANGUAGE.into();
    let multi_line_comment_query = Query::new(&css_language, "(comment) @comment")?;
    let parser = TreeSitterCommentsParser::<fn(usize, &str) -> Option<String>>::new(
        css_language,
        vec![(
            multi_line_comment_query,
            Some(|_, comment| {
                Some(
                    comment
                        .strip_prefix("/*")
                        .unwrap()
                        .lines()
                        .map(|line| {
                            line.trim_start()
                                .trim_start_matches("*")
                                .trim()
                                .trim_end_matches("/")
                                .trim_end_matches("*")
                                .trim()
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                )
            }),
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
                    comment_text: "This is a CSS comment".to_string()
                },
                Comment {
                    source_line_number: 8,
                    source_start_position: 147,
                    source_end_position: 266,
                    comment_text: "This is a multi-line\nCSS comment that spans\nmultiple lines\n"
                        .to_string()
                },
                Comment {
                    source_line_number: 15,
                    source_start_position: 339,
                    source_end_position: 431,
                    comment_text: "Another multi-line\nCSS comment with\ndifferent formatting"
                        .to_string()
                },
            ]
        );

        Ok(())
    }
}
