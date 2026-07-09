use crate::block_parser::{BlocksFromCommentsParser, BlocksParser};
use crate::language_parsers;
use crate::language_parsers::CommentsParser;

/// Returns a [`BlocksParser`] for HCL (Terraform).
pub(super) fn parser() -> anyhow::Result<impl BlocksParser> {
    Ok(BlocksFromCommentsParser::new(comments_parser()?))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let hcl_language = tree_sitter_hcl::LANGUAGE.into();
    let parser = language_parsers::hash_and_c_style_comments_parser(&hcl_language, "comment");
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
variable "region" {
  type = string // inline slash comment
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
                    position_range: Position::new(2, 1)..Position::new(2, 25),
                    source_range: 1..25,
                    comment_text: "  This is a hash comment".to_string()
                },
                Comment {
                    position_range: Position::new(4, 17)..Position::new(4, 40),
                    source_range: 62..85,
                    comment_text: "   inline slash comment".to_string()
                },
                Comment {
                    position_range: Position::new(7, 1)..Position::new(9, 3),
                    source_range: 89..142,
                    comment_text: "   This is a block comment\nspanning multiple lines\n  "
                        .to_string()
                },
            ]
        );

        Ok(())
    }
}
