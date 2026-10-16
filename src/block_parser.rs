use crate::Position;
use crate::blocks::{Block, Content, Declaration};
use crate::language_parsers::{Comment, CommentsParser};
use crate::tag_parser::{BlockTag, BlockTagParseError, BlockTagParser, WinnowBlockTagParser};
use std::collections::{HashMap, VecDeque};
use std::iter::Peekable;
use std::ops::Range;
use std::rc::Rc;

/// Parses the blocks out of one language's comments. Block syntax is identical everywhere, so only
/// the extraction of comment text is language-specific, and that part is delegated to `C`.
pub(crate) struct BlocksFromCommentsParser<C: CommentsParser> {
    comments_parser: C,
}

impl<C: CommentsParser> BlocksFromCommentsParser<C> {
    /// Wraps a language's comment parser so it produces blocks.
    pub(crate) fn new(comments_parser: C) -> Self {
        Self { comments_parser }
    }

    /// Returns an iterator over the [`Block`]s found in `contents`, sorted by where they start.
    ///
    /// The iteration stops at the first error: whatever follows a malformed or unbalanced tag
    /// cannot be trusted to belong to the block the source intended.
    pub(crate) fn parse_blocks<'a>(
        &'a mut self,
        contents: &'a str,
    ) -> impl Iterator<Item = anyhow::Result<Block>> + 'a {
        Blocks::new(self.comments_parser.parse(contents), contents)
    }
}

/// Assembles [`Block`]s out of a comment stream.
pub(crate) struct Blocks<'source, I: Iterator<Item = Comment>> {
    partial_blocks: Tags<'source, I>,
    /// Start tags still waiting for their end tag, outermost first.
    open_blocks: Vec<BlockStart>,
    /// The current group of blocks, in start order.
    blocks: VecDeque<Block>,
    /// Error to yield once `blocks` has drained; yielding it ends the iteration.
    error: Option<anyhow::Error>,
    /// Whether the comment stream is exhausted.
    done: bool,
}

impl<'source, I: Iterator<Item = Comment>> Blocks<'source, I> {
    /// Starts the block stream over the comments of `contents`, which must be in source order.
    pub(crate) fn new(comments: I, contents: &'source str) -> Self {
        Self {
            partial_blocks: Tags::new(comments, contents),
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
            let position =
                BlockStart::source_position_at(block_end.start_position, &block_end.comment);
            self.error = Some(anyhow::anyhow!(
                "Unexpected closed block at line {}, column {}. {NOT_A_TAG_HINT}",
                position.line,
                position.character
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
            let position = &unclosed_block.start_tag_position_range.start;
            self.error = Some(anyhow::anyhow!(
                "Block at line {}, column {} is not closed. {NOT_A_TAG_HINT}",
                position.line,
                position.character
            ));
        }
    }

    /// Puts a group into start-tag order, which is how callers expect blocks to arrive.
    fn sort_completed_group(&mut self) {
        self.blocks.make_contiguous().sort();
    }
}

impl<I: Iterator<Item = Comment>> Iterator for Blocks<'_, I> {
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
/// the iterator parses all the tags of a comment at once and returns them one by one.
struct Tags<'source, I: Iterator<Item = Comment>> {
    contents: &'source str,
    comments: Peekable<I>,
    /// The tags of the last comment read that are not returned yet, in source order. A tag that
    /// does not parse is an error, and it comes last.
    pending: VecDeque<anyhow::Result<PartialBlock>>,
}

impl<'source, I: Iterator<Item = Comment>> Tags<'source, I> {
    /// Starts the tag stream over the comments of `contents`, which must be in source order.
    fn new(comments: I, contents: &'source str) -> Self {
        Self {
            contents,
            comments: comments.peekable(),
            pending: VecDeque::new(),
        }
    }

