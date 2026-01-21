use crate::block_parser::{BlocksFromCommentsParser, BlocksParser};
use crate::language_parsers::{CommentsParser, python_style_comments_parser};

/// Returns a [`BlocksParser`] for Yaml.
pub(super) fn parser() -> anyhow::Result<impl BlocksParser> {
    Ok(BlocksFromCommentsParser::new(comments_parser()?))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let yaml_language = tree_sitter_yaml::LANGUAGE.into();
    let parser = python_style_comments_parser(&yaml_language, "comment");
    Ok(parser)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Position, language_parsers::Comment};

    #[test]
    fn parses_yaml_comments_correctly() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser()?;

        let blocks: Vec<Comment> = comments_parser
            .parse(
                r#"
# This is a YAML comment
key: value  # Inline comment on a key-value pair

# Another comment
list:
  - item1  # Comment in a list
  - item2
# End of comments
"#,
            )
            .collect();

        assert_eq!(
            blocks,
            vec![
                Comment {
                    position_range: Position::new(2, 1)..Position::new(2, 25),
                    source_range: 1..25,
                    comment_text: "  This is a YAML comment".to_string()
                },
                Comment {
                    position_range: Position::new(3, 13)..Position::new(3, 49),
                    source_range: 38..74,
                    comment_text: "  Inline comment on a key-value pair".to_string()
                },
                Comment {
                    position_range: Position::new(5, 1)..Position::new(5, 18),
                    source_range: 76..93,
                    comment_text: "  Another comment".to_string()
                },
                Comment {
                    position_range: Position::new(7, 12)..Position::new(7, 31),
                    source_range: 111..130,
                    comment_text: "  Comment in a list".to_string()
                },
                Comment {
                    position_range: Position::new(9, 1)..Position::new(9, 18),
                    source_range: 141..158,
                    comment_text: "  End of comments".to_string()
                }
            ]
        );

        Ok(())
    }
}
