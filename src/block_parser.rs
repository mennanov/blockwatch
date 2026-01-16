use crate::Position;
use crate::blocks::Block;
use crate::language_parsers::{Comment, CommentsParser};
use crate::tag_parser::{BlockTag, BlockTagParser, WinnowBlockTagParser};
use std::collections::HashMap;
use std::ops::{Range, RangeInclusive};

/// Parses [`Blocks`] from a source code.
pub trait BlocksParser {
    /// Returns [`Block`]s extracted from the given `contents` string.
    ///
    /// The blocks are required to be sorted by the `starts_at` field in ascending order.
    fn parse(&mut self, contents: &str) -> anyhow::Result<Vec<Block>>;
}

pub struct BlocksFromCommentsParser<C: CommentsParser> {
    comments_parser: C,
}

impl<C: CommentsParser> BlocksFromCommentsParser<C> {
    pub(crate) fn new(comments_parser: C) -> Self {
        Self { comments_parser }
    }
}

impl<C: CommentsParser> BlocksParser for BlocksFromCommentsParser<C> {
    fn parse(&mut self, contents: &str) -> anyhow::Result<Vec<Block>> {
        let comments = self.comments_parser.parse(contents)?;
        parse_blocks_from_comments(comments.iter())
    }
}

/// Parses blocks from comments iterator.
pub(crate) fn parse_blocks_from_comments<'c>(
    comments: impl Iterator<Item = &'c Comment>,
) -> anyhow::Result<Vec<Block>> {
    let mut blocks = Vec::new();
    let mut block_starts = Vec::new();
    for partial_block in PartialBlocksIterator::new(comments) {
        match partial_block? {
            PartialBlock::Start(block_start) => {
                block_starts.push(block_start);
            }
            PartialBlock::End(block_end) => {
                if let Some(block_start) = block_starts.pop() {
                    blocks.push(block_end.into_block(block_start));
                } else {
                    return Err(anyhow::anyhow!(
                        "Unexpected closed block at line {}, position {}",
                        block_end.comment.position_range.start.line,
                        block_end.comment.source_range.start + block_end.start_position
                    ));
                }
            }
        }
    }

    if let Some(unclosed_block) = block_starts.pop() {
        return Err(anyhow::anyhow!(format!(
            "Block at line {} is not closed",
            unclosed_block.comment.position_range.start.line
        )));
    }
    blocks.sort_by(|a, b| {
        a.start_tag_position_range
            .start()
            .cmp(b.start_tag_position_range.start())
    });

    Ok(blocks)
}

pub(crate) struct PartialBlocksIterator<'c, I: Iterator<Item = &'c Comment>> {
    comments: I,
    comment: Option<&'c Comment>,
    tags_parser: Option<WinnowBlockTagParser<'c>>,
}

impl<'c, I: Iterator<Item = &'c Comment>> PartialBlocksIterator<'c, I> {
    pub(crate) fn new(comments: I) -> Self {
        Self {
            comments,
            comment: None,
            tags_parser: None,
        }
    }
}

impl<'c, I: Iterator<Item = &'c Comment>> Iterator for PartialBlocksIterator<'c, I> {
    type Item = anyhow::Result<PartialBlock<'c, 'c>>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.tags_parser.is_none() {
                if self.comment.is_none() {
                    self.comment = Some(self.comments.next()?);
                }
                self.tags_parser = Some(WinnowBlockTagParser::new(
                    &self.comment.unwrap().comment_text,
                ));
            }
            return match self.tags_parser.as_mut().unwrap().next() {
                Ok(Some(tag)) => match tag {
                    BlockTag::Start {
                        tag_range,
                        attributes,
                    } => Some(Ok(PartialBlock::Start(BlockStart::new(
                        self.comment.unwrap(),
                        attributes,
                        tag_range,
                    )))),
                    BlockTag::End { start_position } => Some(Ok(PartialBlock::End(BlockEnd::new(
                        self.comment.unwrap(),
                        start_position,
                    )))),
                },
                Ok(None) => {
                    self.tags_parser = None;
                    self.comment = None;
                    continue;
                }
                Err(e) => Some(Err(e)),
            };
        }
    }
}

