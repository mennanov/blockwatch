use crate::block_parser::{BlocksFromCommentsParser, BlocksParser};
use crate::language_parsers::{
    CommentsParser, TreeSitterCommentsParser, c_style_multiline_comment_processor,
};

/// Returns a [`BlocksParser`] for CSS.
pub(super) fn parser() -> anyhow::Result<impl BlocksParser> {
    Ok(BlocksFromCommentsParser::new(comments_parser()?))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let css_language = tree_sitter_css::LANGUAGE.into();
    let parser = TreeSitterCommentsParser::new(
        &css_language,
        Box::new(|node, source_code| {
            if node.kind() == "comment" {
                Some(c_style_multiline_comment_processor(
                    &source_code[node.byte_range()],
                ))
            } else {
                None
            }
        }),
    );
    Ok(parser)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Position, language_parsers::Comment};

    #[test]
    fn parses_css_comments_correctly() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser()?;

        let blocks: Vec<Comment> = comments_parser
            .parse(
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
            )
            .collect();

        assert_eq!(
            blocks,
            vec![
                Comment {
                    position_range: Position::new(6, 13)..Position::new(6, 40),
                    source_range: 81..108,
                    comment_text: "   This is a CSS comment   ".to_string()
                },
                Comment {
                    position_range: Position::new(8, 17)..Position::new(11, 20),
                    source_range: 147..266,
                    comment_text: "   This is a multi-line\n                   CSS comment that spans\n                   multiple lines\n                   "
                        .to_string()
                },
                Comment {
                    position_range: Position::new(15, 13)..Position::new(17, 39),
                    source_range: 339..431,
                    comment_text: "   Another multi-line\n               CSS comment with\n               different formatting   "
                        .to_string()
                },
            ]
        );

        Ok(())
    }
}
