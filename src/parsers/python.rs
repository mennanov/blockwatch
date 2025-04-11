use crate::parsers::{
    BlocksFromCommentsParser, BlocksParser, CommentsParser, TreeSitterCommentsParser,
};
use tree_sitter::Query;

/// Returns a [`BlockParser`] for Python.
pub(crate) fn parser() -> anyhow::Result<Box<dyn BlocksParser>> {
    Ok(Box::new(BlocksFromCommentsParser::new(comments_parser()?)))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let python_language = tree_sitter_python::LANGUAGE.into();
    let line_comment_query = Query::new(&python_language, "(comment) @comment")?;
    let parser = TreeSitterCommentsParser::<fn(usize, &str) -> Option<String>>::new(
        python_language,
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
    fn parses_comments_correctly() -> anyhow::Result<()> {
        let comments_parser = comments_parser()?;

        let blocks = comments_parser.parse(
            r#"
def main():
    # This is a single line comment
    print("Hello")  # This is an inline comment

# This is a multi-line
# comment that spans
# several lines

value = 42  # Comment after code
"#,
        )?;

        assert_eq!(
            blocks,
            vec![
                (3, "This is a single line comment".to_string()),
                (4, "This is an inline comment".to_string()),
                (6, "This is a multi-line".to_string()),
                (7, "comment that spans".to_string()),
                (8, "several lines".to_string()),
                (10, "Comment after code".to_string()),
            ]
        );

        Ok(())
    }
}
