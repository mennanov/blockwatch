use crate::language_parsers::{
    CommentsParser, LanguageParser, LanguageParserImpl, TreeSitterCommentsParser,
    blank_decorative_stars, comment_visitor, node_text,
};

/// Returns a [`LanguageParser`] for OCaml.
pub(super) fn parser() -> anyhow::Result<impl LanguageParser> {
    Ok(LanguageParserImpl::new(comments_parser()?))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let ocaml_language = tree_sitter_ocaml::LANGUAGE_OCAML.into();
    let parser = TreeSitterCommentsParser::new(
        &ocaml_language,
        comment_visitor(|node, source_code| {
            if node.kind() != "comment" {
                return None;
            }
            Some(comment_text(node_text(node, source_code)))
        }),
    )?;
    Ok(parser)
}

/// Blanks the `(*` and `*)` around an OCaml comment, and the decorative `*` at the start of each
/// line, preserving the comment's length. A comment nested inside keeps its own `(*` and `*)`.
fn comment_text(comment: &str) -> String {
    let content = comment
        .strip_prefix("(*")
        .expect("expected '(*' at the start of a comment");
    // The grammar runs a comment that is never closed to the end of the file, so it may have no
    // `*)`. Stripping the opener first keeps the `*` of `(*)` from also closing the comment.
    let (content, closer) = match content.strip_suffix("*)") {
        Some(content) => (content, "  "),
        None => (content, ""),
    };
    format!("  {}{closer}", blank_decorative_stars(content))
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
(* This is a comment *)
let x = 42 (* inline comment *)

(** This is a doc comment *)
let s = "(* not a comment *)" ^ {|(* not a comment *)|}

(* This is a comment
 * spanning multiple lines *)
"#,
            )
            .collect();

        assert_eq!(
            comments,
            vec![
                Comment {
                    position_range: Position::new(2, 1)..Position::new(2, 24),
                    source_range: 1..24,
                    comment_text: "   This is a comment   ".to_string()
                },
                Comment {
                    position_range: Position::new(3, 12)..Position::new(3, 32),
                    source_range: 36..56,
                    comment_text: "   inline comment   ".to_string()
                },
                Comment {
                    position_range: Position::new(5, 1)..Position::new(5, 29),
                    source_range: 58..86,
                    comment_text: "    This is a doc comment   ".to_string()
                },
                Comment {
                    position_range: Position::new(8, 1)..Position::new(9, 30),
                    source_range: 144..194,
                    comment_text: "   This is a comment\n   spanning multiple lines   ".to_string()
                },
            ]
        );

        Ok(())
    }

    #[test]
    fn nested_comment_is_one_comment() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser()?;

        let comments: Vec<Comment> = comments_parser
            .parse(
                r#"(* outer (* inner *) "*)" still outer *)
let x = 1
"#,
            )
            .collect();

        assert_eq!(
            comments,
            vec![Comment {
                position_range: Position::new(1, 1)..Position::new(1, 41),
                source_range: 0..40,
                comment_text: r#"   outer (* inner *) "*)" still outer   "#.to_string()
            }]
        );

        Ok(())
    }

    #[test]
    fn unclosed_comment_end_is_kept() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser()?;

        // In `(*)`, the `*` belongs to the opener, so the `)` does not close the comment, and the
        // comment runs to the end of the file.
        let comments: Vec<Comment> = comments_parser.parse("let x = 1\n(*)").collect();

        assert_eq!(
            comments,
            vec![Comment {
                position_range: Position::new(2, 1)..Position::new(2, 4),
                source_range: 10..13,
                comment_text: "  )".to_string()
            }]
        );

        Ok(())
    }
}
