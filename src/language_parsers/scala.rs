use crate::language_parsers;
use crate::language_parsers::{CommentsParser, LanguageParser, LanguageParserImpl};

/// Returns a [`LanguageParser`] for Scala.
pub(super) fn parser() -> impl LanguageParser {
    LanguageParserImpl::new(comments_parser())
}

fn comments_parser() -> impl CommentsParser {
    let scala_language = tree_sitter_scala::LANGUAGE.into();
    language_parsers::c_style_comments_parser(
        &scala_language,
        &["comment", "block_comment"],
        language_parsers::SLASH_MARKERS,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Position, language_parsers::Comment};

    #[test]
    fn block_marker_inside_nested_comment_is_read_once() -> anyhow::Result<()> {
        // A marker in a nested Scala block comment must be read once, not once per nesting level.
        let contents = r#"/* wrapper
   /* <block name="only_once"> */
*/
val value = 1
// </block>
"#;
        let blocks = parser()
            .parse_blocks(contents)
            .collect::<anyhow::Result<Vec<_>>>()?;
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].attributes["name"], "only_once");
        Ok(())
    }

    #[test]
    fn parses_comments_correctly() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser();

        // The grammar reads the shebang, which must start the file, as a comment with no marker
        // to blank. So its text stays as written.
        let comments: Vec<Comment> = comments_parser
            .parse(
                r#"#!/usr/bin/env -S scala-cli shebang
// This is a single line comment
object Main {
  val x = 42 // inline comment

  /* This is a block comment
  spanning multiple lines
  */
}
/// Triple slash comment.
//! Rust and Doxygen read this marker as a doc comment.
//// A banner line repeats the marker.
"#,
            )
            .collect();

        assert_eq!(
            comments,
            vec![
                Comment {
                    position_range: Position::new(1, 1)..Position::new(1, 36),
                    source_range: 0..35,
                    comment_text: "#!/usr/bin/env -S scala-cli shebang".to_string()
                },
                Comment {
                    position_range: Position::new(2, 1)..Position::new(2, 33),
                    source_range: 36..68,
                    comment_text: "   This is a single line comment".to_string()
                },
                Comment {
                    position_range: Position::new(4, 14)..Position::new(4, 31),
                    source_range: 96..113,
                    comment_text: "   inline comment".to_string()
                },
                Comment {
                    position_range: Position::new(6, 3)..Position::new(8, 5),
                    source_range: 117..174,
                    comment_text: "   This is a block comment\n  spanning multiple lines\n    "
                        .to_string()
                },
                Comment {
                    position_range: Position::new(10, 1)..Position::new(10, 26),
                    source_range: 177..202,
                    comment_text: "    Triple slash comment.".to_string()
                },
                Comment {
                    position_range: Position::new(11, 1)..Position::new(11, 56),
                    source_range: 203..258,
                    comment_text: "    Rust and Doxygen read this marker as a doc comment."
                        .to_string()
                },
                Comment {
                    position_range: Position::new(12, 1)..Position::new(12, 39),
                    source_range: 259..297,
                    comment_text: "     A banner line repeats the marker.".to_string()
                },
            ]
        );

        Ok(())
    }

    #[test]
    fn unclosed_block_comment_runs_to_end_of_file() -> anyhow::Result<()> {
        // The `*/` in `/*/` overlaps the `/*`, so it does not close the comment.
        let contents = r#"// <block name="a">
val value = 1
/*/ </block>
"#;
        let blocks = parser()
            .parse_blocks(contents)
            .collect::<anyhow::Result<Vec<_>>>()?;
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].content.text(contents), "\nval value = 1\n");
        Ok(())
    }
}
