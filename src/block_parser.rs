use crate::Position;
use crate::blocks::Block;
use crate::language_parsers::{Comment, CommentsParser};
use crate::tag_parser::{BlockTag, BlockTagParser, WinnowBlockTagParser};
use std::collections::{HashMap, VecDeque};
use std::ops::{Range, RangeInclusive};
use std::rc::Rc;

/// Parses [`Blocks`] from a source code.
pub trait BlocksParser: Send + Sync {
    /// Returns an iterator over the [`Block`]s found in the given `contents` string.
    ///
    /// The blocks are required to be yielded sorted by the `starts_at` field in ascending order.
    ///
    /// The iteration stops at the first error: whatever follows a malformed or unbalanced tag
    /// cannot be trusted to belong to the block the source intended.
    ///
    /// Returned boxed rather than as an `impl Iterator` because the trait is used as
    /// `dyn BlocksParser`, which a return-position `impl Trait` would rule out.
    fn parse<'a>(
        &'a mut self,
        contents: &'a str,
    ) -> Box<dyn Iterator<Item = anyhow::Result<Block>> + 'a>;
}

/// The one [`BlocksParser`] every language uses: block syntax is identical everywhere, so only the
/// extraction of comment text is language-specific and that part is delegated to `C`.
pub struct BlocksFromCommentsParser<C: CommentsParser> {
    comments_parser: C,
}

impl<C: CommentsParser> BlocksFromCommentsParser<C> {
    /// Wraps a language's comment parser so it produces blocks. Called by each
    /// `language_parsers::<lang>::parser()`.
    pub(crate) fn new(comments_parser: C) -> Self {
        Self { comments_parser }
    }
}

impl<C: CommentsParser> BlocksParser for BlocksFromCommentsParser<C> {
    fn parse<'a>(
        &'a mut self,
        contents: &'a str,
    ) -> Box<dyn Iterator<Item = anyhow::Result<Block>> + 'a> {
        Box::new(BlocksIterator::new(self.comments_parser.parse(contents)))
    }
}

/// Assembles [`Block`]s out of a comment stream.
pub(crate) struct BlocksIterator<I: Iterator<Item = Comment>> {
    partial_blocks: PartialBlocksIterator<I>,
    /// Start tags still waiting for their end tag, outermost first.
    open_blocks: Vec<BlockStart>,
    /// The current group of blocks, in start order.
    blocks: VecDeque<Block>,
    /// Error to yield once `blocks` has drained; yielding it ends the iteration.
    error: Option<anyhow::Error>,
    /// Whether the comment stream is exhausted.
    done: bool,
}

impl<I: Iterator<Item = Comment>> BlocksIterator<I> {
    /// Starts the block stream over the given comments, which must be in source order.
    pub(crate) fn new(comments: I) -> Self {
        Self {
            partial_blocks: PartialBlocksIterator::new(comments),
            open_blocks: Vec::new(),
            blocks: VecDeque::new(),
            error: None,
            done: false,
        }
    }

    /// The next block of a group that has closed, or the error that ends the iteration.
    ///
    /// `None` means neither is available yet and the stream has to be read further.
    fn take_ready(&mut self) -> Option<anyhow::Result<Block>> {
        if self.open_blocks.is_empty()
            && let Some(block) = self.blocks.pop_front()
        {
            return Some(Ok(block));
        }
        let error = self.error.take()?;
        self.done = true;
        Some(Err(error))
    }

    /// Reads one start or end tag and folds it into the pending state.
    fn consume_tag(&mut self) {
        match self.partial_blocks.next() {
            Some(Ok(PartialBlock::Start(block_start))) => self.open_blocks.push(block_start),
            Some(Ok(PartialBlock::End(block_end))) => self.close_block(block_end),
            Some(Err(error)) => self.error = Some(error),
            None => self.finish(),
        }
    }

    /// Pairs an end tag with the innermost tag still open, completing one block.
    fn close_block(&mut self, block_end: BlockEnd) {
        let Some(block_start) = self.open_blocks.pop() else {
            self.error = Some(anyhow::anyhow!(
                "Unexpected closed block at line {}, position {}",
                block_end.comment.position_range.start.line,
                block_end.comment.source_range.start + block_end.start_position
            ));
            return;
        };
        self.blocks.push_back(block_end.into_block(block_start));
        if self.open_blocks.is_empty() {
            self.sort_completed_group();
        }
    }

