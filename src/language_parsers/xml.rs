use crate::block_parser::{BlocksFromCommentsParser, BlocksParser};
use crate::language_parsers::{CommentsParser, xml_style_comments_parser};
use tree_sitter::Query;

/// Returns a [`BlocksParser`] for Xml.
pub(super) fn parser() -> anyhow::Result<impl BlocksParser> {
    Ok(BlocksFromCommentsParser::new(comments_parser()?))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let xml_language = tree_sitter_xml::LANGUAGE_XML.into();
    let line_comment_query = Query::new(&xml_language, "(Comment) @comment")?;
    let parser = xml_style_comments_parser(xml_language, line_comment_query);
    Ok(parser)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Position, language_parsers::Comment};

    #[test]
    fn parses_xml_comments_correctly() -> anyhow::Result<()> {
        let comments_parser = comments_parser()?;

        let blocks = comments_parser.parse(
            r#"
            <!-- This is a comment -->
            <root>
                <!-- Another comment -->
                <child>Value</child>
                <!-- 
                Multiline comment 
                <foo>bar</foo>
                -->
            </root>
            <!-- Final comment -->
            "#,
        )?;

        assert_eq!(
            blocks,
            vec![
                Comment {
                    start_position: Position::new(2, 13),
                    end_position: Position::new(2, 39),
                    source_range: 13..39,
                    comment_text: "     This is a comment    ".to_string()
                },
                Comment {
                    start_position: Position::new(4, 17),
                    end_position: Position::new(4, 41),
                    source_range: 75..99,
                    comment_text: "     Another comment    ".to_string()
                },
                Comment {
                    start_position: Position::new(6, 17),
                    end_position: Position::new(9, 20),
                    source_range: 153..244,
                    comment_text: "     \n                Multiline comment \n                <foo>bar</foo>\n                   ".to_string()
                },
                Comment {
                    start_position: Position::new(11, 13),
                    end_position: Position::new(11, 35),
                    source_range: 277..299,
                    comment_text: "     Final comment    ".to_string()
                }
            ]
        );

        Ok(())
    }
}