    /// Parses the tags of `comment`.
    ///
    /// A tag that does not parse continues in the comments right after `comment`, with only
    /// whitespace between. Those comments are taken from the stream and joined to `comment`, until
    /// the tag parses or no such comment is left.
    ///
    /// Returns the comment with the comments joined to it, and its tags in source order. A tag
    /// that still does not parse is an error, and it comes last.
    fn parse_tags(
        &mut self,
        mut comment: Comment,
    ) -> (Comment, Vec<Result<BlockTag, BlockTagParseError>>) {
        let mut tags = Vec::new();
        let mut tags_parser_cursor = 0;
        loop {
            let mut tags_parser =
                WinnowBlockTagParser::new(&comment.comment_text, tags_parser_cursor);
            match tags_parser.next() {
                Ok(Some(tag)) => {
                    tags.push(Ok(tag));
                    tags_parser_cursor = tags_parser.cursor();
                }
                Ok(None) => return (comment, tags),
                // Most grammars make each line comment a node of its own, and a formatter such as
                // rustfmt wraps a long tag in a line comment over several of them.
                Err(BlockTagParseError::Malformed(malformed)) => {
                    match self.take_adjacent_comment(&comment) {
                        Some(next) => {
                            comment = join_comments(comment, next, self.contents);
                            // Joining only adds text after the open tag, so the tags before it
                            // stay as they were parsed.
                            tags_parser_cursor = malformed.position;
                        }
                        None => {
                            tags.push(Err(BlockTagParseError::Malformed(malformed)));
                            return (comment, tags);
                        }
                    }
                }
            }
        }
    }

    /// Takes the next comment from the stream if only whitespace separates it from `comment`.
    fn take_adjacent_comment(&mut self, comment: &Comment) -> Option<Comment> {
        let contents = self.contents;
        self.comments.next_if(|next| {
            #[expect(
                clippy::string_slice,
                reason = "parsers give comment ranges of whole characters"
            )]
            contents[comment.source_range.end..next.source_range.start]
                .trim()
                .is_empty()
        })
    }
}

/// Joins `comment` and `next`, a later comment of `contents`, into one comment that spans both.
fn join_comments(comment: Comment, next: Comment, contents: &str) -> Comment {
    // The text between the comments is copied from the source, so an offset into the joined text
    // maps onto the source as it does for a single comment.
    #[expect(
        clippy::string_slice,
        reason = "parsers give comment ranges of whole characters"
    )]
    let between = &contents[comment.source_range.end..next.source_range.start];
    Comment {
        position_range: comment.position_range.start..next.position_range.end,
        source_range: comment.source_range.start..next.source_range.end,
        comment_text: comment.comment_text + between + &next.comment_text,
    }
}

/// Turns a tag parsed from `comment` into a partial block. A tag that does not parse becomes an
/// error that shows its line and column.
fn to_partial_block(
    tag: Result<BlockTag, BlockTagParseError>,
    comment: &Rc<Comment>,
) -> anyhow::Result<PartialBlock> {
    match tag {
        Ok(BlockTag::Start {
            tag_range,
            attributes,
        }) => Ok(PartialBlock::Start(BlockStart::new(
            Rc::clone(comment),
            attributes,
            tag_range,
        ))),
        Ok(BlockTag::End { start_position }) => Ok(PartialBlock::End(BlockEnd::new(
            Rc::clone(comment),
            start_position,
        ))),
        Err(BlockTagParseError::Malformed(malformed)) => {
            let position = BlockStart::source_position_at(malformed.position, comment);
            Err(anyhow::anyhow!(
                "Malformed block tag at line {}, column {}. {NOT_A_TAG_HINT}",
                position.line,
                position.character
            ))
        }
    }
}

/// The end of the message of every error about a tag. A comment that only mentions a tag in prose
/// fails the run with these errors too, and backticks are the fix for it.
const NOT_A_TAG_HINT: &str = "If this is not meant as a tag, put it in backticks.";

