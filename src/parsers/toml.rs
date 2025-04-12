use crate::parsers::{
    BlocksFromCommentsParser, BlocksParser, CommentsParser, TreeSitterCommentsParser,
};
use tree_sitter::Query;

/// Returns a [`BlocksParser`] for Toml.
pub(crate) fn parser() -> anyhow::Result<Box<dyn BlocksParser>> {
    Ok(Box::new(BlocksFromCommentsParser::new(comments_parser()?)))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let toml_language = tree_sitter_toml_ng::LANGUAGE.into();
    let line_comment_query = Query::new(&toml_language, "(comment) @comment")?;
    let parser = TreeSitterCommentsParser::<fn(usize, &str) -> Option<String>>::new(
        toml_language,
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
    fn parses_toml_comments_correctly() -> anyhow::Result<()> {
        let comments_parser = comments_parser()?;

        let blocks = comments_parser.parse(
            r#"
# This is a TOML file
title = "TOML Example" # Inline comment
[owner]
# Owner's details
name = "Tom Preston-Werner" # Another inline comment
dob = 1979-05-27T07:32:00-08:00 # Date of birth with comment
# End of file
"#,
        )?;

        assert_eq!(
            blocks,
            vec![
                (2, "This is a TOML file".to_string()),
                (3, "Inline comment".to_string()),
                (5, "Owner's details".to_string()),
                (6, "Another inline comment".to_string()),
                (7, "Date of birth with comment".to_string()),
                (8, "End of file".to_string()),
            ]
        );

        Ok(())
    }
}
