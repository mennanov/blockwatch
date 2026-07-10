use crate::block_parser::{BlocksFromCommentsParser, BlocksParser};
use crate::language_parsers;
use crate::language_parsers::CommentsParser;

/// Returns a [`BlocksParser`] for TypeScript TSX.
pub(super) fn parser() -> anyhow::Result<impl BlocksParser> {
    Ok(BlocksFromCommentsParser::new(comments_parser()?))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let tsx_language = tree_sitter_typescript::LANGUAGE_TSX.into();
    let parser = language_parsers::c_style_and_html_comments_parser(
        &tsx_language,
        "comment",
        "html_comment",
    );
    Ok(parser)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Position, language_parsers::Comment};

    #[test]
    fn parses_tsx_comments_correctly() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser()?;

        // The HTML-like comments are single-line: the statement between the `<!--` and `-->`
        // lines is code, not comment content.
        let blocks: Vec<Comment> = comments_parser
            .parse(
                r#"
                /**
                 * This is a TSX component with comments.
                 *
                 * @component TSXExample
                 */
                const TSXExample = () => {
                    // This is a single-line comment in TSX.
                    const [count, setCount] = useState(0);

                    /*
                     * This is a multi-line comment
                     * used in a functional component.
                     */
                    const increment = () => {
                        setCount(count + 1); /* Inline multi-line comment */
                    };

                    // Render the component
                    return (
                        <div>
                            {/* JSX single-line comment */}
                            <p>Current count: {count}</p>
                            {/* JSX multi-line 
                            comment */}
                            <button onClick={increment}>Increment</button>
                        </div>
                    );
                };
/// Triple slash comment.
<!-- html open comment
let between = 2;
--> html close comment
                "#,
            )
            .collect();

        assert_eq!(
            blocks,
            vec![
                Comment {
                    position_range: Position::new(2, 17)..Position::new(6, 20),
                    source_range: 17..158,
                    comment_text:
                        "   \n                   This is a TSX component with comments.\n                  \n                   @component TSXExample\n                   "
                            .to_string()
                },
                Comment {
                    position_range: Position::new(8, 21)..Position::new(8, 61),
                    source_range: 222..262,
                    comment_text: "   This is a single-line comment in TSX.".to_string()
                },
                Comment {
                    position_range: Position::new(11, 21)..Position::new(14, 24),
                    source_range: 343..476,
                    comment_text:
                        "  \n                       This is a multi-line comment\n                       used in a functional component.\n                       "
                            .to_string()
                },
                Comment {
                    position_range: Position::new(16, 46)..Position::new(16, 77),
                    source_range: 568..599,
                    comment_text: "   Inline multi-line comment   ".to_string()
                },
                Comment {
                    position_range: Position::new(19, 21)..Position::new(19, 44),
                    source_range: 644..667,
                    comment_text: "   Render the component".to_string()
                },
                Comment {
                    position_range: Position::new(22, 30)..Position::new(22, 59),
                    source_range: 756..785,
                    comment_text: "   JSX single-line comment   ".to_string()
                },
                Comment {
                    position_range: Position::new(24, 30)..Position::new(25, 39),
                    source_range: 874..931,
                    comment_text: "   JSX multi-line \n                            comment   ".to_string()
                },
                Comment {
                    position_range: Position::new(30, 1)..Position::new(30, 26),
                    source_range: 1081..1106,
                    comment_text: "  / Triple slash comment.".to_string()
                },
                Comment {
                    position_range: Position::new(31, 1)..Position::new(31, 23),
                    source_range: 1107..1129,
                    comment_text: "     html open comment".to_string()
                },
                Comment {
                    position_range: Position::new(33, 1)..Position::new(33, 23),
                    source_range: 1147..1169,
                    comment_text: "    html close comment".to_string()
                },
            ]
        );

        Ok(())
    }
}
