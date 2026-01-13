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
    use crate::{Position, language_parsers::Comment};

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
                    start_position: Position::new(2, 1),
                    end_position: Position::new(2, 39),
                    source_range: 1..39,
                    comment_text: "   This is a single line comment in Go".to_string()
                },
                Comment {
                    start_position: Position::new(5, 1),
                    end_position: Position::new(8, 3),
                    source_range: 54..111,
                    comment_text: "  \nThis is a multi-line comment\nspanning several lines\n  "
                        .to_string()
                },
                Comment {
                    start_position: Position::new(13, 34),
                    end_position: Position::new(13, 51),
                    source_range: 174..191,
                    comment_text: "   Inline comment".to_string()
                },
                Comment {
                    start_position: Position::new(14, 5),
                    end_position: Position::new(14, 35),
                    source_range: 196..226,
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
                    start_position: Position::new(6, 1),
                    end_position: Position::new(6, 31),
                    source_range: 40..70,
                    comment_text: "   This is a comment in go.mod".to_string()
                },
                Comment {
                    start_position: Position::new(8, 39),
                    end_position: Position::new(8, 56),
                    source_range: 119..136,
                    comment_text: "   Inline comment".to_string()
                },
                Comment {
                    start_position: Position::new(11, 1),
                    end_position: Position::new(14, 3),
                    source_range: 140..164,
                    comment_text: "  \nMulti-line\ncomment\n  ".to_string()
                },
                Comment {
                    start_position: Position::new(15, 1),
                    end_position: Position::new(15, 19),
                    source_range: 165..183,
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
                    start_position: Position::new(4, 1),
                    end_position: Position::new(4, 32),
                    source_range: 10..41,
                    comment_text: "   This is a comment in go.work".to_string()
                },
                Comment {
                    start_position: Position::new(7, 15),
                    end_position: Position::new(7, 32),
                    source_range: 76..93,
                    comment_text: "   Inline comment".to_string()
                },
                Comment {
                    start_position: Position::new(10, 1),
                    end_position: Position::new(13, 3),
                    source_range: 97..131,
                    comment_text: "  \nMulti-line\nworkspace comment\n  ".to_string()
                },
                Comment {
                    start_position: Position::new(14, 1),
                    end_position: Position::new(14, 19),
                    source_range: 132..150,
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
                    start_position: Position::new(2, 1),
                    end_position: Position::new(2, 31),
                    source_range: 1..31,
                    comment_text: "   This is a comment in go.sum".to_string()
                },
                Comment {
                    start_position: Position::new(4, 48),
                    end_position: Position::new(4, 63),
                    source_range: 119..134,
                    comment_text: "   Hash comment".to_string()
                },
                Comment {
                    start_position: Position::new(6, 1),
                    end_position: Position::new(9, 3),
                    source_range: 136..170,
                    comment_text: "  \nMulti-line comment\nin go.sum\n  ".to_string()
                },
                Comment {
                    start_position: Position::new(10, 1),
                    end_position: Position::new(10, 17),
                    source_range: 171..187,
                    comment_text: "   Final comment".to_string()
                },
            ]
        );

        Ok(())
    }
}
