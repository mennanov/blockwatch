use crate::parsers::{
    BlocksFromCommentsParser, BlocksParser, CommentsParser, TreeSitterCommentsParser,
};
use tree_sitter::Query;

/// Returns a [`BlocksParser`] for Xml.
pub(crate) fn parser() -> anyhow::Result<Box<dyn BlocksParser>> {
    Ok(Box::new(BlocksFromCommentsParser::new(comments_parser()?)))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let xml_language = tree_sitter_xml::LANGUAGE_XML.into();
    let line_comment_query = Query::new(&xml_language, "(Comment) @comment")?;
    let parser = TreeSitterCommentsParser::<fn(usize, &str) -> Option<String>>::new(
        xml_language,
        vec![(
            line_comment_query,
            Some(|_, comment| {
                Some(
                    comment
                        .strip_prefix("<!--")
                        .unwrap()
                        .trim_end_matches("-->")
                        .lines()
                        .map(|line| line.trim())
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
                    source_line_number: 2,
                    source_start_position: 13,
                    source_end_position: 39,
                    comment_text: "This is a comment".to_string()
                },
                Comment {
                    source_line_number: 4,
                    source_start_position: 75,
                    source_end_position: 99,
                    comment_text: "Another comment".to_string()
                },
                Comment {
                    source_line_number: 6,
                    source_start_position: 153,
                    source_end_position: 244,
                    comment_text: "\nMultiline comment\n<foo>bar</foo>\n".to_string()
                },
                Comment {
                    source_line_number: 11,
                    source_start_position: 277,
                    source_end_position: 299,
                    comment_text: "Final comment".to_string()
                }
            ]
        );

        Ok(())
    }
}
