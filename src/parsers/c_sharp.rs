use crate::parsers::{
    BlocksFromCommentsParser, BlocksParser, CommentsParser, TreeSitterCommentsParser,
};
use tree_sitter::Query;

/// Returns a [`BlocksParser`] for C++.
pub(crate) fn parser() -> anyhow::Result<Box<dyn BlocksParser>> {
    Ok(Box::new(BlocksFromCommentsParser::new(comments_parser()?)))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let c_sharp = tree_sitter_c_sharp::LANGUAGE.into();
    let block_comment_query = Query::new(&c_sharp, "(comment) @comment")?;
    let parser = TreeSitterCommentsParser::<fn(usize, &str) -> Option<String>>::new(
        c_sharp,
        vec![(
            block_comment_query,
            Some(|_, comment| {
                if comment.starts_with("//") {
                    Some(
                        comment
                            .strip_prefix("//")
                            .unwrap()
                            .trim_start_matches("/")
                            .trim()
                            .to_string(),
                    )
                } else {
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
                }
            }),
        )],
    );
    Ok(parser)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_c_sharp_comments_correctly() -> anyhow::Result<()> {
        let comments_parser = comments_parser()?;

        let code = r#"
// Single line comment
using System;

namespace HelloWorld
{
    /* Multi-line
     * comment example.
     */
    class Program
    {
        /// <summary>
        /// XML Doc comment.
        /// </summary>
        static void Main(string[] args)
        {
            Console.WriteLine("Hello World!"); // Another single line
            /* Simple block */
        }
    }
}
"#;
        let blocks = comments_parser.parse(code)?;

        assert_eq!(
            blocks,
            vec![
                (2, "Single line comment".to_string()),
                (7, "Multi-line\ncomment example.\n".to_string()),
                (12, "<summary>".to_string()), // Note: Current parser logic treats /// like //
                (13, "XML Doc comment.".to_string()),
                (14, "</summary>".to_string()),
                (17, "Another single line".to_string()),
                (18, "Simple block".to_string())
            ]
        );

        Ok(())
    }
}
