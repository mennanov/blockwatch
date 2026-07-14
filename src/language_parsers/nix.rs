use crate::block_parser::{BlocksFromCommentsParser, BlocksParser};
use crate::language_parsers;
use crate::language_parsers::CommentsParser;

/// Returns a [`BlocksParser`] for Nix.
pub(super) fn parser() -> anyhow::Result<impl BlocksParser> {
    Ok(BlocksFromCommentsParser::new(comments_parser()?))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let nix_language = tree_sitter_nix::LANGUAGE.into();
    // Nix has `#` line comments and `/* */` block comments; the `//` branch of the shared
    // parser is unreachable because `//` is the attrset-merge operator in Nix.
    let parser = language_parsers::hash_and_c_style_comments_parser(&nix_language, "comment");
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
# This is a hash comment
{
  region = "us-east-1"; # inline comment

  /* This is a block comment
  spanning multiple lines
  */
}
"#,
            )
            .collect();

        assert_eq!(
            comments,
            vec![
                Comment {
                    position_range: Position::new(2, 1)..Position::new(2, 25),
                    source_range: 1..25,
                    comment_text: "  This is a hash comment".to_string()
                },
                Comment {
                    position_range: Position::new(4, 25)..Position::new(4, 41),
                    source_range: 52..68,
                    comment_text: "  inline comment".to_string()
                },
                Comment {
                    position_range: Position::new(6, 3)..Position::new(8, 5),
                    source_range: 72..129,
                    comment_text: "   This is a block comment\n  spanning multiple lines\n    "
                        .to_string()
                },
            ]
        );

        Ok(())
    }
}
