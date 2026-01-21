use crate::block_parser::{BlocksFromCommentsParser, BlocksParser};
use crate::language_parsers::{
    CommentsParser, TreeSitterCommentsParser, c_style_multiline_comment_processor,
};

/// Returns a [`BlocksParser`] for C++.
pub(super) fn parser() -> anyhow::Result<impl BlocksParser> {
    Ok(BlocksFromCommentsParser::new(comments_parser()?))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let c_sharp = tree_sitter_c_sharp::LANGUAGE.into();
    let parser = TreeSitterCommentsParser::new(
        &c_sharp,
        Box::new(|node, source_code| {
            if node.kind() == "comment" {
                let comment = &source_code[node.byte_range()];
                Some(if comment.starts_with("///") {
                    comment.replacen("///", "   ", 1)
                } else if comment.starts_with("//") {
                    comment.replacen("//", "  ", 1)
                } else {
                    c_style_multiline_comment_processor(comment)
                })
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
    fn parses_c_sharp_comments_correctly() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser()?;

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
        let blocks: Vec<Comment> = comments_parser.parse(code).collect();

        assert_eq!(
            blocks,
            vec![
                Comment {
                    position_range: Position::new(2, 1)..Position::new(2, 23),
                    source_range: 1..23,
                    comment_text: "   Single line comment".to_string()
                },
                Comment {
                    position_range: Position::new(7, 5)..Position::new(9, 8),
                    source_range: 66..111,
                    comment_text: "   Multi-line\n       comment example.\n       ".to_string()
                },
                Comment {
                    position_range: Position::new(12, 9)..Position::new(12, 22),
                    source_range: 144..157,
                    comment_text: "    <summary>".to_string()
                },
                Comment {
                    position_range: Position::new(13, 9)..Position::new(13, 29),
                    source_range: 166..186,
                    comment_text: "    XML Doc comment.".to_string()
                },
                Comment {
                    position_range: Position::new(14, 9)..Position::new(14, 23),
                    source_range: 195..209,
                    comment_text: "    </summary>".to_string()
                },
                Comment {
                    position_range: Position::new(17, 48)..Position::new(17, 70),
                    source_range: 307..329,
                    comment_text: "   Another single line".to_string()
                },
                Comment {
                    position_range: Position::new(18, 13)..Position::new(18, 31),
                    source_range: 342..360,
                    comment_text: "   Simple block   ".to_string()
                }
            ]
        );

        Ok(())
    }
}