    /// Winds up once the comments run out, reporting the innermost block left open.
    ///
    /// Blocks that closed inside an unclosed one stay behind the gate and are never yielded: an
    /// unbalanced file pairs tags unreliably from the missing tag onward, so those blocks may hold
    /// the wrong content.
    fn finish(&mut self) {
        self.done = true;
        if let Some(unclosed_block) = self.open_blocks.pop() {
            self.error = Some(anyhow::anyhow!(
                "Block at line {} is not closed",
                unclosed_block.comment.position_range.start.line
            ));
        }
    }

    /// Puts a group into start-tag order, which is how callers expect blocks to arrive.
    fn sort_completed_group(&mut self) {
        self.blocks.make_contiguous().sort();
    }
}

impl<I: Iterator<Item = Comment>> Iterator for BlocksIterator<I> {
    type Item = anyhow::Result<Block>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(item) = self.take_ready() {
                return Some(item);
            }
            if self.done {
                return None;
            }
            self.consume_tag();
        }
    }
}

/// Flattens comments into the stream of individual start and end tags they contain.
///
/// One comment may hold several tags (or a whole block, opened and closed in the same comment), so
/// the iterator keeps a cursor into the current comment's text and only advances to the next
/// comment once that text is exhausted.
pub(crate) struct PartialBlocksIterator<I: Iterator<Item = Comment>> {
    comments: I,
    comment: Option<Rc<Comment>>,
    tags_parser_cursor: usize,
}

impl<I: Iterator<Item = Comment>> PartialBlocksIterator<I> {
    /// Starts the tag stream over the given comments, which must be in source order.
    pub(crate) fn new(comments: I) -> Self {
        Self {
            comments,
            comment: None,
            tags_parser_cursor: 0,
        }
    }
}

impl<I: Iterator<Item = Comment>> Iterator for PartialBlocksIterator<I> {
    type Item = anyhow::Result<PartialBlock>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.comment.is_none() {
                {
                    let c = self.comments.next()?;
                    self.comment = Some(Rc::new(c));
                    self.tags_parser_cursor = 0;
                }
            }

            let comment_rc = self.comment.as_ref().unwrap();
            let mut tags_parser =
                WinnowBlockTagParser::new(&comment_rc.comment_text, self.tags_parser_cursor);
            let block_tag_result = tags_parser.next();
            self.tags_parser_cursor = tags_parser.cursor();
            return match block_tag_result {
                Ok(Some(tag)) => match tag {
                    BlockTag::Start {
                        tag_range,
                        attributes,
                    } => Some(Ok(PartialBlock::Start(BlockStart::new(
                        Rc::clone(self.comment.as_ref().unwrap()),
                        attributes,
                        tag_range,
                    )))),
                    BlockTag::End { start_position } => Some(Ok(PartialBlock::End(BlockEnd::new(
                        Rc::clone(self.comment.as_ref().unwrap()),
                        start_position,
                    )))),
                },
                Ok(None) => {
                    self.comment = None;
                    continue;
                }
                Err(e) => Some(Err(e)),
            };
        }
    }
}

/// One half of a block. A [`Block`] is only formed once a `Start` has been matched with an `End`,
/// which is what lets blocks nest.
pub(crate) enum PartialBlock {
    Start(BlockStart),
    End(BlockEnd),
}

/// An opening block tag: its attributes, and where it sits in the source.
pub(crate) struct BlockStart {
    /// The comment the tag was found in. Shared with [`BlockEnd`] via `Rc` so that a block opened
    /// and closed inside a single comment can be recognized by pointer identity.
    pub(crate) comment: Rc<Comment>,
    /// Attributes parsed from the tag; these decide which validators apply.
    pub(crate) attributes: HashMap<String, String>,
    /// Where the tag sits in the source file, translated from its offset within the comment.
    pub(crate) start_tag_position_range: RangeInclusive<Position>,
}

impl BlockStart {
    fn new(
        comment: Rc<Comment>,
        attributes: HashMap<String, String>,
        position_in_comment_range: Range<usize>,
    ) -> Self {
        let start_tag_position_range =
            Self::source_position_at(position_in_comment_range.start, &comment)
                ..=Self::source_position_at(position_in_comment_range.end - 1, &comment);
        Self {
            comment,
            attributes,
            start_tag_position_range,
        }
    }

