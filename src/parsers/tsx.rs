use crate::parsers::{
    BlocksFromCommentsParser, BlocksParser, CommentsParser, TreeSitterCommentsParser,
};
use tree_sitter::Query;

/// Returns a [`BlocksParser`] for TypeScript TSX.
pub(crate) fn parser() -> anyhow::Result<Box<dyn BlocksParser>> {
    Ok(Box::new(BlocksFromCommentsParser::new(comments_parser()?)))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let tsx_language = tree_sitter_typescript::LANGUAGE_TSX.into();
    let block_comment_query = Query::new(&tsx_language, "(comment) @comment")?;
    let parser = TreeSitterCommentsParser::<fn(usize, &str) -> Option<String>>::new(
        tsx_language,
        vec![(
            block_comment_query,
            Some(|_, comment| {
                if comment.starts_with("//") {
                    Some(comment.strip_prefix("//").unwrap().trim().to_string())
                } else {
                    Some(
                        comment
                            .strip_prefix("/*")
                            .unwrap()
                            .lines()
                            .map(|line| {
                                line.trim_start()
                                    .trim_start_matches("*")
                                    .trim()
                                    .trim_end_matches("/")
                                    .trim_end_matches("*")
                                    .trim()
                            })
                            .collect::<Vec<_>>()
                            .join("\n"),
                    )
                }
            }),
        )],
    );
    Ok(parser)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tsx_comments_correctly() -> anyhow::Result<()> {
        let comments_parser = comments_parser()?;

        let blocks = comments_parser.parse(
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
                "#,
        )?;

        assert_eq!(
            blocks,
            vec![
                (
                    2,
                    "\nThis is a TSX component with comments.\n\n@component TSXExample\n"
                        .to_string()
                ),
                (8, "This is a single-line comment in TSX.".to_string()),
                (
                    11,
                    "\nThis is a multi-line comment\nused in a functional component.\n".to_string()
                ),
                (16, "Inline multi-line comment".to_string()),
                (19, "Render the component".to_string()),
                (22, "JSX single-line comment".to_string()),
                (24, "JSX multi-line\ncomment".to_string()),
            ]
        );

        Ok(())
    }
}