pub(crate) enum PartialBlock<'s, 'e> {
    Start(BlockStart<'s>),
    End(BlockEnd<'e>),
}

pub(crate) struct BlockStart<'c> {
    pub(crate) comment: &'c Comment,
    pub(crate) attributes: HashMap<String, String>,
    pub(crate) start_tag_position_range: RangeInclusive<Position>,
}

impl<'c> BlockStart<'c> {
    fn new(
        comment: &'c Comment,
        attributes: HashMap<String, String>,
        position_in_comment_range: Range<usize>,
    ) -> Self {
        Self {
            comment,
            attributes,
            start_tag_position_range: Self::source_position_at(
                position_in_comment_range.start,
                comment,
            )
                ..=Self::source_position_at(position_in_comment_range.end - 1, comment),
        }
    }
    fn source_position_at(position_in_comment: usize, comment: &Comment) -> Position {
        let line_number = comment.position_range.start.line
            + comment.comment_text[..position_in_comment + 1]
                .lines()
                .count()
            - 1;
        Position::new(
            line_number,
            if line_number == comment.position_range.start.line {
                // The given `position_in_comment` is in the same line as the comment's start.
                comment.position_range.start.character + position_in_comment
            } else {
                position_in_comment
                    - comment.comment_text[..position_in_comment]
                        .rfind('\n')
                        .unwrap_or(0)
            },
        )
    }
}

/// Represents the end of a block, capturing its content range and position range.
pub(crate) struct BlockEnd<'c> {
    pub(crate) comment: &'c Comment,
    pub(crate) start_position: usize,
}

impl<'c> BlockEnd<'c> {
    fn new(end_tag_comment: &'c Comment, start_position: usize) -> Self {
        Self {
            comment: end_tag_comment,
            start_position,
        }
    }

    pub(crate) fn into_block(self, block_start: BlockStart) -> Block {
        let content_range = if !std::ptr::eq(self.comment, block_start.comment) {
            block_start.comment.source_range.end..self.comment.source_range.start
        } else {
            // Block that starts and ends in the same comment can't have any
            // content.
            0..0
        };
        let content_start_position = block_start.comment.position_range.end.clone();
        let content_end_position = self.comment.position_range.start.clone();
        Block::new(
            block_start.attributes,
            block_start.start_tag_position_range,
            content_range,
            content_start_position..content_end_position,
        )
    }
}

#[cfg(test)]
mod tests {
    use crate::block_parser::BlocksParser;
    use crate::blocks::Block;
    use crate::{Position, language_parsers, test_utils};
    use std::collections::HashMap;

    fn create_parser() -> impl BlocksParser {
        // Reuse existing real blocks parser.
        language_parsers::rust::parser().unwrap()
    }

    #[test]
    fn no_defined_blocks_returns_empty_blocks() -> anyhow::Result<()> {
        let mut parser = create_parser();
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
        let mut parser = create_parser();
        let contents = r#"/* <block> */ let say = "hi"; /* </block> */"#;
        let blocks = parser.parse(contents)?;
        assert_eq!(
            blocks,
            vec![Block::new(
                HashMap::new(),
                Position::new(1, 4)..=Position::new(1, 10),
                test_utils::substr_range(contents, " let say = \"hi\"; "),
                Position::new(1, 14)..Position::new(1, 31),
            ),]
        );
        Ok(())
    }

    #[test]
    fn single_block_with_multiple_lines_content() -> anyhow::Result<()> {
        let mut parser = create_parser();
        let contents = "// <block>\nlet say = \"hi\";\n// </block>";
        let blocks = parser.parse(contents)?;
        assert_eq!(
            blocks,
            vec![Block::new(
                HashMap::new(),
                Position::new(1, 4)..=Position::new(1, 10),
                test_utils::substr_range(contents, "\nlet say = \"hi\";\n"),
                Position::new(1, 11)..Position::new(3, 1)
            ),]
        );
        Ok(())
    }

