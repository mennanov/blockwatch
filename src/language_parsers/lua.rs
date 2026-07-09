use crate::block_parser::{BlocksFromCommentsParser, BlocksParser};
use crate::language_parsers::{CommentsParser, TreeSitterCommentsParser};
use tree_sitter::Node;

/// Returns a [`BlocksParser`] for Lua.
pub(super) fn parser() -> anyhow::Result<impl BlocksParser> {
    Ok(BlocksFromCommentsParser::new(comments_parser()?))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let lua_language = tree_sitter_lua::LANGUAGE.into();
    let parser = TreeSitterCommentsParser::new(
        &lua_language,
        Box::new(|node, source_code| {
            if node.kind() != "comment" {
                return None;
            }
            Some(lua_comment_text(node, &source_code[node.byte_range()]))
        }),
    );
    Ok(parser)
}

/// Blanks out the comment delimiters (`--`, `--[=*[`, `]=*]`) around the `content` field,
/// preserving the comment's length. A bare `--` has no `content` node.
fn lua_comment_text(node: &Node, comment: &str) -> String {
    let content_range = match node.child_by_field_name("content") {
        Some(content) => {
            content.start_byte() - node.start_byte()..content.end_byte() - node.start_byte()
        }
        None => 0..0,
    };
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
-- This is a line comment
local x = 42 -- inline comment

--[[ This is a block comment
spanning multiple lines
]]

--[=[ Bracketed
comment ]=]
print(x)
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
                    position_range: Position::new(3, 14)..Position::new(3, 31),
                    source_range: 40..57,
                    comment_text: "   inline comment".to_string()
                },
                Comment {
                    position_range: Position::new(5, 1)..Position::new(7, 3),
                    source_range: 59..114,
                    comment_text: "     This is a block comment\nspanning multiple lines\n  "
                        .to_string()
                },
                Comment {
                    position_range: Position::new(9, 1)..Position::new(10, 12),
                    source_range: 116..143,
                    comment_text: "      Bracketed\ncomment    ".to_string()
                },
            ]
        );

        Ok(())
    }
}
