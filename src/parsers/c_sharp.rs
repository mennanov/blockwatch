use crate::parsers::{
    BlocksFromCommentsParser, BlocksParser, CommentsParser, TreeSitterCommentsParser,
    c_style_multiline_comment_processor,
};
use tree_sitter::Query;

/// Returns a [`BlocksParser`] for C++.
pub(super) fn parser() -> anyhow::Result<Box<dyn BlocksParser>> {
    Ok(Box::new(BlocksFromCommentsParser::new(comments_parser()?)))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let c_sharp = tree_sitter_c_sharp::LANGUAGE.into();
    let comment_query = Query::new(&c_sharp, "(comment) @comment")?;
    let parser = TreeSitterCommentsParser::<fn(usize, &str) -> Option<String>>::new(
        c_sharp,
        vec![(
            comment_query,
            Some(|_, comment| {
                Some(if comment.starts_with("///") {
                    comment.replacen("///", "   ", 1)
                } else if comment.starts_with("//") {
                    comment.replacen("//", "  ", 1)
                } else {
                    c_style_multiline_comment_processor(comment)
                })
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
                Comment {
                    source_line_number: 2,
                    source_start_position: 1,
                    source_end_position: 23,
                    comment_text: "   Single line comment".to_string()
                },
                Comment {
                    source_line_number: 7,
                    source_start_position: 66,
                    source_end_position: 111,
                    comment_text: "   Multi-line\n       comment example.\n       ".to_string()
                },
                Comment {
                    source_line_number: 12,
                    source_start_position: 144,
                    source_end_position: 157,
                    comment_text: "    <summary>".to_string()
                },
                Comment {
                    source_line_number: 13,
                    source_start_position: 166,
                    source_end_position: 186,
                    comment_text: "    XML Doc comment.".to_string()
                },
                Comment {
                    source_line_number: 14,
                    source_start_position: 195,
                    source_end_position: 209,
                    comment_text: "    </summary>".to_string()
                },
                Comment {
                    source_line_number: 17,
                    source_start_position: 307,
                    source_end_position: 329,
                    comment_text: "   Another single line".to_string()
                },
                Comment {
                    source_line_number: 18,
                    source_start_position: 342,
                    source_end_position: 360,
                    comment_text: "   Simple block   ".to_string()
                }
            ]
        );

        Ok(())
    }
}
