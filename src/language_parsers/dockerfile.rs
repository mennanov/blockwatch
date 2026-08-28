use crate::block_parser::{BlocksFromCommentsParser, BlocksParser};
use crate::language_parsers;
use crate::language_parsers::CommentsParser;

/// Returns a [`BlocksParser`] for Dockerfile (Containerfile).
pub(super) fn parser() -> anyhow::Result<impl BlocksParser> {
    Ok(BlocksFromCommentsParser::new(comments_parser()?))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let containerfile_language = tree_sitter_containerfile::LANGUAGE.into();
    let parser = language_parsers::python_style_comments_parser(&containerfile_language, "comment")
        .with_break_at_node_kinds(&["double_quoted_string", "single_quoted_string"])
        // Because of the bug in the Dockerfile tree-sitter grammar, the strings like
        // "# not a comment" are lexed as a comment. This is a workaround to ignore them.
        .with_break_when(|node, source_code| {
            node.kind() == "comment"
                && node.start_byte() > 0
                && source_code.as_bytes()[node.start_byte() - 1] == b'"'
        });
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

    #[test]
    fn hash_inside_a_double_quoted_argument_is_not_a_comment() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser()?;

        let comments: Vec<String> = comments_parser
            .parse(
                r##"FROM alpine:3.20
LABEL description="# 1 of a kind"

# The only comment here
COPY . /app
"##,
            )
            .map(|comment| comment.comment_text)
            .collect();

        assert_eq!(comments, vec!["  The only comment here\n".to_string()]);

        Ok(())
    }

    #[test]
    fn hash_inside_a_single_quoted_argument_is_not_a_comment() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser()?;

        let comments: Vec<String> = comments_parser
            .parse(
                r##"FROM alpine:3.20
LABEL summary='# also not a comment'

# The only comment here
COPY . /app
"##,
            )
            .map(|comment| comment.comment_text)
            .collect();

        assert_eq!(comments, vec!["  The only comment here\n".to_string()]);

        Ok(())
    }

    #[test]
    fn hash_opening_a_json_array_argument_is_not_a_comment() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser()?;

        let comments: Vec<String> = comments_parser
            .parse(
                r##"FROM alpine:3.20
ENTRYPOINT ["#/bin/sh", "-c", "# neither of these is a comment"]

# The only comment here
COPY . /app
"##,
            )
            .map(|comment| comment.comment_text)
            .collect();

        assert_eq!(comments, vec!["  The only comment here\n".to_string()]);

        Ok(())
    }

    #[test]
    fn real_block_is_parsed_while_json_array_marker_is_ignored() -> anyhow::Result<()> {
        let contents = r##"FROM alpine:3.20
CMD ["sh", "-c", "# <block name='fake'> x # </block>"]
# <block name="real">
COPY . /app
# </block>
"##;
        let blocks = parser()?
            .parse(contents)
            .collect::<anyhow::Result<Vec<_>>>()?;
        let names: Vec<&str> = blocks
            .iter()
            .map(|block| block.attributes["name"].as_str())
            .collect();

        assert_eq!(names, ["real"]);

        Ok(())
    }
}
