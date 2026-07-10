use crate::block_parser::{BlocksFromCommentsParser, BlocksParser};
use crate::language_parsers;
use crate::language_parsers::CommentsParser;

/// Returns a [`BlocksParser`] for Groovy.
pub(super) fn parser() -> anyhow::Result<impl BlocksParser> {
    Ok(BlocksFromCommentsParser::new(comments_parser()?))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let groovy_language = tree_sitter_groovy::LANGUAGE.into();
    let parser = language_parsers::c_style_line_and_block_comments_parser(
        &groovy_language,
        "line_comment",
        "block_comment",
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

        // The shebang line is a separate node kind and must not be extracted as a comment.
        let comments: Vec<Comment> = comments_parser
            .parse(
                r#"#!/usr/bin/env groovy
// This is a line comment
def x = 42 // inline comment

/* This is a block comment
spanning multiple lines
*/
println(x)
/// Triple slash comment.
"#,
            )
            .collect();

        assert_eq!(
            comments,
            vec![
                Comment {
                    position_range: Position::new(2, 1)..Position::new(2, 26),
                    source_range: 22..47,
                    comment_text: "   This is a line comment".to_string()
                },
                Comment {
                    position_range: Position::new(3, 12)..Position::new(3, 29),
                    source_range: 59..76,
                    comment_text: "   inline comment".to_string()
                },
                Comment {
                    position_range: Position::new(5, 1)..Position::new(7, 3),
                    source_range: 78..131,
                    comment_text: "   This is a block comment\nspanning multiple lines\n  "
                        .to_string()
                },
                Comment {
                    position_range: Position::new(9, 1)..Position::new(9, 26),
                    source_range: 143..168,
                    comment_text: "  / Triple slash comment.".to_string()
                },
            ]
        );

        Ok(())
    }
}
