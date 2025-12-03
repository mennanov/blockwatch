use crate::blocks::Block;
use crate::language_parsers::{Comment, CommentsParser};
use crate::tag_parser::{BlockTag, BlockTagParser, WinnowBlockTagParser};
use std::collections::HashMap;
use std::ops::Range;

/// Parses [`Blocks`] from a source code.
pub trait BlocksParser {
    /// Returns [`Block`]s extracted from the given `contents` string.
    ///
    /// The blocks are required to be sorted by the `starts_at` field in ascending order.
    fn parse(&self, contents: &str) -> anyhow::Result<Vec<Block>>;
}

pub struct BlocksFromCommentsParser<C: CommentsParser> {
    comments_parser: C,
}

impl<C: CommentsParser> BlocksFromCommentsParser<C> {
    pub(crate) fn new(comments_parser: C) -> Self {
        Self { comments_parser }
    }

    fn process_start_tag<'c>(
        comment: &'c Comment,
        stack: &mut Vec<BlockBuilder<'c>>,
        start_position: usize,
        end_position: usize,
        attributes: HashMap<String, String>,
    ) {
        let start_tag_range = comment.source_start_position + start_position
            ..comment.source_start_position + end_position;
        let starts_at_line = comment.source_line_number
            + comment.comment_text[..start_position + 1].lines().count()
            - 1;
        stack.push(BlockBuilder::new(
            starts_at_line,
            comment,
            attributes,
            start_tag_range,
        ));
    }

    fn process_end_tag<'c>(
        comment: &'c Comment,
        stack: &mut Vec<BlockBuilder<'c>>,
        blocks: &mut Vec<Block>,
        start_position: usize,
    ) -> anyhow::Result<()> {
        if let Some(block_builder) = stack.pop() {
            let content_range = if !std::ptr::eq(comment, block_builder.comment) {
                block_builder.comment.source_end_position..comment.source_start_position
            } else {
                // Block that starts and ends in the same comment can't have any
                // content.
                0..0
            };
            let end_line_number = comment.source_line_number
                + comment.comment_text[..start_position + 1].lines().count()
                - 1;
            blocks.push(block_builder.build(end_line_number, content_range));
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "Unexpected closed block at line {}, position {}",
                comment.source_line_number,
                comment.source_start_position + start_position
            ))
        }
    }
}

impl<C: CommentsParser> BlocksParser for BlocksFromCommentsParser<C> {
    fn parse(&self, contents: &str) -> anyhow::Result<Vec<Block>> {
        let comments = self.comments_parser.parse(contents)?;
        let mut blocks = Vec::new();
        let mut stack = Vec::new();
        for comment in &comments {
            let mut parser = WinnowBlockTagParser::new(&comment.comment_text);

            while let Some(tag) = parser.next()? {
                match tag {
                    BlockTag::Start {
                        start_position,
                        end_position,
                        attributes,
                    } => {
                        Self::process_start_tag(
                            comment,
                            &mut stack,
                            start_position,
                            end_position,
                            attributes,
                        );
                    }
                    BlockTag::End { start_position, .. } => {
                        Self::process_end_tag(comment, &mut stack, &mut blocks, start_position)?;
                    }
                }
            }
        }

        if let Some(unclosed_block) = stack.pop() {
            return Err(anyhow::anyhow!(format!(
                "Block at line {} is not closed",
                unclosed_block.comment.source_line_number
            )));
        }
        blocks.sort_by(|a, b| a.starts_at_line.cmp(&b.starts_at_line));

        Ok(blocks)
    }
}

struct BlockBuilder<'c> {
    starts_at_line: usize,
    comment: &'c Comment,
    attributes: HashMap<String, String>,
    start_tag_range: Range<usize>,
}

impl<'c> BlockBuilder<'c> {
    fn new(
        starts_at_line: usize,
        comment: &'c Comment,
        attributes: HashMap<String, String>,
        start_tag_range: Range<usize>,
    ) -> Self {
        Self {
            starts_at_line,
            comment,
            attributes,
            start_tag_range,
        }
    }

    /// Finalizes the block with the given end line and captured content, producing a `Block`.
    pub(crate) fn build(self, ends_at_line: usize, content_range: Range<usize>) -> Block {
        Block::new(
            self.starts_at_line,
            ends_at_line,
            self.attributes,
            self.start_tag_range,
            content_range,
        )
    }
}

