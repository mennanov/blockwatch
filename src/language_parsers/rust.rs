use crate::block_parser::{BlocksFromCommentsParser, BlocksParser};
use crate::language_parsers::{
    CommentsParser, TreeSitterCommentsParser, c_style_multiline_comment_processor,
};

/// Returns a [`BlocksParser`] for Rust.
pub(crate) fn parser() -> anyhow::Result<impl BlocksParser> {
    Ok(BlocksFromCommentsParser::new(comments_parser()?))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let rust_language = tree_sitter_rust::LANGUAGE.into();
    let parser = TreeSitterCommentsParser::new(
        &rust_language,
        Box::new(|node, source_code| match node.kind() {
            "line_comment" => {
                let comment = &source_code[node.byte_range()];
                Some(if comment.starts_with("///") {
                    comment.replacen("///", "   ", 1)
                } else if comment.starts_with("//!") {
                    comment.replacen("//!", "   ", 1)
                } else if comment.starts_with("//") {
                    comment.replacen("//", "  ", 1)
                } else {
                    comment.to_string()
                })
            }
            "block_comment" => Some(c_style_multiline_comment_processor(
                &source_code[node.byte_range()],
            )),
            _ => None,
        }),
    );
    Ok(parser)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Position, language_parsers::Comment};

    #[test]
    fn parses_comments_correctly() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser()?;

        let blocks: Vec<Comment> = comments_parser
            .parse(
                r#"
        //! This is a crate-level documentation comment.
        //! It provides an overview of the module or library.
        
        /// This function adds two numbers.
        ///
        /// Returns the sum of `a` and `b`.
        fn add(a: i32, b: i32) -> i32 {
            a + b
        }
        
        fn main() {
            // This is a single-line comment.
            println!("Hello, Rust!");
        
            /* 
               This is a block comment.
               It can span multiple lines. */
            
            let x = 10;
            let y = 20;
            
            println!("{} + {} = {}", x, y, add(x, y)); // Using the add function.
        }
        "#,
            )
            .collect();

        assert_eq!(
            blocks,
            vec![
                Comment {
                    position_range: Position::new(2, 9)..Position::new(3, 1),
                    source_range: 9..58,
                    comment_text: "    This is a crate-level documentation comment.\n".to_string()
                },
                Comment {
                    position_range: Position::new(3, 9)..Position::new(4, 1),
                    source_range: 66..120,
                    comment_text: "    It provides an overview of the module or library.\n".to_string()
                },
                Comment {
                    position_range: Position::new(5, 9)..Position::new(6, 1),
                    source_range: 137..173, // TODO: incorrect?
                    comment_text: "    This function adds two numbers.\n".to_string()
                },
                Comment {
                    position_range: Position::new(6, 9)..Position::new(7, 1),
                    source_range: 181..185,
                    comment_text: "   \n".to_string()
                },
                Comment {
                    position_range: Position::new(7, 9)..Position::new(8, 1),
                    source_range: 193..229,
                    comment_text: "    Returns the sum of `a` and `b`.\n".to_string()
                },
                Comment {
                    position_range: Position::new(13, 13)..Position::new(13, 46),
                    source_range: 338..371,
                    comment_text: "   This is a single-line comment.".to_string()
                },
                Comment {
                    position_range: Position::new(16, 13)..Position::new(18, 46),
                    source_range: 431..520,
                    comment_text: "   \n               This is a block comment.\n               It can span multiple lines.   "
                        .to_string()
                },
                Comment {
                    position_range: Position::new(23, 56)..Position::new(23, 82),
                    source_range: 650..676,
                    comment_text: "   Using the add function.".to_string()
                }
            ]
        );

        Ok(())
    }
}
