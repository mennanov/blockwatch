use crate::parsers::{
    BlocksFromCommentsParser, BlocksParser, CommentsParser, TreeSitterCommentsParser,
};
use tree_sitter::Query;

/// Returns a [`BlocksParser`] for Yaml.
pub(crate) fn parser() -> anyhow::Result<Box<dyn BlocksParser>> {
    Ok(Box::new(BlocksFromCommentsParser::new(comments_parser()?)))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let yaml_language = tree_sitter_yaml::LANGUAGE.into();
    let line_comment_query = Query::new(&yaml_language, "(comment) @comment")?;
    let parser = TreeSitterCommentsParser::<fn(usize, &str) -> Option<String>>::new(
        yaml_language,
        vec![(
            line_comment_query,
            Some(|_, comment| Some(comment.strip_prefix("#").unwrap().trim().to_string())),
        )],
    );
    Ok(parser)
}

#[cfg(test)]
mod tests {
    use super::*;

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
                (2, "This is a YAML comment".to_string()),
                (3, "Inline comment on a key-value pair".to_string()),
                (5, "Another comment".to_string()),
                (7, "Comment in a list".to_string()),
                (9, "End of comments".to_string()),
            ]
        );

        Ok(())
    }
}
