use crate::parsers;
use crate::parsers::{BlocksFromCommentsParser, BlocksParser, CommentsParser};
use tree_sitter::Query;

/// Returns a [`BlocksParser`] for Golang.
pub(super) fn parser() -> anyhow::Result<Box<dyn BlocksParser>> {
    Ok(Box::new(BlocksFromCommentsParser::new(comments_parser()?)))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let go_language = tree_sitter_go::LANGUAGE.into();
    let comment_query = Query::new(&go_language, "(comment) @comment")?;
    let parser = parsers::c_style_comments_parser(go_language, comment_query);
    Ok(parser)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsers::Comment;

    #[test]
    fn parses_go_comments_correctly() -> anyhow::Result<()> {
        let comments_parser = comments_parser()?;

        let blocks = comments_parser.parse(
            r#"
// This is a single line comment in Go
package main

/*
This is a multi-line comment
spanning several lines
*/

import "fmt"

func main() {
    fmt.Println("Hello, World!") // Inline comment
    // Another single line comment
}
"#,
        )?;

        assert_eq!(
            blocks,
            vec![
                Comment {
                    source_line_number: 2,
                    source_start_position: 1,
                    source_end_position: 39,
                    comment_text: "   This is a single line comment in Go".to_string()
                },
                Comment {
                    source_line_number: 5,
                    source_start_position: 54,
                    source_end_position: 111,
                    comment_text: "  \nThis is a multi-line comment\nspanning several lines\n  "
                        .to_string()
                },
                Comment {
                    source_line_number: 13,
                    source_start_position: 174,
                    source_end_position: 191,
                    comment_text: "   Inline comment".to_string()
                },
                Comment {
                    source_line_number: 14,
                    source_start_position: 196,
                    source_end_position: 226,
                    comment_text: "   Another single line comment".to_string()
                },
            ]
        );

        Ok(())
    }
}