#[cfg(test)]
mod tests {
    use crate::block_parser::BlocksParser;
    use crate::blocks::Block;
    use crate::{language_parsers, test_utils};
    use std::collections::HashMap;

    fn create_parser() -> impl BlocksParser {
        // Reuse existing real blocks parser.
        language_parsers::rust::parser().unwrap()
    }

    #[test]
    fn no_defined_blocks_returns_empty_blocks() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = r#""
            fn say_hello_world() {
              println!("hello world!");
            }
        "#;
        let blocks = parser.parse(contents)?;
        assert_eq!(blocks, vec![]);
        Ok(())
    }

    #[test]
    fn single_block_with_single_line_content() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = "/* <block> */ let say = \"hi\"; /* </block> */";
        let blocks = parser.parse(contents)?;
        assert_eq!(
            blocks,
            vec![Block::new(
                1,
                1,
                HashMap::new(),
                test_utils::substr_range(contents, "<block>"),
                test_utils::substr_range(contents, " let say = \"hi\"; "),
            ),]
        );
        Ok(())
    }

    #[test]
    fn single_block_with_multiple_lines_content() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = "// <block>\nlet say = \"hi\";\n// </block>";
        let blocks = parser.parse(contents)?;
        assert_eq!(
            blocks,
            vec![Block::new(
                1,
                3,
                HashMap::new(),
                test_utils::substr_range(contents, "<block>"),
                test_utils::substr_range(contents, "\nlet say = \"hi\";\n")
            ),]
        );
        Ok(())
    }

    #[test]
    fn single_block_with_multiline_starting_block_tag() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = "/* <block\n> */ let say = \"hi\"; // </block>";
        let blocks = parser.parse(contents)?;
        assert_eq!(
            blocks,
            vec![Block::new(
                1,
                2,
                HashMap::new(),
                test_utils::substr_range(contents, "<block\n>"),
                test_utils::substr_range(contents, " let say = \"hi\"; ")
            ),]
        );
        Ok(())
    }

    #[test]
    fn single_block_with_multiline_ending_block_tag() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = "/* <block> */ let say = \"hi\"; /* </block\n> */";
        let blocks = parser.parse(contents)?;
        assert_eq!(
            blocks,
            vec![Block::new(
                1,
                1,
                HashMap::new(),
                test_utils::substr_range(contents, "<block>"),
                test_utils::substr_range(contents, " let say = \"hi\"; ")
            ),]
        );
        Ok(())
    }

    #[test]
    fn multiple_blocks_on_separate_lines() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = "// <block>\nprintln!(\"hello1\");\n// </block>
            // <block>\nprintln!(\"hello2\");\n// </block>";
        let blocks = parser.parse(contents)?;
        assert_eq!(
            blocks,
            vec![
                Block::new(
                    1,
                    3,
                    HashMap::new(),
                    test_utils::substr_range(contents, "<block>"),
                    test_utils::substr_range(contents, "\nprintln!(\"hello1\");\n"),
                ),
                Block::new(
                    4,
                    6,
                    HashMap::new(),
                    test_utils::substr_range_nth(contents, "<block>", 1),
                    test_utils::substr_range(contents, "\nprintln!(\"hello2\");\n")
                )
            ]
        );
        Ok(())
    }

    #[test]
    fn multiple_blocks_on_intersecting_lines() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = "// <block>\nprintln!(\"hello1\");\n/* </block><block> */\nprintln!(\"hello2\");\n// </block>";
        let blocks = parser.parse(contents)?;
        assert_eq!(
            blocks,
            vec![
                Block::new(
                    1,
                    3,
                    HashMap::new(),
                    test_utils::substr_range_nth(contents, "<block>", 0),
                    test_utils::substr_range(contents, "\nprintln!(\"hello1\");\n"),
                ),
                Block::new(
                    3,
                    5,
                    HashMap::new(),
                    test_utils::substr_range_nth(contents, "<block>", 1),
                    test_utils::substr_range(contents, "\nprintln!(\"hello2\");\n"),
                )
            ]
        );
        Ok(())
    }

    #[test]
    fn multiple_blocks_on_single_line() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = "/* <block> */println!(\"hello1\");/* </block><block> */println!(\"hello2\");// </block>";
        let blocks = parser.parse(contents)?;
        assert_eq!(
            blocks,
            vec![
                Block::new(
                    1,
                    1,
                    HashMap::new(),
                    test_utils::substr_range_nth(contents, "<block>", 0),
                    test_utils::substr_range(contents, "println!(\"hello1\");")
                ),
                Block::new(
                    1,
                    1,
                    HashMap::new(),
                    test_utils::substr_range_nth(contents, "<block>", 1),
                    test_utils::substr_range(contents, "println!(\"hello2\");")
                )
            ]
        );
        Ok(())
    }

    #[test]
    fn block_starts_on_non_first_comment_line() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = "/* Some comment\n<block> */println!(\"hello1\");// </block>";
        let blocks = parser.parse(contents)?;
        assert_eq!(
            blocks,
            vec![Block::new(
                2,
                2,
                HashMap::new(),
                test_utils::substr_range_nth(contents, "<block>", 0),
                test_utils::substr_range(contents, "println!(\"hello1\");")
            ),]
        );
        Ok(())
    }

    #[test]
    fn block_ends_on_non_first_comment_line() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = "/* <block> */println!(\"hello1\");/* Some comment\n</block> */";
        let blocks = parser.parse(contents)?;
        assert_eq!(
            blocks,
            vec![Block::new(
                1,
                2,
                HashMap::new(),
                test_utils::substr_range_nth(contents, "<block>", 0),
                test_utils::substr_range(contents, "println!(\"hello1\");")
            ),]
        );
        Ok(())
    }

    #[test]
    fn nested_blocks() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = r#"
        // <block name="foo">
        fn say_hello_world() {
          println!("hello world!");
        }

            // <block name="bar">
            fn say_hello_world_bar() {
              println!("hello world bar!");
            }
                // <block name="bar-bar">
                fn say_hello_world_bar_bar() {
                  println!("hello world bar bar!");
                }
                // </block>

            // </block>

            // <block name="buzz">
            fn say_hello_world_buzz() {
              println!("hello world buzz!");
            }
            // </block>

        // </block>
        // <block name="fizz">
        // </block>
        "#;
        let blocks = parser.parse(contents)?;
        assert_eq!(
            blocks,
            vec![
                Block::new(
                    2,
                    25,
                    HashMap::from([("name".to_string(), "foo".to_string())]),
                    test_utils::substr_range(contents, "<block name=\"foo\">"),
                    30..620
                ),
                Block::new(
                    7,
                    17,
                    HashMap::from([("name".to_string(), "bar".to_string())]),
                    test_utils::substr_range(contents, "<block name=\"bar\">"),
                    142..440
                ),
                Block::new(
                    11,
                    15,
                    HashMap::from([("name".to_string(), "bar-bar".to_string())]),
                    test_utils::substr_range(contents, "<block name=\"bar-bar\">"),
                    281..415
                ),
                Block::new(
                    19,
                    23,
                    HashMap::from([("name".to_string(), "buzz".to_string())]),
                    test_utils::substr_range(contents, "<block name=\"buzz\">"),
                    487..599
                ),
                Block::new(
                    26,
                    27,
                    HashMap::from([("name".to_string(), "fizz".to_string())]),
                    test_utils::substr_range(contents, "<block name=\"fizz\">"),
                    662..671
                ),
            ]
        );
        Ok(())
    }

    #[test]
    fn multiple_nested_blocks_at_same_level() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = r#"
        // <block name="parent">
            // <block name="child1">
            fn child1() {}
            // </block>
            // <block name="child2">
            fn child2() {}
            // </block>
            // <block name="child3">
            fn child3() {}
            // </block>
        // </block>
        "#;
        let blocks = parser.parse(contents)?;
        assert_eq!(blocks.len(), 4);
        assert_eq!(blocks[0].attributes["name"], "parent");
        assert_eq!(blocks[1].attributes["name"], "child1");
        assert_eq!(blocks[2].attributes["name"], "child2");
        assert_eq!(blocks[3].attributes["name"], "child3");
        Ok(())
    }

    #[test]
    fn block_contents_in_comments_is_ignored() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = r#"// <block name="foo">This text is ignored
        let word = "hello";
        // </block> Some comment."#;
        let blocks = parser.parse(contents)?;
        assert_eq!(
            blocks,
            vec![Block::new(
                1,
                3,
                HashMap::from([("name".to_string(), "foo".to_string())]),
                test_utils::substr_range(contents, "<block name=\"foo\">"),
                test_utils::substr_range(contents, "\n        let word = \"hello\";\n        ")
            ),]
        );
        Ok(())
    }

    #[test]
    fn unclosed_block_returns_error() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = r#"
        // <block name="foo">
        fn say_hello_world() {
          println!("hello world!");
        }
        "#;
        let error_message = parser.parse(contents).unwrap_err().to_string();
        assert_eq!(error_message, "Block at line 2 is not closed");
        Ok(())
    }

    #[test]
    fn unclosed_nested_block_returns_error() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = r#"
        // <block name="foo">
        fn say_hello_world() {
          println!("hello world!");
        }
            // <block name="bar">
            fn say_hello_world_bar() {
            }

        // </block>
        "#;
        let error_message = parser.parse(contents).unwrap_err().to_string();
        assert_eq!(error_message, "Block at line 2 is not closed");
        Ok(())
    }

    #[test]
    fn incorrect_endblock_returns_error() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = r#"
        fn say_hello_world() {
          println!("hello world!");
        }
        // </block>
        "#;
        let result = parser.parse(contents);
        assert!(result.is_err());
        Ok(())
    }

    #[test]
    fn attributes_on_single_line() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = r#"
        // <block foo="bar" fizz="buzz">
        fn foo() {
          println!("hello world!");
        }
        // </block>
        "#;
        let blocks = parser.parse(contents)?;
        assert_eq!(blocks.len(), 1);
        assert_eq!(
            blocks[0].attributes,
            HashMap::from([
                ("foo".to_string(), "bar".to_string()),
                ("fizz".to_string(), "buzz".to_string())
            ])
        );
        Ok(())
    }

    #[test]
    fn attributes_on_multiple_lines() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = r#"
        /* <block
            foo="bar"
            fizz="buzz"> */
        fn foo() {
          println!("hello world!");
        }
        // </block>
        "#;
        let blocks = parser.parse(contents)?;
        assert_eq!(blocks.len(), 1);
        assert_eq!(
            blocks[0].attributes,
            HashMap::from([
                ("foo".to_string(), "bar".to_string()),
                ("fizz".to_string(), "buzz".to_string())
            ])
        );
        Ok(())
    }

    #[test]
    fn attributes_with_single_quotes() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = r#"
        // <block text='He said "Hello"'>
        // </block>
        "#;
        let blocks = parser.parse(contents)?;
        assert_eq!(blocks[0].attributes["text"], "He said \"Hello\"");
        Ok(())
    }

    #[test]
    fn attributes_with_html_escaped_quotes_are_not_decoded() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = r#"
        // <block text="He said &quot;Hello&quot;">
        // </block>
        "#;
        let blocks = parser.parse(contents)?;

        assert_eq!(blocks[0].attributes["text"], "He said &quot;Hello&quot;");
        Ok(())
    }

    #[test]
    fn attributes_with_no_quotes() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = r#"
        // <block color=red flavor=sweet>
        // </block>
        "#;
        let blocks = parser.parse(contents)?;
        assert_eq!(blocks[0].attributes["color"], "red");
        assert_eq!(blocks[0].attributes["flavor"], "sweet");
        Ok(())
    }

    #[test]
    fn attributes_with_no_value() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = r#"
        // <block attr1 attr2>
        // </block>
        "#;
        let blocks = parser.parse(contents)?;
        assert_eq!(blocks[0].attributes["attr1"], "");
        assert_eq!(blocks[0].attributes["attr2"], "");
        Ok(())
    }

    #[test]
    fn attributes_with_empty_string_value() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = r#"
        // <block name="" foo="" bar=''>
        fn foo() {}
        // </block>
        "#;
        let blocks = parser.parse(contents)?;
        assert_eq!(
            blocks[0].attributes,
            HashMap::from([
                ("name".to_string(), "".to_string()),
                ("foo".to_string(), "".to_string()),
                ("bar".to_string(), "".to_string())
            ])
        );
        Ok(())
    }

    #[test]
    fn attributes_with_html_symbols() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = r#"
        // <block keep-unique="(?P<value>\w+)">
        // </block>
        "#;
        let blocks = parser.parse(contents)?;
        assert_eq!(blocks[0].attributes["keep-unique"], r"(?P<value>\w+)");
        Ok(())
    }

    #[test]
    fn attributes_with_unicode_value() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = r#"
        // <block name="🦀" desc="Rust">
        fn unicode() {}
        // </block>
        "#;
        let blocks = parser.parse(contents)?;
        assert_eq!(blocks[0].attributes["name"], "🦀");
        assert_eq!(blocks[0].attributes["desc"], "Rust");
        Ok(())
    }

    #[test]
    fn attributes_with_spaces_around_values() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = r#"
        // <block name = "foo" desc = 'bar'>
        fn unicode() {}
        // </block>
        "#;
        let blocks = parser.parse(contents)?;
        assert_eq!(blocks[0].attributes["name"], "foo");
        assert_eq!(blocks[0].attributes["desc"], "bar");
        Ok(())
    }

    #[test]
    fn multiple_mixed_attributes() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = r#"
        // <block color="red" attr1 align="center" attr2>
        fn escaped() {}
        // </block>
        "#;
        let blocks = parser.parse(contents)?;
        assert_eq!(blocks[0].attributes["color"], "red");
        assert_eq!(blocks[0].attributes["attr1"], "");
        assert_eq!(blocks[0].attributes["align"], "center");
        assert_eq!(blocks[0].attributes["attr2"], "");
        Ok(())
    }

    #[test]
    fn duplicated_attributes_uses_last_value() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = r#"
        // <block color="red" color="blue">
        fn escaped() {}
        // </block>
        "#;
        let blocks = parser.parse(contents)?;

        // Duplicate attributes: last value wins (standard HTML/XML behavior)
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].attributes.get("color"), Some(&"blue".to_string()));
        Ok(())
    }

    #[test]
    fn nested_blocks_with_attributes() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = r#"
        // <block name="outer" foo="bar">
        fn outer() {
            // <block name="inner" fizz="buzz">
            fn inner() {}
            // </block>
        }
        // </block>
        "#;
        let blocks = parser.parse(contents)?;
        assert_eq!(blocks.len(), 2);
        assert_eq!(
            blocks[0].attributes,
            HashMap::from([
                ("name".to_string(), "outer".to_string()),
                ("foo".to_string(), "bar".to_string()),
            ])
        );
        assert_eq!(
            blocks[1].attributes,
            HashMap::from([
                ("name".to_string(), "inner".to_string()),
                ("fizz".to_string(), "buzz".to_string())
            ])
        );
        Ok(())
    }

    #[test]
    fn malformed_block_tag_returns_error() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = r#"
        // <block name="foo" affects="file:block" invalid-attr=">
        fn foo() {}
        // </block>
        "#;
        assert!(parser.parse(contents).is_err());
        Ok(())
    }

    #[test]
    fn blocks_with_different_line_endings() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = "// <block>\r\nWindows\r\n// </block>\n// <block>\nUnix\n// </block>";
        let blocks = parser.parse(contents)?;
        assert_eq!(blocks.len(), 2);
        assert!(blocks[0].content(contents).contains("\r\n"));
        assert!(blocks[1].content(contents).contains("\n"));
        Ok(())
    }

    #[test]
    fn comments_with_xml_like_symbols() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = r#"
        /*
        Some logical expressions: a && b, a & b, a ^ b, a || b, a | b, a ^ !b
        Arithmetic expressions: a < b, a << b, d > f
        <block>
        */
        fn unicode() {}
        // </block>
        "#;
        let blocks = parser.parse(contents)?;
        assert_eq!(blocks.len(), 1);
        Ok(())
    }

    #[test]
    fn comments_with_quotes_and_parenthesis_symbols() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = r#"
/// "cxx" -> "c")
// "a" block
// "b" block
// "c" block
// <block name="foo-bar">
// </block>"#;
        let blocks = parser.parse(contents)?;
        assert_eq!(blocks.len(), 1);
        Ok(())
    }

    #[test]
    fn comments_with_unrelated_tags() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = r#"
        // <p>Paragraph</p><block><b>bold</b>
        fn unicode() {}
        // </block><body>hello</body>
        "#;
        let blocks = parser.parse(contents)?;
        assert_eq!(blocks.len(), 1);
        Ok(())
    }

    #[test]
    fn comments_with_unclosed_unrelated_tags() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = r#"
        // <p>this tag has no ending tag <block></b> this tag has no starting tag
        fn unicode() {}
        // </block>
        "#;
        let blocks = parser.parse(contents)?;
        assert_eq!(blocks.len(), 1);
        Ok(())
    }

    #[test]
    fn comments_with_invalid_tags() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = r#"
        // <invalid tag
        // <block>
        fn unicode() {}
        // </block>"#;
        let blocks = parser.parse(contents)?;
        assert_eq!(blocks.len(), 1);
        Ok(())
    }
}