    #[test]
    fn single_block_with_multiline_starting_block_tag() -> anyhow::Result<()> {
        let mut parser = create_parser();
        let contents = "/* <block\n> */ let say = \"hi\"; // </block>";
        let blocks = parser.parse(contents)?;
        assert_eq!(
            blocks,
            vec![Block::new(
                HashMap::new(),
                Position::new(1, 4)..=Position::new(2, 1),
                test_utils::substr_range(contents, " let say = \"hi\"; "),
                Position::new(2, 5)..Position::new(2, 22),
            ),]
        );
        Ok(())
    }

    #[test]
    fn single_block_with_multiline_ending_block_tag() -> anyhow::Result<()> {
        let mut parser = create_parser();
        let contents = "/* <block> */ let say = \"hi\"; /* </block\n> */";
        let blocks = parser.parse(contents)?;
        assert_eq!(
            blocks,
            vec![Block::new(
                HashMap::new(),
                Position::new(1, 4)..=Position::new(1, 10),
                test_utils::substr_range(contents, " let say = \"hi\"; "),
                Position::new(1, 14)..Position::new(1, 31),
            ),]
        );
        Ok(())
    }

    #[test]
    fn multiple_blocks_on_separate_lines() -> anyhow::Result<()> {
        let mut parser = create_parser();
        let contents = r#"// <block>
println!("hello1");
// </block>
// <block>
println!("hello2");
// </block>"#;
        let blocks = parser.parse(contents)?;
        assert_eq!(
            blocks,
            vec![
                Block::new(
                    HashMap::new(),
                    Position::new(1, 4)..=Position::new(1, 10),
                    test_utils::substr_range(contents, "\nprintln!(\"hello1\");\n"),
                    Position::new(1, 11)..Position::new(3, 1),
                ),
                Block::new(
                    HashMap::new(),
                    Position::new(4, 4)..=Position::new(4, 10),
                    test_utils::substr_range(contents, "\nprintln!(\"hello2\");\n"),
                    Position::new(4, 11)..Position::new(6, 1),
                )
            ]
        );
        Ok(())
    }

    #[test]
    fn multiple_blocks_on_intersecting_lines() -> anyhow::Result<()> {
        let mut parser = create_parser();
        let contents = "// <block>\nprintln!(\"hello1\");\n/* </block><block> */\nprintln!(\"hello2\");\n// </block>";
        let blocks = parser.parse(contents)?;
        assert_eq!(
            blocks,
            vec![
                Block::new(
                    HashMap::new(),
                    Position::new(1, 4)..=Position::new(1, 10),
                    test_utils::substr_range(contents, "\nprintln!(\"hello1\");\n"),
                    Position::new(1, 11)..Position::new(3, 1),
                ),
                Block::new(
                    HashMap::new(),
                    Position::new(3, 12)..=Position::new(3, 18),
                    test_utils::substr_range(contents, "\nprintln!(\"hello2\");\n"),
                    Position::new(3, 22)..Position::new(5, 1),
                )
            ]
        );
        Ok(())
    }

    #[test]
    fn multiple_blocks_on_single_line() -> anyhow::Result<()> {
        let mut parser = create_parser();
        let contents = "/* <block> */println!(\"hello1\");/* </block><block> */println!(\"hello2\");// </block>";
        let blocks = parser.parse(contents)?;
        assert_eq!(
            blocks,
            vec![
                Block::new(
                    HashMap::new(),
                    Position::new(1, 4)..=Position::new(1, 10),
                    test_utils::substr_range(contents, "println!(\"hello1\");"),
                    Position::new(1, 14)..Position::new(1, 33),
                ),
                Block::new(
                    HashMap::new(),
                    Position::new(1, 44)..=Position::new(1, 50),
                    test_utils::substr_range(contents, "println!(\"hello2\");"),
                    Position::new(1, 54)..Position::new(1, 73),
                )
            ]
        );
        Ok(())
    }

    #[test]
    fn block_starts_on_non_first_comment_line() -> anyhow::Result<()> {
        let mut parser = create_parser();
        let contents = "/* Some comment\n<block> */println!(\"hello1\");// </block>";
        let blocks = parser.parse(contents)?;
        assert_eq!(
            blocks,
            vec![Block::new(
                HashMap::new(),
                Position::new(2, 1)..=Position::new(2, 7),
                test_utils::substr_range(contents, "println!(\"hello1\");"),
                Position::new(2, 11)..Position::new(2, 30),
            ),]
        );
        Ok(())
    }

