use crate::parsers::{
    BlocksFromCommentsParser, BlocksParser, CommentsParser, TreeSitterCommentsParser,
};
use tree_sitter::Query;

/// Returns a [`BlocksParser`] for Yaml.
pub(super) fn parser() -> anyhow::Result<Box<dyn BlocksParser>> {
    Ok(Box::new(BlocksFromCommentsParser::new(comments_parser()?)))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let yaml_language = tree_sitter_yaml::LANGUAGE.into();
    let line_comment_query = Query::new(&yaml_language, "(comment) @comment")?;
    let parser = TreeSitterCommentsParser::<fn(usize, &str) -> Option<String>>::new(
        yaml_language,
        vec![(
            line_comment_query,
            Some(|_, comment| Some(comment.strip_prefix('#').unwrap().trim().to_string())),
        )],
    );
    Ok(parser)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsers::Comment;

    #[test]
    fn parses_yaml_comments_correctly() -> anyhow::Result<()> {
        let comments_parser = comments_parser()?;

        let blocks = comments_parser.parse(
            r#"
# This is a YAML comment
key: value  # Inline comment on a key-value pair

# Another comment
list:
  - item1  # Comment in a list
  - item2
# End of comments
"#,
        )?;

        assert_eq!(
            blocks,
            vec![
                Comment {
                    source_line_number: 2,
                    source_start_position: 1,
                    source_end_position: 25,
                    comment_text: "This is a YAML comment".to_string()
                },
                Comment {
                    source_line_number: 3,
                    source_start_position: 38,
                    source_end_position: 74,
                    comment_text: "Inline comment on a key-value pair".to_string()
                },
                Comment {
                    source_line_number: 5,
                    source_start_position: 76,
                    source_end_position: 93,
                    comment_text: "Another comment".to_string()
                },
                Comment {
                    source_line_number: 7,
                    source_start_position: 111,
                    source_end_position: 130,
                    comment_text: "Comment in a list".to_string()
                },
                Comment {
                    source_line_number: 9,
                    source_start_position: 141,
                    source_end_position: 158,
                    comment_text: "End of comments".to_string()
                }
            ]
        );

        Ok(())
    }
}