impl<I: Iterator<Item = Comment>> Iterator for Tags<'_, I> {
    type Item = anyhow::Result<PartialBlock>;

    fn next(&mut self) -> Option<Self::Item> {
        while self.pending.is_empty() {
            let comment = self.comments.next()?;
            let (comment, tags) = self.parse_tags(comment);
            // The tags become partial blocks only once the comment stops growing, so all of them
            // share one `Comment`, as in a `/* */` one.
            let comment = Rc::new(comment);
            self.pending
                .extend(tags.into_iter().map(|tag| to_partial_block(tag, &comment)));
        }
        self.pending.pop_front()
    }
}

/// One half of a block. A [`Block`] is only formed once a `Start` has been matched with an `End`,
/// which is what lets blocks nest.
enum PartialBlock {
    Start(BlockStart),
    End(BlockEnd),
}

/// An opening block tag: its attributes, and where it sits in the source.
struct BlockStart {
    /// The comment the tag was found in. Shared with [`BlockEnd`] via `Rc` so that a block opened
    /// and closed inside a single comment can be recognized by pointer identity.
    comment: Rc<Comment>,
    /// Attributes parsed from the tag; these decide which validators apply.
    attributes: HashMap<String, String>,
    /// Where the tag sits in the source file, translated from its offset within the comment.
    start_tag_position_range: Range<Position>,
}

