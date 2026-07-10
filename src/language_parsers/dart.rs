use crate::block_parser::{BlocksFromCommentsParser, BlocksParser};
use crate::language_parsers;
use crate::language_parsers::CommentsParser;

/// Returns a [`BlocksParser`] for Dart.
pub(super) fn parser() -> anyhow::Result<impl BlocksParser> {
    Ok(BlocksFromCommentsParser::new(comments_parser()?))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let dart_language = tree_sitter_dart::LANGUAGE.into();
    let parser = language_parsers::c_style_and_doc_comments_parser(&dart_language, "comment");
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
/// Doc comment
int add(int a, int b) {
  return a + b; // inline comment
}

/* This is a block comment
spanning multiple lines
*/
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
                    position_range: Position::new(3, 1)..Position::new(3, 16),
                    source_range: 27..42,
                    comment_text: "    Doc comment".to_string()
                },
                Comment {
                    position_range: Position::new(5, 17)..Position::new(5, 34),
                    source_range: 83..100,
                    comment_text: "   inline comment".to_string()
                },
                Comment {
                    position_range: Position::new(8, 1)..Position::new(10, 3),
                    source_range: 104..157,
                    comment_text: "   This is a block comment\nspanning multiple lines\n  "
                        .to_string()
                },
            ]
        );

        Ok(())
    }
}
