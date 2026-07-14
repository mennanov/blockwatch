use crate::block_parser::{BlocksFromCommentsParser, BlocksParser};
use crate::language_parsers::{CommentsParser, TreeSitterCommentsParser};
use tree_sitter::Node;

/// Returns a [`BlocksParser`] for CMake.
pub(super) fn parser() -> anyhow::Result<impl BlocksParser> {
    Ok(BlocksFromCommentsParser::new(comments_parser()?))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let cmake_language = tree_sitter_cmake::LANGUAGE.into();
    let parser = TreeSitterCommentsParser::new(
        &cmake_language,
        Box::new(|node, source_code| match node.kind() {
            "line_comment" => Some(source_code[node.byte_range()].replacen('#', " ", 1)),
            "bracket_comment" => Some(bracket_comment_text(node, &source_code[node.byte_range()])),
            _ => None,
        }),
    );
    Ok(parser)
}

/// Blanks out the bracket comment delimiters (`#[=*[`, `]=*]`) around the
/// `bracket_comment_content` child, preserving the comment's length.
fn bracket_comment_text(node: &Node, comment: &str) -> String {
    let mut cursor = node.walk();
    let content_range = node
        .named_children(&mut cursor)
        .find(|child| child.kind() == "bracket_comment_content")
        .map(|child| child.start_byte() - node.start_byte()..child.end_byte() - node.start_byte())
        .unwrap_or(0..0);
    let mut result = " ".repeat(content_range.start);
    result.push_str(&comment[content_range.clone()]);
    result.push_str(&" ".repeat(comment.len() - content_range.end));
    result
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
# This is a line comment
add_library(foo foo.c) # inline comment

#[[ This is a bracket comment
spanning multiple lines
]]

#[=[ Bracketed
comment ]=]
"#,
            )
            .collect();

        assert_eq!(
            comments,
            vec![
                Comment {
                    position_range: Position::new(2, 1)..Position::new(2, 25),
                    source_range: 1..25,
                    comment_text: "  This is a line comment".to_string()
                },
                Comment {
                    position_range: Position::new(3, 24)..Position::new(3, 40),
                    source_range: 49..65,
                    comment_text: "  inline comment".to_string()
                },
                Comment {
                    position_range: Position::new(5, 1)..Position::new(7, 3),
                    source_range: 67..123,
                    comment_text: "    This is a bracket comment\nspanning multiple lines\n  "
                        .to_string()
                },
                Comment {
                    position_range: Position::new(9, 1)..Position::new(10, 12),
                    source_range: 125..151,
                    comment_text: "     Bracketed\ncomment    ".to_string()
                },
            ]
        );

        Ok(())
    }
}