    /// Maps a byte offset within a comment's text onto its position in the source file.
    ///
    /// Columns count characters, so the offset is converted by counting the characters it skips
    /// rather than the bytes: the two differ on any line holding a multi-byte character.
    fn source_position_at(position_in_comment: usize, comment: &Comment) -> Position {
        let text_before = &comment.comment_text[..position_in_comment];
        match text_before.rfind('\n') {
            // Past the comment's first line, so the column is measured from that line's start.
            Some(line_break) => Position::new(
                comment.position_range.start.line + text_before.matches('\n').count(),
                text_before[line_break + 1..].chars().count() + 1,
            ),
            // Still on the comment's first line, which starts at the comment's own column.
            None => Position::new(
                comment.position_range.start.line,
                comment.position_range.start.character + text_before.chars().count(),
            ),
        }
    }
}

/// Represents the end of a block, capturing its content range and position range.
pub(crate) struct BlockEnd {
    /// The comment holding the closing tag. The block's content ends where this comment begins.
    pub(crate) comment: Rc<Comment>,
    /// Byte offset of the tag within `comment`, used to report unmatched end tags.
    pub(crate) start_position: usize,
}

impl BlockEnd {
    fn new(end_tag_comment: Rc<Comment>, start_position: usize) -> Self {
        Self {
            comment: end_tag_comment,
            start_position,
        }
    }

