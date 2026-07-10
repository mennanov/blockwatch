use crate::block_parser::{BlocksFromCommentsParser, BlocksParser};
use crate::language_parsers::{CommentsParser, TreeSitterCommentsParser};

/// Returns a [`BlocksParser`] for Ruby.
pub(super) fn parser() -> anyhow::Result<impl BlocksParser> {
    Ok(BlocksFromCommentsParser::new(comments_parser()?))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let ruby_language = tree_sitter_ruby::LANGUAGE.into();
    let parser = TreeSitterCommentsParser::new(
        &ruby_language,
        Box::new(|node, source_code| {
            if node.kind() != "comment" {
                return None;
            }
            let comment = &source_code[node.byte_range()];
            Some(if comment.starts_with('#') {
                comment.replacen('#', " ", 1)
            } else {
                multiline_comment_processor(comment)
            })
        }),
    );
    Ok(parser)
}

/// Replaces the `=begin` and `=end` markers of a Ruby multiline comment with the corresponding
/// number of whitespaces, preserving the comment's length.
fn multiline_comment_processor(comment: &str) -> String {
    let mut result = String::with_capacity(comment.len());
    // Replace the leading "=begin" with spaces; any text after it on the same line is content.
    result.push_str("      ");
    // The closing "=end" always starts the comment's last line.
    let last_line_idx = comment
        .rfind('\n')
        .expect("expected '\\n' in a multiline comment")
        + 1;
    result.push_str(&comment[6..last_line_idx]);
    // Replace "=end" with spaces.
    result.push_str("    ");
    result.push_str(&comment[last_line_idx + 4..]);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Position, language_parsers::Comment};

    #[test]
    fn parses_comments_correctly() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser()?;

        let blocks: Vec<Comment> = comments_parser
            .parse(
                r#"
def main
    # This is a single line comment
    puts "Hello, # this is not a comment"  # This is an inline comment

# This is a multi-line
# comment that spans
# several lines

value = 42  # Comment after code
"#,
            )
            .collect();

        assert_eq!(
            blocks,
            vec![
                Comment {
                    position_range: Position::new(3, 5)..Position::new(3, 36),
                    source_range: 14..45,
                    comment_text: "  This is a single line comment".to_string()
                },
                Comment {
                    position_range: Position::new(4, 44)..Position::new(4, 71),
                    source_range: 89..116,
                    comment_text: "  This is an inline comment".to_string()
                },
                Comment {
                    position_range: Position::new(6, 1)..Position::new(6, 23),
                    source_range: 118..140,
                    comment_text: "  This is a multi-line".to_string()
                },
                Comment {
                    position_range: Position::new(7, 1)..Position::new(7, 21),
                    source_range: 141..161,
                    comment_text: "  comment that spans".to_string()
                },
                Comment {
                    position_range: Position::new(8, 1)..Position::new(8, 16),
                    source_range: 162..177,
                    comment_text: "  several lines".to_string()
                },
                Comment {
                    position_range: Position::new(10, 13)..Position::new(10, 33),
                    source_range: 191..211,
                    comment_text: "  Comment after code".to_string()
                }
            ]
        );

        Ok(())
    }

    #[test]
    fn preserves_content_of_multiline_comments() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser()?;

        // The `=begin`/`=end` markers are blanked; everything else — including a `#` and the
        // text after `=begin` — is content and must be preserved.
        let blocks: Vec<Comment> = comments_parser
            .parse(
                r#"
=begin rdoc
This mentions # a hash and
pattern ^# stays intact
=end
value = 1
"#,
            )
            .collect();

        assert_eq!(
            blocks,
            vec![Comment {
                position_range: Position::new(2, 1)..Position::new(5, 5),
                source_range: 1..68,
                comment_text:
                    "       rdoc\nThis mentions # a hash and\npattern ^# stays intact\n    "
                        .to_string()
            }]
        );

        Ok(())
    }
}
