use crate::block_parser::{BlocksFromCommentsParser, BlocksParser};
use crate::language_parsers::{CommentsParser, xml_style_comments_parser};
use tree_sitter::Query;

/// Returns a [`BlocksParser`] for HTML.
pub(super) fn parser() -> anyhow::Result<impl BlocksParser> {
    Ok(BlocksFromCommentsParser::new(comments_parser()?))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let html_language = tree_sitter_html::LANGUAGE.into();
    let comment_query = Query::new(&html_language, "(comment) @comment")?;
    let parser = xml_style_comments_parser(html_language, comment_query);
    Ok(parser)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Position, language_parsers::Comment};

    #[test]
    fn parses_html_comments_correctly() -> anyhow::Result<()> {
        let comments_parser = comments_parser()?;

        let blocks = comments_parser.parse(
            r#"<!DOCTYPE html>
            <!-- Simple comment -->
            <div>
                <!-- Another comment -->
                <p>Some text</p>
                <!--
                Multi-line comment
                with multiple lines
                -->
            </div>
            <!-- Final comment -->
            "#,
        )?;

        assert_eq!(
            blocks,
            vec![
                Comment {
                    position_range: Position::new(2, 13)..Position::new(2, 36),
                    source_range: 28..51,
                    comment_text: "     Simple comment    ".to_string()
                },
                Comment {
                    position_range: Position::new(4, 17)..Position::new(4, 41),
                    source_range: 86..110,
                    comment_text: "     Another comment    ".to_string()
                },
                Comment {
                    position_range: Position::new(6, 17)..Position::new(9, 20),
                    source_range: 160..255,
                    comment_text: "    \n                Multi-line comment\n                with multiple lines\n                   ".to_string()
                },
                Comment {
                    position_range: Position::new(11, 13)..Position::new(11, 35),
                    source_range: 287..309,
                    comment_text: "     Final comment    ".to_string()
                },
            ]
        );

        Ok(())
    }
}