impl BlockStart {
    fn new(
        comment: Rc<Comment>,
        attributes: HashMap<String, String>,
        position_in_comment_range: Range<usize>,
    ) -> Self {
        let start_tag_position_range =
            Self::source_position_at(position_in_comment_range.start, &comment)
                ..Self::source_position_at(position_in_comment_range.end, &comment);
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
        #[expect(
            clippy::string_slice,
            reason = "a tag starts at a `<` and ends after a `>`"
        )]
        let text_before = &comment.comment_text[..position_in_comment];
        match text_before.rfind('\n') {
            // Past the comment's first line, so the column is measured from that line's start.
            #[expect(clippy::string_slice, reason = "`line_break` is the index of a `\\n`")]
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
struct BlockEnd {
    /// The comment holding the closing tag. The block's content ends where this comment begins.
    comment: Rc<Comment>,
    /// Byte offset of the tag within `comment`, used to report unmatched end tags.
    start_position: usize,
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
    fn into_block(self, block_start: BlockStart) -> Block {
        let content_range = if !Rc::ptr_eq(&self.comment, &block_start.comment) {
            block_start.comment.source_range.end..self.comment.source_range.start
        } else {
            // Block that starts and ends in the same comment can't have any
            // content.
            0..0
        };
        let content_start_position = block_start.comment.position_range.end;
        let content_end_position = self.comment.position_range.start;
        Block {
            attributes: block_start.attributes,
            start_tag_position_range: block_start.start_tag_position_range,
            content: Content::source(content_range, content_start_position..content_end_position),
            declaration: Declaration::Tags,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::blocks::{Block, Content, Declaration};
    use crate::language_parsers::LanguageParser;
    use crate::{Position, language_parsers, test_utils};
    use std::collections::HashMap;

    fn create_parser() -> impl LanguageParser {
        // Reuse existing real blocks parser.
        language_parsers::rust::parser().unwrap()
    }

    /// Drains the parser into a `Vec`, failing on the first parse error.
    fn parse_all(parser: &mut impl LanguageParser, contents: &str) -> anyhow::Result<Vec<Block>> {
        parser.parse_blocks(contents).collect()
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
            vec![Block {
                attributes: HashMap::new(),
                start_tag_position_range: Position::new(1, 4)..Position::new(1, 11),
                content: Content::source(
                    test_utils::substr_range(contents, " let say = \"hi\"; "),
                    Position::new(1, 14)..Position::new(1, 31),
                ),
                declaration: Declaration::Tags,
            },]
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
            vec![Block {
                attributes: HashMap::new(),
                start_tag_position_range: Position::new(1, 4)..Position::new(1, 11),
                content: Content::source(
                    test_utils::substr_range(contents, "\nlet say = \"hi\";\n"),
                    Position::new(1, 11)..Position::new(3, 1),
                ),
                declaration: Declaration::Tags,
            },]
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
                Block {
                    attributes: HashMap::new(),
                    start_tag_position_range: Position::new(1, 4)..Position::new(1, 11),
                    content: Content::source(
                        test_utils::substr_range(contents, "\nprintln!(\"hello1\");\n"),
                        Position::new(1, 11)..Position::new(3, 1),
                    ),
                    declaration: Declaration::Tags,
                },
                Block {
                    attributes: HashMap::new(),
                    start_tag_position_range: Position::new(4, 4)..Position::new(4, 11),
                    content: Content::source(
                        test_utils::substr_range(contents, "\nprintln!(\"hello2\");\n"),
                        Position::new(4, 11)..Position::new(6, 1),
                    ),
                    declaration: Declaration::Tags,
                }
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
                Block {
                    attributes: HashMap::new(),
                    start_tag_position_range: Position::new(1, 4)..Position::new(1, 11),
                    content: Content::source(
                        test_utils::substr_range(contents, "println!(\"hello1\");"),
                        Position::new(1, 14)..Position::new(1, 33),
                    ),
                    declaration: Declaration::Tags,
                },
                Block {
                    attributes: HashMap::new(),
                    start_tag_position_range: Position::new(1, 44)..Position::new(1, 51),
                    content: Content::source(
                        test_utils::substr_range(contents, "println!(\"hello2\");"),
                        Position::new(1, 54)..Position::new(1, 73),
                    ),
                    declaration: Declaration::Tags,
                }
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
                Block {
                    attributes: HashMap::new(),
                    start_tag_position_range: Position::new(1, 4)..Position::new(1, 11),
                    content: Content::source(
                        test_utils::substr_range(contents, "\nprintln!(\"hello1\");\n"),
                        Position::new(1, 11)..Position::new(3, 1),
                    ),
                    declaration: Declaration::Tags,
                },
                Block {
                    attributes: HashMap::new(),
                    start_tag_position_range: Position::new(3, 12)..Position::new(3, 19),
                    content: Content::source(
                        test_utils::substr_range(contents, "\nprintln!(\"hello2\");\n"),
                        Position::new(3, 22)..Position::new(5, 1),
                    ),
                    declaration: Declaration::Tags,
                }
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
                Block {
                    attributes: HashMap::from([("name".to_string(), "foo".to_string())]),
                    start_tag_position_range: Position::new(2, 12)..Position::new(2, 30),
                    content: Content::source(30..620, Position::new(2, 30)..Position::new(25, 9),),
                    declaration: Declaration::Tags,
                },
                Block {
                    attributes: HashMap::from([("name".to_string(), "bar".to_string())]),
                    start_tag_position_range: Position::new(7, 16)..Position::new(7, 34),
                    content: Content::source(142..440, Position::new(7, 34)..Position::new(17, 13),),
                    declaration: Declaration::Tags,
                },
                Block {
                    attributes: HashMap::from([("name".to_string(), "bar-bar".to_string())]),
                    start_tag_position_range: Position::new(11, 20)..Position::new(11, 42),
                    content: Content::source(
                        281..415,
                        Position::new(11, 42)..Position::new(15, 17),
                    ),
                    declaration: Declaration::Tags,
                },
                Block {
                    attributes: HashMap::from([("name".to_string(), "buzz".to_string())]),
                    start_tag_position_range: Position::new(19, 16)..Position::new(19, 35),
                    content: Content::source(
                        487..599,
                        Position::new(19, 35)..Position::new(23, 13),
                    ),
                    declaration: Declaration::Tags,
                },
                Block {
                    attributes: HashMap::from([("name".to_string(), "fizz".to_string())]),
                    start_tag_position_range: Position::new(26, 12)..Position::new(26, 31),
                    content: Content::source(662..671, Position::new(26, 31)..Position::new(27, 9),),
                    declaration: Declaration::Tags,
                },
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
            vec![Block {
                attributes: HashMap::from([("name".to_string(), "foo".to_string())]),
                start_tag_position_range: Position::new(1, 4)..Position::new(1, 22),
                content: Content::source(
                    test_utils::substr_range(contents, "\n        let word = \"hello\";\n        "),
                    Position::new(1, 42)..Position::new(3, 9),
                ),
                declaration: Declaration::Tags,
            },]
        );
        Ok(())
    }

    #[test]
    fn blocks_with_different_line_endings_return_correct_content() -> anyhow::Result<()> {
        let mut parser = create_parser();
        let contents = "// <block>\r\nWindows\r\n// </block>\n// <block>\nUnix\n// </block>";
        let blocks = parse_all(&mut parser, contents)?;
        assert_eq!(blocks.len(), 2);
        assert!(blocks[0].content.text(contents).contains("\r\n"));
        assert!(blocks[1].content.text(contents).contains("\n"));
        Ok(())
    }

    #[test]
    fn block_with_a_multiline_start_tag_returns_correct_block() -> anyhow::Result<()> {
        let mut parser = create_parser();
        let contents = "/* <block\n> */ let say = \"hi\"; // </block>";
        let blocks = parse_all(&mut parser, contents)?;
        assert_eq!(
            blocks,
            vec![Block {
                attributes: HashMap::new(),
                start_tag_position_range: Position::new(1, 4)..Position::new(2, 2),
                content: Content::source(
                    test_utils::substr_range(contents, " let say = \"hi\"; "),
                    Position::new(2, 5)..Position::new(2, 22),
                ),
                declaration: Declaration::Tags,
            },]
        );
        Ok(())
    }

    #[test]
    fn start_tag_continued_over_line_comments_returns_correct_block() -> anyhow::Result<()> {
        let mut parser = create_parser();
        let contents =
            "// <block name=\"foo\"\n// keep-sorted\n// keep-unique>\nlet a = 1;\n// </block>";
        let blocks = parse_all(&mut parser, contents)?;
        assert_eq!(
            blocks,
            vec![Block {
                attributes: HashMap::from([
                    ("name".to_string(), "foo".to_string()),
                    ("keep-sorted".to_string(), String::new()),
                    ("keep-unique".to_string(), String::new()),
                ]),
                start_tag_position_range: Position::new(1, 4)..Position::new(3, 16),
                content: Content::source(
                    test_utils::substr_range(contents, "\nlet a = 1;\n"),
                    Position::new(3, 16)..Position::new(5, 1),
                ),
                declaration: Declaration::Tags,
            },]
        );
        Ok(())
    }

    #[test]
    fn start_tag_continued_after_an_end_tag_on_the_same_line_returns_correct_blocks()
    -> anyhow::Result<()> {
        let mut parser = create_parser();
        let contents = "// <block name=\"a\">\nlet x = 1;\n// </block> <block name=\"b\"\n// keep-sorted>\nlet y = 2;\n// </block>";
        let blocks = parse_all(&mut parser, contents)?;
        assert_eq!(
            blocks,
            vec![
                Block {
                    attributes: HashMap::from([("name".to_string(), "a".to_string())]),
                    start_tag_position_range: Position::new(1, 4)..Position::new(1, 20),
                    content: Content::source(
                        test_utils::substr_range(contents, "\nlet x = 1;\n"),
                        Position::new(1, 20)..Position::new(3, 1),
                    ),
                    declaration: Declaration::Tags,
                },
                Block {
                    attributes: HashMap::from([
                        ("name".to_string(), "b".to_string()),
                        ("keep-sorted".to_string(), String::new()),
                    ]),
                    start_tag_position_range: Position::new(3, 13)..Position::new(4, 16),
                    content: Content::source(
                        test_utils::substr_range(contents, "\nlet y = 2;\n"),
                        Position::new(4, 16)..Position::new(6, 1),
                    ),
                    declaration: Declaration::Tags,
                },
            ]
        );
        Ok(())
    }

    #[test]
    fn block_with_only_line_comments_as_content_returns_correct_block() -> anyhow::Result<()> {
        let mut parser = create_parser();
        let contents = "// <block>\n// b\n// a\n// </block>";
        let blocks = parse_all(&mut parser, contents)?;
        assert_eq!(
            blocks,
            vec![Block {
                attributes: HashMap::new(),
                start_tag_position_range: Position::new(1, 4)..Position::new(1, 11),
                content: Content::source(
                    test_utils::substr_range(contents, "\n// b\n// a\n"),
                    Position::new(1, 11)..Position::new(4, 1),
                ),
                declaration: Declaration::Tags,
            },]
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
            vec![Block {
                attributes: HashMap::new(),
                start_tag_position_range: Position::new(1, 4)..Position::new(1, 11),
                content: Content::source(
                    test_utils::substr_range(contents, " let say = \"hi\"; "),
                    Position::new(1, 14)..Position::new(1, 31),
                ),
                declaration: Declaration::Tags,
            },]
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
            vec![Block {
                attributes: HashMap::new(),
                start_tag_position_range: Position::new(2, 1)..Position::new(2, 8),
                content: Content::source(
                    test_utils::substr_range(contents, "println!(\"hello1\");"),
                    Position::new(2, 11)..Position::new(2, 30),
                ),
                declaration: Declaration::Tags,
            },]
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
            vec![Block {
                attributes: HashMap::new(),
                start_tag_position_range: Position::new(1, 4)..Position::new(1, 11),
                content: Content::source(
                    test_utils::substr_range(contents, "println!(\"hello1\");"),
                    Position::new(1, 14)..Position::new(1, 33),
                ),
                declaration: Declaration::Tags,
            },]
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
            vec![Block {
                attributes: HashMap::new(),
                start_tag_position_range: Position::new(1, 9)..Position::new(1, 16),
                content: Content::source(
                    test_utils::substr_range(contents, "\nlet say = \"hi\";\n"),
                    Position::new(1, 16)..Position::new(3, 1),
                ),
                declaration: Declaration::Tags,
            },]
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
            vec![Block {
                attributes: HashMap::new(),
                start_tag_position_range: Position::new(2, 9)..Position::new(2, 16),
                content: Content::source(
                    test_utils::substr_range(contents, "\nlet say = \"hi\";\n"),
                    Position::new(2, 19)..Position::new(4, 1),
                ),
                declaration: Declaration::Tags,
            },]
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
        assert_eq!(
            error_message,
            "Block at line 2, column 12 is not closed. If this is not meant as a tag, put it in backticks."
        );
        Ok(())
    }

    #[test]
    fn unclosed_block_on_a_later_comment_line_reports_that_line_and_column() -> anyhow::Result<()> {
        let mut parser = create_parser();
        let contents = "/*\n * checks the <block> rules\n */\nfn foo() {}\n";
        let error_message = parse_all(&mut parser, contents).unwrap_err().to_string();
        assert_eq!(
            error_message,
            "Block at line 2, column 15 is not closed. If this is not meant as a tag, put it in backticks."
        );
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
        assert_eq!(
            error_message,
            "Block at line 2, column 12 is not closed. If this is not meant as a tag, put it in backticks."
        );
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
        assert_eq!(
            result.unwrap_err().to_string(),
            "Unexpected closed block at line 5, column 12. If this is not meant as a tag, put it in backticks."
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
        assert!(parse_all(&mut parser, contents).is_err());
        Ok(())
    }

    #[test]
    fn unclosed_start_tag_with_nothing_else_in_the_file_returns_error() -> anyhow::Result<()> {
        let mut parser = create_parser();
        // No other tag anywhere in the file, so nothing else can trip an error: the missing `>`
        // has to be caught on its own.
        let contents = "// <block name=\"foo\" keep-sorted\nfn foo() {}\n";
        let error_message = parse_all(&mut parser, contents).unwrap_err().to_string();
        assert_eq!(
            error_message,
            "Malformed block tag at line 1, column 4. If this is not meant as a tag, put it in backticks."
        );
        Ok(())
    }

    #[test]
    fn unclosed_end_tag_with_nothing_else_in_the_file_returns_error() -> anyhow::Result<()> {
        let mut parser = create_parser();
        let contents = "// </block\nfn foo() {}\n";
        let error_message = parse_all(&mut parser, contents).unwrap_err().to_string();
        assert_eq!(
            error_message,
            "Malformed block tag at line 1, column 4. If this is not meant as a tag, put it in backticks."
        );
        Ok(())
    }

    #[test]
    fn malformed_tag_on_a_later_line_reports_that_line_and_column() -> anyhow::Result<()> {
        let mut parser = create_parser();
        let contents = "fn foo() {}\n    // <block name=\"foo\" keep-sorted\n";
        let error_message = parse_all(&mut parser, contents).unwrap_err().to_string();
        assert_eq!(
            error_message,
            "Malformed block tag at line 2, column 8. If this is not meant as a tag, put it in backticks."
        );
        Ok(())
    }

    #[test]
    fn start_tag_continued_after_code_returns_error() -> anyhow::Result<()> {
        let mut parser = create_parser();
        // `A` reads as a valueless attribute, so the tag would close if the code between the two
        // comments were read as part of it.
        let contents = "enum E {\n    // <block name=\"foo\"\n    A // keep-sorted>\n}\n";
        let error_message = parse_all(&mut parser, contents).unwrap_err().to_string();
        assert_eq!(
            error_message,
            "Malformed block tag at line 2, column 8. If this is not meant as a tag, put it in backticks."
        );
        Ok(())
    }

    #[test]
    fn word_continuing_past_the_block_prefix_is_not_treated_as_a_tag_attempt() -> anyhow::Result<()>
    {
        let mut parser = create_parser();
        // "blockquote" continues past "block" rather than ending at a boundary, so this is
        // ordinary text, not an attempted tag.
        let contents = "// see <blockquote> for details\nfn foo() {}\n";
        let blocks = parse_all(&mut parser, contents)?;
        assert_eq!(blocks, vec![]);
        Ok(())
    }

    #[test]
    fn punctuation_after_the_block_prefix_is_not_treated_as_a_tag_attempt() -> anyhow::Result<()> {
        let mut parser = create_parser();
        // A real tag has whitespace after "block", or the comment ends there. Prose can have a
        // comma there instead.
        let contents = "// only <block, filesystem> is tested\nfn foo() {}\n";
        let blocks = parse_all(&mut parser, contents)?;
        assert_eq!(blocks, vec![]);
        Ok(())
    }

    #[test]
    fn punctuation_after_the_end_tag_prefix_is_not_treated_as_a_tag_attempt() -> anyhow::Result<()>
    {
        let mut parser = create_parser();
        let contents = "// a block ends at </block, as in HTML\nfn foo() {}\n";
        let blocks = parse_all(&mut parser, contents)?;
        assert_eq!(blocks, vec![]);
        Ok(())
    }

    #[test]
    fn non_breaking_space_after_the_block_prefix_returns_error() -> anyhow::Result<()> {
        let mut parser = create_parser();
        // An editor can type a non-breaking space by accident. The tag must then fail the run,
        // rather than be skipped as prose.
        let contents = "// <block\u{a0}name=\"foo\">\nfn foo() {}\n";
        let error_message = parse_all(&mut parser, contents).unwrap_err().to_string();
        assert_eq!(
            error_message,
            "Malformed block tag at line 1, column 4. If this is not meant as a tag, put it in backticks."
        );
        Ok(())
    }

    #[test]
    fn tags_in_code_spans_are_not_read() -> anyhow::Result<()> {
        let mut parser = create_parser();
        let contents = "// `<block keep-sorted>`, `</block>`, `<block name=x` <block name=\"a\">\nfn foo() {}\n// </block>\n";
        let blocks = parse_all(&mut parser, contents)?;
        assert_eq!(
            blocks
                .iter()
                .map(|block| block.attributes["name"].as_str())
                .collect::<Vec<_>>(),
            ["a"]
        );
        Ok(())
    }

    #[test]
    fn backtick_without_a_closer_on_its_line_is_ordinary_text() -> anyhow::Result<()> {
        let mut parser = create_parser();
        let contents = "/* it`s <block name=\"a\">\n` */\nfn foo() {}\n/* </block> */\n";
        let blocks = parse_all(&mut parser, contents)?;
        assert_eq!(blocks.len(), 1);
        Ok(())
    }

    #[test]
    fn code_span_ends_only_at_as_many_backticks_as_it_starts_with() -> anyhow::Result<()> {
        let mut parser = create_parser();
        let contents = "// ``a ` <block> `` and `b `` <block> `\nfn foo() {}\n";
        let blocks = parse_all(&mut parser, contents)?;
        assert_eq!(blocks, vec![]);
        Ok(())
    }

    #[test]
    fn block_closed_before_an_unclosed_one_is_returned_ahead_of_the_error() {
        let mut parser = create_parser();
        let contents = "// <block name=\"closed\">\n// </block>\n// <block name=\"open\">";

        let mut blocks = parser.parse_blocks(contents);

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
            "Block at line 3, column 4 is not closed. If this is not meant as a tag, put it in backticks."
        );
        assert!(blocks.next().is_none());
    }

    #[test]
    fn block_closed_inside_unclosed_ones_is_not_returned() {
        let mut parser = create_parser();
        // `inner` is complete, but both of the blocks holding it are left open. Its tags could
        // just as well have been mispaired by the missing ones, so it is not handed over.
        let contents = "// <block name=\"outer\">\n// <block name=\"middle\">\n// <block name=\"inner\">\n// </block>";

        let mut blocks = parser.parse_blocks(contents);

        assert_eq!(
            blocks
                .next()
                .expect("an error")
                .expect_err("the unclosed block")
                .to_string(),
            "Block at line 2, column 4 is not closed. If this is not meant as a tag, put it in backticks."
        );
        assert!(blocks.next().is_none());
    }

    #[test]
    fn block_following_the_first_error_is_not_returned() {
        let mut parser = create_parser();
        // The stray end tag has nothing to close, so the block following it is never reached.
        let contents = "// </block>\n// <block>\n// </block>";

        let mut blocks = parser.parse_blocks(contents);

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
    fn escaped_quotes_in_quoted_attribute_values_return_the_values_verbatim() -> anyhow::Result<()>
    {
        let mut parser = create_parser();
        let contents = r#"
        // <block keep-unique="\"(?P<value>[a-z]+)\"" line-pattern='^\'[a-z]+\'$'>
        // </block>
        "#;
        let blocks = parse_all(&mut parser, contents)?;
        assert_eq!(
            blocks[0].attributes,
            HashMap::from([
                (
                    "keep-unique".to_string(),
                    r#"\"(?P<value>[a-z]+)\""#.to_string()
                ),
                ("line-pattern".to_string(), r"^\'[a-z]+\'$".to_string())
            ])
        );
        Ok(())
    }

    #[test]
    fn quoted_attribute_values_ending_in_two_backslashes_return_the_values_verbatim()
    -> anyhow::Result<()> {
        let mut parser = create_parser();
        let contents = r#"
        // <block line-pattern="^C:\\" keep-unique='\w+\\'>
        // </block>
        "#;
        let blocks = parse_all(&mut parser, contents)?;
        assert_eq!(
            blocks[0].attributes,
            HashMap::from([
                ("line-pattern".to_string(), r"^C:\\".to_string()),
                ("keep-unique".to_string(), r"\w+\\".to_string())
            ])
        );
        Ok(())
    }

    #[test]
    fn quoted_attribute_value_ending_in_one_backslash_returns_error() -> anyhow::Result<()> {
        let mut parser = create_parser();
        // The backslash escapes the quote after it, so the value has no closing quote.
        let contents = "// <block name=\"foo\\\">\nfn foo() {}\n// </block>\n";
        let error_message = parse_all(&mut parser, contents).unwrap_err().to_string();
        assert_eq!(
            error_message,
            "Malformed block tag at line 1, column 4. If this is not meant as a tag, put it in backticks."
        );
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