    #[test]
    fn block_ends_on_non_first_comment_line() -> anyhow::Result<()> {
        let mut parser = create_parser();
        let contents = "/* <block> */println!(\"hello1\");/* Some comment\n</block> */";
        let blocks = parser.parse(contents)?;
        assert_eq!(
            blocks,
            vec![Block::new(
                HashMap::new(),
                Position::new(1, 4)..=Position::new(1, 10),
                test_utils::substr_range(contents, "println!(\"hello1\");"),
                Position::new(1, 14)..Position::new(1, 33),
            ),]
        );
        Ok(())
    }

    #[test]
    fn nested_blocks() -> anyhow::Result<()> {
        let mut parser = create_parser();
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
                    HashMap::from([("name".to_string(), "foo".to_string())]),
                    Position::new(2, 12)..=Position::new(2, 29),
                    30..620,
                    Position::new(2, 30)..Position::new(25, 9),
                ),
                Block::new(
                    HashMap::from([("name".to_string(), "bar".to_string())]),
                    Position::new(7, 16)..=Position::new(7, 33),
                    142..440,
                    Position::new(7, 34)..Position::new(17, 13),
                ),
                Block::new(
                    HashMap::from([("name".to_string(), "bar-bar".to_string())]),
                    Position::new(11, 20)..=Position::new(11, 41),
                    281..415,
                    Position::new(11, 42)..Position::new(15, 17),
                ),
                Block::new(
                    HashMap::from([("name".to_string(), "buzz".to_string())]),
                    Position::new(19, 16)..=Position::new(19, 34),
                    487..599,
                    Position::new(19, 35)..Position::new(23, 13),
                ),
                Block::new(
                    HashMap::from([("name".to_string(), "fizz".to_string())]),
                    Position::new(26, 12)..=Position::new(26, 30),
                    662..671,
                    Position::new(26, 31)..Position::new(27, 9),
                ),
            ]
        );
        Ok(())
    }

    #[test]
    fn multiple_nested_blocks_at_same_level() -> anyhow::Result<()> {
        let mut parser = create_parser();
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
        let mut parser = create_parser();
        let contents = r#"// <block name="foo">This text is ignored
        let word = "hello";
        // </block> Some comment."#;
        let blocks = parser.parse(contents)?;
        assert_eq!(
            blocks,
            vec![Block::new(
                HashMap::from([("name".to_string(), "foo".to_string())]),
                Position::new(1, 4)..=Position::new(1, 21),
                test_utils::substr_range(contents, "\n        let word = \"hello\";\n        "),
                Position::new(1, 42)..Position::new(3, 9),
            ),]
        );
        Ok(())
    }

    #[test]
    fn unclosed_block_returns_error() -> anyhow::Result<()> {
        let mut parser = create_parser();
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
        let mut parser = create_parser();
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
        let mut parser = create_parser();
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
        let mut parser = create_parser();
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
        let mut parser = create_parser();
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
        let mut parser = create_parser();
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
        let mut parser = create_parser();
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
        let mut parser = create_parser();
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
        let mut parser = create_parser();
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
        let mut parser = create_parser();
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
        let mut parser = create_parser();
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
        let mut parser = create_parser();
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
        let mut parser = create_parser();
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
        let mut parser = create_parser();
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
        let mut parser = create_parser();
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
        let mut parser = create_parser();
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
        let mut parser = create_parser();
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
        let mut parser = create_parser();
        let contents = "// <block>\r\nWindows\r\n// </block>\n// <block>\nUnix\n// </block>";
        let blocks = parser.parse(contents)?;
        assert_eq!(blocks.len(), 2);
        assert!(blocks[0].content(contents).contains("\r\n"));
        assert!(blocks[1].content(contents).contains("\n"));
        Ok(())
    }

    #[test]
    fn comments_with_xml_like_symbols() -> anyhow::Result<()> {
        let mut parser = create_parser();
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
        let mut parser = create_parser();
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
        let mut parser = create_parser();
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
        let mut parser = create_parser();
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
        let mut parser = create_parser();
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
