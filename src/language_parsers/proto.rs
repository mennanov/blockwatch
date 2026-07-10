use crate::block_parser::{BlocksFromCommentsParser, BlocksParser};
use crate::language_parsers;
use crate::language_parsers::CommentsParser;

/// Returns a [`BlocksParser`] for Protocol Buffers.
pub(super) fn parser() -> anyhow::Result<impl BlocksParser> {
    Ok(BlocksFromCommentsParser::new(comments_parser()?))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let proto_language = tree_sitter_proto::LANGUAGE.into();
    let parser = language_parsers::c_style_comments_parser(&proto_language, "comment");
    Ok(parser)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Position, language_parsers::Comment};

    #[test]
    fn parses_comments_correctly() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser()?;

        let comments: Vec<Comment> = comments_parser
            .parse(
                r#"
// This is a line comment
syntax = "proto3";

message User {
  int32 id = 1; // inline comment
}

/* This is a block comment
spanning multiple lines
*/
/// Triple slash comment.
"#,
            )
            .collect();

        assert_eq!(
            comments,
            vec![
                Comment {
                    position_range: Position::new(2, 1)..Position::new(2, 26),
                    source_range: 1..26,
                    comment_text: "   This is a line comment".to_string()
                },
                Comment {
                    position_range: Position::new(6, 17)..Position::new(6, 34),
                    source_range: 78..95,
                    comment_text: "   inline comment".to_string()
                },
                Comment {
                    position_range: Position::new(9, 1)..Position::new(11, 3),
                    source_range: 99..152,
                    comment_text: "   This is a block comment\nspanning multiple lines\n  "
                        .to_string()
                },
                Comment {
                    position_range: Position::new(12, 1)..Position::new(12, 26),
                    source_range: 153..178,
                    comment_text: "  / Triple slash comment.".to_string()
                },
            ]
        );

        Ok(())
    }
}
