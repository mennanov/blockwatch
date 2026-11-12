use crate::language_parsers::{
    CommentsParser, LanguageParser, LanguageParserImpl, TreeSitterCommentsParser,
    blank_line_marker, blank_outside, comment_visitor, node_text,
};
use tree_sitter::Node;

/// Returns a [`LanguageParser`] for Lua.
pub(super) fn parser() -> impl LanguageParser {
    LanguageParserImpl::new(comments_parser())
}

fn comments_parser() -> impl CommentsParser {
    let lua_language = tree_sitter_lua::LANGUAGE.into();
    TreeSitterCommentsParser::new(
        &lua_language,
        comment_visitor(|node, source_code| {
            if node.kind() != "comment" {
                return None;
            }
            Some(lua_comment_text(node, node_text(node, source_code)))
        }),
    )
}

/// `comment`, the text of `node`, with its markers blanked, so it keeps its length: the `--` of a
/// line comment, or the `--[=*[` and `]=*]` around the `content` field of a block comment.
fn lua_comment_text(node: &Node, comment: &str) -> String {
    // Only a block comment has an `end` field. A line comment's `content` starts right after `--`,
    // so it would keep the third `-` of `---`.
    if node.child_by_field_name("end").is_none() {
        return blank_line_marker(comment, &["--"])
            .expect("tree-sitter-lua starts a comment with `--`");
    }
    let content = node
        .child_by_field_name("content")
        .expect("tree-sitter-lua gives every comment a `content` field");
    let content_range =
        content.start_byte() - node.start_byte()..content.end_byte() - node.start_byte();
    blank_outside(comment, content_range)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Position, language_parsers::Comment};

    #[test]
    fn parses_comments_correctly() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser();

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
--- LuaLS reads this marker as a doc comment, as in ---@param.
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
                Comment {
                    position_range: Position::new(12, 1)..Position::new(12, 63),
                    source_range: 153..215,
                    comment_text: "    LuaLS reads this marker as a doc comment, as in ---@param."
                        .to_string()
                },
            ]
        );

        Ok(())
    }
}
