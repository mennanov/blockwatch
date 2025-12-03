use crate::block_parser::{BlocksFromCommentsParser, BlocksParser};
use crate::language_parsers;
use crate::language_parsers::CommentsParser;
use tree_sitter::Query;

/// Returns a [`BlocksParser`] for Golang.
pub(super) fn parser() -> anyhow::Result<impl BlocksParser> {
    Ok(BlocksFromCommentsParser::new(comments_parser()?))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let go_language = tree_sitter_go::LANGUAGE.into();
    let comment_query = Query::new(&go_language, "(comment) @comment")?;
    let parser = language_parsers::c_style_comments_parser(go_language, comment_query);
    Ok(parser)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::language_parsers::Comment;

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

    #[test]
    fn parses_go_mod_comments_correctly() -> anyhow::Result<()> {
        let comments_parser = comments_parser()?;

        let blocks = comments_parser.parse(
            r#"
module example.com/my/module

go 1.21

// This is a comment in go.mod
require (
    github.com/some/dependency v1.2.3 // Inline comment
)

/*
Multi-line
comment
*/
// Another comment
exclude github.com/bad/dependency v0.0.0
"#,
        )?;

        assert_eq!(
            blocks,
            vec![
                Comment {
                    source_line_number: 6,
                    source_start_position: 40,
                    source_end_position: 70,
                    comment_text: "   This is a comment in go.mod".to_string()
                },
                Comment {
                    source_line_number: 8,
                    source_start_position: 119,
                    source_end_position: 136,
                    comment_text: "   Inline comment".to_string()
                },
                Comment {
                    source_line_number: 11,
                    source_start_position: 140,
                    source_end_position: 164,
                    comment_text: "  \nMulti-line\ncomment\n  ".to_string()
                },
                Comment {
                    source_line_number: 15,
                    source_start_position: 165,
                    source_end_position: 183,
                    comment_text: "   Another comment".to_string()
                },
            ]
        );

        Ok(())
    }

    #[test]
    fn parses_go_work_comments_correctly() -> anyhow::Result<()> {
        let comments_parser = comments_parser()?;

        let blocks = comments_parser.parse(
            r#"
go 1.21

// This is a comment in go.work
use (
    ./module1
    ./module2 // Inline comment
)

/*
Multi-line
workspace comment
*/
// Another comment
use ./module3
"#,
        )?;

        assert_eq!(
            blocks,
            vec![
                Comment {
                    source_line_number: 4,
                    source_start_position: 10,
                    source_end_position: 41,
                    comment_text: "   This is a comment in go.work".to_string()
                },
                Comment {
                    source_line_number: 7,
                    source_start_position: 76,
                    source_end_position: 93,
                    comment_text: "   Inline comment".to_string()
                },
                Comment {
                    source_line_number: 10,
                    source_start_position: 97,
                    source_end_position: 131,
                    comment_text: "  \nMulti-line\nworkspace comment\n  ".to_string()
                },
                Comment {
                    source_line_number: 14,
                    source_start_position: 132,
                    source_end_position: 150,
                    comment_text: "   Another comment".to_string()
                },
            ]
        );

        Ok(())
    }

    #[test]
    fn parses_go_sum_comments_correctly() -> anyhow::Result<()> {
        let comments_parser = comments_parser()?;

        let blocks = comments_parser.parse(
            r#"
// This is a comment in go.sum
github.com/example/pkg v1.0.0 h1:abc123
github.com/example/pkg v1.0.0/go.mod h1:def456 // Hash comment

/*
Multi-line comment
in go.sum
*/
// Final comment
github.com/another/pkg v2.0.0 h1:xyz789
"#,
        )?;

        assert_eq!(
            blocks,
            vec![
                Comment {
                    source_line_number: 2,
                    source_start_position: 1,
                    source_end_position: 31,
                    comment_text: "   This is a comment in go.sum".to_string()
                },
                Comment {
                    source_line_number: 4,
                    source_start_position: 119,
                    source_end_position: 134,
                    comment_text: "   Hash comment".to_string()
                },
                Comment {
                    source_line_number: 6,
                    source_start_position: 136,
                    source_end_position: 170,
                    comment_text: "  \nMulti-line comment\nin go.sum\n  ".to_string()
                },
                Comment {
                    source_line_number: 10,
                    source_start_position: 171,
                    source_end_position: 187,
                    comment_text: "   Final comment".to_string()
                },
            ]
        );

        Ok(())
    }
}
