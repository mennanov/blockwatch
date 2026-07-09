use crate::block_parser::{BlocksFromCommentsParser, BlocksParser};
use crate::language_parsers;
use crate::language_parsers::CommentsParser;

/// Returns a [`BlocksParser`] for Dockerfile (Containerfile).
pub(super) fn parser() -> anyhow::Result<impl BlocksParser> {
    Ok(BlocksFromCommentsParser::new(comments_parser()?))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let containerfile_language = tree_sitter_containerfile::LANGUAGE.into();
    let parser = language_parsers::python_style_comments_parser(&containerfile_language, "comment");
    Ok(parser)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Position, language_parsers::Comment};

    #[test]
    fn parses_comments_correctly() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser()?;

        // The `#` in the `RUN` instruction belongs to the shell command and must not be
        // extracted as a comment. Note that the grammar includes the trailing newline in
        // each comment node.
        let comments: Vec<Comment> = comments_parser
            .parse(
                r#"
# syntax=docker/dockerfile:1
FROM alpine:3.20

# This is a comment
RUN echo hi # not a comment

  # Indented comment
COPY . /app
"#,
            )
            .collect();

        assert_eq!(
            comments,
            vec![
                Comment {
                    position_range: Position::new(2, 1)..Position::new(3, 1),
                    source_range: 1..30,
                    comment_text: "  syntax=docker/dockerfile:1\n".to_string()
                },
                Comment {
                    position_range: Position::new(5, 1)..Position::new(6, 1),
                    source_range: 48..68,
                    comment_text: "  This is a comment\n".to_string()
                },
                Comment {
                    position_range: Position::new(8, 3)..Position::new(9, 1),
                    source_range: 99..118,
                    comment_text: "  Indented comment\n".to_string()
                },
            ]
        );

        Ok(())
    }
}