    /// Joins this end tag with its matching start tag into a complete [`Block`].
    ///
    /// The content is everything between the two comments, so a block opened and closed within one
    /// comment has no content at all.
    pub(crate) fn into_block(self, block_start: BlockStart) -> Block {
        let content_range = if !Rc::ptr_eq(&self.comment, &block_start.comment) {
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

    /// Drains the parser into a `Vec`, failing on the first parse error.
    fn parse_all(parser: &mut impl BlocksParser, contents: &str) -> anyhow::Result<Vec<Block>> {
        parser.parse(contents).collect()
    }

    #[test]
    fn source_without_blocks_returns_no_blocks() -> anyhow::Result<()> {
        let mut parser = create_parser();
        let contents = r#""
            fn say_hello_world() {
              println!("hello world!");
            }
        "#;
        let blocks = parse_all(&mut parser, contents)?;
        assert_eq!(blocks, vec![]);
        Ok(())
    }

    #[test]
    fn block_with_single_line_content_returns_correct_block() -> anyhow::Result<()> {
        let mut parser = create_parser();
        let contents = r#"/* <block> */ let say = "hi"; /* </block> */"#;
        let blocks = parse_all(&mut parser, contents)?;
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
    fn block_with_multiline_content_returns_correct_block() -> anyhow::Result<()> {
        let mut parser = create_parser();
        let contents = "// <block>\nlet say = \"hi\";\n// </block>";
        let blocks = parse_all(&mut parser, contents)?;
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
    fn blocks_on_separate_lines_return_correct_blocks() -> anyhow::Result<()> {
        let mut parser = create_parser();
        let contents = r#"// <block>
println!("hello1");
// </block>
// <block>
println!("hello2");
// </block>"#;
        let blocks = parse_all(&mut parser, contents)?;
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
    fn blocks_on_a_single_line_return_correct_blocks() -> anyhow::Result<()> {
        let mut parser = create_parser();
        let contents = "/* <block> */println!(\"hello1\");/* </block><block> */println!(\"hello2\");// </block>";
        let blocks = parse_all(&mut parser, contents)?;
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
    fn adjacent_blocks_sharing_one_comment_return_correct_blocks() -> anyhow::Result<()> {
        let mut parser = create_parser();
        let contents = "// <block>\nprintln!(\"hello1\");\n/* </block><block> */\nprintln!(\"hello2\");\n// </block>";
        let blocks = parse_all(&mut parser, contents)?;
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
    fn nested_blocks_return_correct_blocks() -> anyhow::Result<()> {
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
        let blocks = parse_all(&mut parser, contents)?;
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
    fn nested_blocks_at_the_same_level_are_returned_in_start_order() -> anyhow::Result<()> {
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
        let blocks = parse_all(&mut parser, contents)?;
        assert_eq!(blocks.len(), 4);
        assert_eq!(blocks[0].attributes["name"], "parent");
        assert_eq!(blocks[1].attributes["name"], "child1");
        assert_eq!(blocks[2].attributes["name"], "child2");
        assert_eq!(blocks[3].attributes["name"], "child3");
        Ok(())
    }

    #[test]
    fn text_around_the_tags_in_a_comment_is_excluded_from_the_content() -> anyhow::Result<()> {
        let mut parser = create_parser();
        let contents = r#"// <block name="foo">This text is ignored
        let word = "hello";
        // </block> Some comment."#;
        let blocks = parse_all(&mut parser, contents)?;
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
    fn blocks_with_different_line_endings_return_correct_content() -> anyhow::Result<()> {
        let mut parser = create_parser();
        let contents = "// <block>\r\nWindows\r\n// </block>\n// <block>\nUnix\n// </block>";
        let blocks = parse_all(&mut parser, contents)?;
        assert_eq!(blocks.len(), 2);
        assert!(blocks[0].content(contents).contains("\r\n"));
        assert!(blocks[1].content(contents).contains("\n"));
        Ok(())
    }

    #[test]
    fn block_with_a_multiline_start_tag_returns_correct_block() -> anyhow::Result<()> {
        let mut parser = create_parser();
        let contents = "/* <block\n> */ let say = \"hi\"; // </block>";
        let blocks = parse_all(&mut parser, contents)?;
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
    fn block_with_a_multiline_end_tag_returns_correct_block() -> anyhow::Result<()> {
        let mut parser = create_parser();
        let contents = "/* <block> */ let say = \"hi\"; /* </block\n> */";
        let blocks = parse_all(&mut parser, contents)?;
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
    fn block_starting_on_a_non_first_comment_line_returns_correct_block() -> anyhow::Result<()> {
        let mut parser = create_parser();
        let contents = "/* Some comment\n<block> */println!(\"hello1\");// </block>";
        let blocks = parse_all(&mut parser, contents)?;
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
    fn block_ending_on_a_non_first_comment_line_returns_correct_block() -> anyhow::Result<()> {
        let mut parser = create_parser();
        let contents = "/* <block> */println!(\"hello1\");/* Some comment\n</block> */";
        let blocks = parse_all(&mut parser, contents)?;
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
    fn non_ascii_before_a_start_tag_returns_a_character_based_column() -> anyhow::Result<()> {
        let mut parser = create_parser();
        // `é` occupies two bytes but one column, so the tag sits at the same column as it would
        // with a plain `e`.
        let contents = "// café <block>\nlet say = \"hi\";\n// </block>";
        let blocks = parse_all(&mut parser, contents)?;
        assert_eq!(
            blocks,
            vec![Block::new(
                HashMap::new(),
                Position::new(1, 9)..=Position::new(1, 15),
                test_utils::substr_range(contents, "\nlet say = \"hi\";\n"),
                Position::new(1, 16)..Position::new(3, 1)
            ),]
        );
        Ok(())
    }

    #[test]
    fn non_ascii_before_a_start_tag_on_a_later_comment_line_returns_a_character_based_column()
    -> anyhow::Result<()> {
        let mut parser = create_parser();
        // The tag sits on the second line of a block comment, past a multi-byte character on that
        // same line.
        let contents = "/*\n   café <block> */\nlet say = \"hi\";\n// </block>";
        let blocks = parse_all(&mut parser, contents)?;
        assert_eq!(
            blocks,
            vec![Block::new(
                HashMap::new(),
                Position::new(2, 9)..=Position::new(2, 15),
                test_utils::substr_range(contents, "\nlet say = \"hi\";\n"),
                Position::new(2, 19)..Position::new(4, 1)
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
        let error_message = parse_all(&mut parser, contents).unwrap_err().to_string();
        assert_eq!(error_message, "Block at line 2 is not closed");
        Ok(())
    }

    #[test]
    fn unclosed_block_with_a_closed_nested_one_returns_error() -> anyhow::Result<()> {
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
        let error_message = parse_all(&mut parser, contents).unwrap_err().to_string();
        assert_eq!(error_message, "Block at line 2 is not closed");
        Ok(())
    }

    #[test]
    fn unmatched_end_tag_returns_error() -> anyhow::Result<()> {
        let mut parser = create_parser();
        let contents = r#"
        fn say_hello_world() {
          println!("hello world!");
        }
        // </block>
        "#;
        let result = parse_all(&mut parser, contents);
        assert!(result.is_err());
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
        assert!(parse_all(&mut parser, contents).is_err());
        Ok(())
    }

    #[test]
    fn block_closed_before_an_unclosed_one_is_returned_ahead_of_the_error() {
        let mut parser = create_parser();
        let contents = "// <block name=\"closed\">\n// </block>\n// <block name=\"open\">";

        let mut blocks = parser.parse(contents);

        assert_eq!(
            blocks
                .next()
                .expect("a block")
                .expect("no parse error")
                .attributes["name"],
            "closed"
        );
        assert_eq!(
            blocks
                .next()
                .expect("an error")
                .expect_err("the unclosed block")
                .to_string(),
            "Block at line 3 is not closed"
        );
        assert!(blocks.next().is_none());
    }

    #[test]
    fn block_closed_inside_unclosed_ones_is_not_returned() {
        let mut parser = create_parser();
        // `inner` is complete, but both of the blocks holding it are left open. Its tags could
        // just as well have been mispaired by the missing ones, so it is not handed over.
        let contents = "// <block name=\"outer\">\n// <block name=\"middle\">\n// <block name=\"inner\">\n// </block>";

        let mut blocks = parser.parse(contents);

        assert_eq!(
            blocks
                .next()
                .expect("an error")
                .expect_err("the unclosed block")
                .to_string(),
            "Block at line 2 is not closed"
        );
        assert!(blocks.next().is_none());
    }

    #[test]
    fn block_following_the_first_error_is_not_returned() {
        let mut parser = create_parser();
        // The stray end tag has nothing to close, so the block following it is never reached.
        let contents = "// </block>\n// <block>\n// </block>";

        let mut blocks = parser.parse(contents);

        assert!(blocks.next().expect("an error").is_err());
        assert!(blocks.next().is_none());
    }

    #[test]
    fn tag_with_attributes_on_a_single_line_returns_correct_attributes() -> anyhow::Result<()> {
        let mut parser = create_parser();
        let contents = r#"
        // <block foo="bar" fizz="buzz">
        fn foo() {
          println!("hello world!");
        }
        // </block>
        "#;
        let blocks = parse_all(&mut parser, contents)?;
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
    fn tag_with_attributes_on_multiple_lines_returns_correct_attributes() -> anyhow::Result<()> {
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
        let blocks = parse_all(&mut parser, contents)?;
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
    fn unquoted_attribute_values_return_correct_values() -> anyhow::Result<()> {
        let mut parser = create_parser();
        let contents = r#"
        // <block color=red flavor=sweet>
        // </block>
        "#;
        let blocks = parse_all(&mut parser, contents)?;
        assert_eq!(blocks[0].attributes["color"], "red");
        assert_eq!(blocks[0].attributes["flavor"], "sweet");
        Ok(())
    }

    #[test]
    fn single_quoted_attribute_value_returns_the_value_verbatim() -> anyhow::Result<()> {
        let mut parser = create_parser();
        let contents = r#"
        // <block text='He said "Hello"'>
        // </block>
        "#;
        let blocks = parse_all(&mut parser, contents)?;
        assert_eq!(blocks[0].attributes["text"], "He said \"Hello\"");
        Ok(())
    }

    #[test]
    fn valueless_attributes_return_empty_strings() -> anyhow::Result<()> {
        let mut parser = create_parser();
        let contents = r#"
        // <block attr1 attr2>
        // </block>
        "#;
        let blocks = parse_all(&mut parser, contents)?;
        assert_eq!(blocks[0].attributes["attr1"], "");
        assert_eq!(blocks[0].attributes["attr2"], "");
        Ok(())
    }

    #[test]
    fn attributes_with_an_empty_quoted_value_return_empty_strings() -> anyhow::Result<()> {
        let mut parser = create_parser();
        let contents = r#"
        // <block name="" foo="" bar=''>
        fn foo() {}
        // </block>
        "#;
        let blocks = parse_all(&mut parser, contents)?;
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
    fn attributes_with_spaces_around_the_equals_sign_return_correct_values() -> anyhow::Result<()> {
        let mut parser = create_parser();
        let contents = r#"
        // <block name = "foo" desc = 'bar'>
        fn unicode() {}
        // </block>
        "#;
        let blocks = parse_all(&mut parser, contents)?;
        assert_eq!(blocks[0].attributes["name"], "foo");
        assert_eq!(blocks[0].attributes["desc"], "bar");
        Ok(())
    }

    #[test]
    fn tag_mixing_valued_and_valueless_attributes_returns_correct_attributes() -> anyhow::Result<()>
    {
        let mut parser = create_parser();
        let contents = r#"
        // <block color="red" attr1 align="center" attr2>
        fn escaped() {}
        // </block>
        "#;
        let blocks = parse_all(&mut parser, contents)?;
        assert_eq!(blocks[0].attributes["color"], "red");
        assert_eq!(blocks[0].attributes["attr1"], "");
        assert_eq!(blocks[0].attributes["align"], "center");
        assert_eq!(blocks[0].attributes["attr2"], "");
        Ok(())
    }

    #[test]
    fn duplicated_attribute_returns_the_last_value() -> anyhow::Result<()> {
        let mut parser = create_parser();
        let contents = r#"
        // <block color="red" color="blue">
        fn escaped() {}
        // </block>
        "#;
        let blocks = parse_all(&mut parser, contents)?;

        // Duplicate attributes: last value wins (standard HTML/XML behavior)
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].attributes.get("color"), Some(&"blue".to_string()));
        Ok(())
    }

    #[test]
    fn html_escaped_quotes_in_an_attribute_value_are_not_decoded() -> anyhow::Result<()> {
        let mut parser = create_parser();
        let contents = r#"
        // <block text="He said &quot;Hello&quot;">
        // </block>
        "#;
        let blocks = parse_all(&mut parser, contents)?;

        assert_eq!(blocks[0].attributes["text"], "He said &quot;Hello&quot;");
        Ok(())
    }

    #[test]
    fn attribute_value_holding_angle_brackets_returns_the_value_verbatim() -> anyhow::Result<()> {
        let mut parser = create_parser();
        let contents = r#"
        // <block keep-unique="(?P<value>\w+)">
        // </block>
        "#;
        let blocks = parse_all(&mut parser, contents)?;
        assert_eq!(blocks[0].attributes["keep-unique"], r"(?P<value>\w+)");
        Ok(())
    }

    #[test]
    fn non_ascii_attribute_value_returns_the_value_verbatim() -> anyhow::Result<()> {
        let mut parser = create_parser();
        let contents = r#"
        // <block name="🦀" desc="Rust">
        fn unicode() {}
        // </block>
        "#;
        let blocks = parse_all(&mut parser, contents)?;
        assert_eq!(blocks[0].attributes["name"], "🦀");
        assert_eq!(blocks[0].attributes["desc"], "Rust");
        Ok(())
    }

    #[test]
    fn nested_blocks_return_their_own_attributes() -> anyhow::Result<()> {
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
        let blocks = parse_all(&mut parser, contents)?;
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
    fn comment_with_xml_like_symbols_returns_the_block() -> anyhow::Result<()> {
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
        let blocks = parse_all(&mut parser, contents)?;
        assert_eq!(blocks.len(), 1);
        Ok(())
    }

    #[test]
    fn comment_with_quotes_and_parentheses_returns_the_block() -> anyhow::Result<()> {
        let mut parser = create_parser();
        let contents = r#"
/// "cxx" -> "c")
// "a" block
// "b" block
// "c" block
// <block name="foo-bar">
// </block>"#;
        let blocks = parse_all(&mut parser, contents)?;
        assert_eq!(blocks.len(), 1);
        Ok(())
    }

    #[test]
    fn comment_with_unrelated_tags_returns_the_block() -> anyhow::Result<()> {
        let mut parser = create_parser();
        let contents = r#"
        // <p>Paragraph</p><block><b>bold</b>
        fn unicode() {}
        // </block><body>hello</body>
        "#;
        let blocks = parse_all(&mut parser, contents)?;
        assert_eq!(blocks.len(), 1);
        Ok(())
    }

    #[test]
    fn comment_with_unclosed_unrelated_tags_returns_the_block() -> anyhow::Result<()> {
        let mut parser = create_parser();
        let contents = r#"
        // <p>this tag has no ending tag <block></b> this tag has no starting tag
        fn unicode() {}
        // </block>
        "#;
        let blocks = parse_all(&mut parser, contents)?;
        assert_eq!(blocks.len(), 1);
        Ok(())
    }

    #[test]
    fn comment_with_an_invalid_tag_before_a_block_returns_the_block() -> anyhow::Result<()> {
        let mut parser = create_parser();
        let contents = r#"
        // <invalid tag
        // <block>
        fn unicode() {}
        // </block>"#;
        let blocks = parse_all(&mut parser, contents)?;
        assert_eq!(blocks.len(), 1);
        Ok(())
    }
}
