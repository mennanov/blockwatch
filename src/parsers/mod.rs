mod java;
mod rust;

use anyhow::Context;
use quick_xml::events::Event;
use std::string::ToString;
use tree_sitter::{Language, Parser, Query, QueryCursor, StreamingIterator};

#[derive(Debug, PartialEq)]
struct Block {
    name: Option<String>,
    starts_at: usize,
    ends_at: usize,
    affects: Vec<(Option<String>, String)>,
}

/// Parses [`Blocks`] from a source code.
trait BlocksParser {
    /// Returns [`Block`]s extracted from the given `contents` string.
    ///
    /// The blocks are required to be sorted by the `starts_at` field in ascending order.
    fn parse(&self, contents: &str) -> anyhow::Result<Vec<Block>>;
}

/// Parses comment strings from a source code.
trait CommentsParser {
    /// Returns a `Vec` of pairs `(comment_start_line, comment_string)`.
    ///
    /// The `comment_start_line` is a 1-based index of the line the comment starts at.
    ///
    /// The `comment_string` is expected to be the actual content of the comment without any
    /// language specific symbols like "//", "/**", etc. However, it **should preserve the line
    /// breaks**.
    fn parse(&self, source_code: &str) -> anyhow::Result<Vec<(usize, String)>>;
}

struct TreeSitterCommentsParser<F: Fn(&str) -> String> {
    language: Language,
    queries: Vec<(Query, Option<F>)>,
}

impl<F: Fn(&str) -> String> TreeSitterCommentsParser<F> {
    fn new(language: Language, queries: Vec<(Query, Option<F>)>) -> Self {
        Self { language, queries }
    }
}

impl<F: Fn(&str) -> String> CommentsParser for TreeSitterCommentsParser<F> {
    fn parse(&self, source_code: &str) -> anyhow::Result<Vec<(usize, String)>> {
        let mut parser = Parser::new();
        parser
            .set_language(&self.language)
            .expect("Error setting Tree-sitter language");
        let tree = parser.parse(source_code, None).unwrap();
        let root_node = tree.root_node();

        let mut blocks = vec![];
        for (query, post_processor) in self.queries.iter() {
            let mut query_cursor = QueryCursor::new();
            let mut matches = query_cursor.matches(&query, root_node, source_code.as_bytes());
            while let Some(query_match) = matches.next() {
                for capture in query_match.captures {
                    let node = capture.node;
                    let start_line = node.start_position().row + 1; // Convert to 1-based indexing
                    let comment_text = &source_code[node.start_byte()..node.end_byte()];
                    if let Some(processor) = post_processor {
                        blocks.push((start_line, processor(comment_text)));
                    } else {
                        blocks.push((start_line, comment_text.to_string()));
                    }
                }
            }
        }

        blocks.sort_by(|(start_line1, _), (start_line2, _)| start_line1.cmp(&start_line2));
        Ok(blocks)
    }
}

struct BlocksFromCommentsParser<C: CommentsParser> {
    comments_parser: C,
}

impl<C: CommentsParser> BlocksFromCommentsParser<C> {
    fn new(comments_parser: C) -> Self {
        Self { comments_parser }
    }

    /// Returns a string of concatenated `comments` and its corresponding index.
    ///
    /// The index is represented as a sorted `Vec` of a character position and its corresponding
    /// line number in the original source code.
    fn build_index(comments: &[(usize, String)]) -> (String, Vec<(usize, usize)>) {
        let mut result = String::new();
        let mut index: Vec<(usize, usize)> = Vec::new();
        for (comment_start_line, comment) in comments {
            for (line_number, line) in comment.lines().enumerate() {
                if !result.is_empty() {
                    result.push('\n');
                }
                index.push((result.len() + line.len(), comment_start_line + line_number));
                result.push_str(line);
            }
        }

        (result, index)
    }

    fn parse_affects_attribute(value: &str) -> anyhow::Result<Vec<(Option<String>, String)>> {
        let mut result = Vec::new();
        for block_ref in value.split(",") {
            let block = block_ref.trim();
            let (mut filename, block_name) = block.split_once(":").context(format!(
                "Failed to parse filename and block name in '{:?}'",
                block
            ))?;
            filename = filename.trim();
            result.push((
                if filename.is_empty() {
                    None
                } else {
                    Some(filename.to_string())
                },
                block_name.trim().to_string(),
            ));
        }
        Ok(result)
    }
}

const UNNAMED_BLOCK_LABEL: &str = "(unnamed)";

impl<C: CommentsParser> BlocksParser for BlocksFromCommentsParser<C> {
    fn parse(&self, contents: &str) -> anyhow::Result<Vec<Block>> {
        let comments = self.comments_parser.parse(contents)?;
        let (concatenated_comments, index) = Self::build_index(&comments);
        let mut blocks = Vec::new();
        let mut stack = Vec::new();
        let mut reader = quick_xml::Reader::from_str(concatenated_comments.as_str());
        loop {
            let event = reader.read_event()?;
            match event {
                Event::Start(event) => {
                    if event.name().as_ref() != b"block" {
                        continue;
                    }
                    let starts_at = index[index
                        .binary_search_by(|(line_start_position, _)| {
                            line_start_position.cmp(&(reader.buffer_position() as usize))
                        })
                        .unwrap_or_else(|e| e)]
                    .1;
                    let mut name = None;
                    let mut affects = vec![];
                    for attr in event.attributes() {
                        let attr = attr.context("Failed to parse attribute")?;
                        let attr_name = attr.key.as_ref();
                        if attr_name == b"name" {
                            name = String::from_utf8(attr.value.into()).map(|v| Some(v))?;
                        } else if attr_name == b"affects" {
                            affects = Self::parse_affects_attribute(
                                String::from_utf8(attr.value.into())?.as_str(),
                            )?;
                        }
                    }
                    stack.push(Block {
                        name,
                        starts_at,
                        ends_at: 0,
                        affects,
                    });
                }
                Event::End(event) => {
                    if event.name().as_ref() != b"block" {
                        continue;
                    }
                    let ends_at = index[index
                        .binary_search_by(|(line_start_position, _)| {
                            line_start_position.cmp(&(reader.buffer_position() as usize))
                        })
                        .unwrap_or_else(|e| e)]
                    .1;
                    if let Some(mut block) = stack.pop() {
                        block.ends_at = ends_at;
                        blocks.push(block);
                    } else {
                        return Err(anyhow::anyhow!(
                            "Unexpected closed block at line {}",
                            ends_at
                        ));
                    }
                }
                Event::Eof => break,
                _ => {}
            }
        }
        if let Some(unclosed_block) = stack.pop() {
            return Err(anyhow::anyhow!(format!(
                "Block \"{}\" at line {} is not closed",
                unclosed_block
                    .name
                    .unwrap_or(UNNAMED_BLOCK_LABEL.to_string()),
                unclosed_block.starts_at
            )));
        }
        blocks.sort_by(|a, b| a.starts_at.cmp(&b.starts_at));

        Ok(blocks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_parser() -> impl BlocksParser {
        BlocksFromCommentsParser::new(rust::parser().unwrap())
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
    fn unnested_blocks_parsed_correctly() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = r#"
        // <block name="foo">
        fn say_hello_world() {
          println!("hello world!");
        }
        // </block>

        /// Doc string for the function below.
        /// <block name="bar">
        fn say_hello_world2() {
          println!("hello world 2!");
        }
        /// </block>
        "#;
        let blocks = parser.parse(contents)?;
        assert_eq!(
            blocks,
            vec![
                Block {
                    name: Some("foo".into()),
                    starts_at: 2,
                    ends_at: 6,
                    affects: vec![],
                },
                Block {
                    name: Some("bar".into()),
                    starts_at: 9,
                    ends_at: 13,
                    affects: vec![],
                }
            ]
        );
        Ok(())
    }

    #[test]
    fn nested_blocks_parsed_correctly() -> anyhow::Result<()> {
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
                Block {
                    name: Some("foo".into()),
                    starts_at: 2,
                    ends_at: 25,
                    affects: vec![],
                },
                Block {
                    name: Some("bar".into()),
                    starts_at: 7,
                    ends_at: 17,
                    affects: vec![],
                },
                Block {
                    name: Some("bar-bar".into()),
                    starts_at: 11,
                    ends_at: 15,
                    affects: vec![],
                },
                Block {
                    name: Some("buzz".into()),
                    starts_at: 19,
                    ends_at: 23,
                    affects: vec![],
                },
                Block {
                    name: Some("fizz".into()),
                    starts_at: 26,
                    ends_at: 27,
                    affects: vec![],
                }
            ]
        );
        Ok(())
    }

    #[test]
    fn block_defined_at_first_and_last_lines_parsed_correctly() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = r#"// <block name="foo">
        fn say_hello_world() {
          println!("hello world!");
        }
        // </block>"#;
        let blocks = parser.parse(contents)?;
        assert_eq!(
            blocks,
            vec![Block {
                name: Some("foo".into()),
                starts_at: 1,
                ends_at: 5,
                affects: vec![],
            },]
        );
        Ok(())
    }

    #[test]
    fn one_line_blocks_parsed_correctly() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = r#"/*<block name="foo"></block>
        <block name="bar"></block>*/"#;
        let blocks = parser.parse(contents)?;
        assert_eq!(
            blocks,
            vec![
                Block {
                    name: Some("foo".into()),
                    starts_at: 1,
                    ends_at: 1,
                    affects: vec![],
                },
                Block {
                    name: Some("bar".into()),
                    starts_at: 2,
                    ends_at: 2,
                    affects: vec![],
                }
            ]
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
        assert_eq!(error_message, "Block \"foo\" at line 2 is not closed");
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
        assert_eq!(error_message, "Block \"foo\" at line 2 is not closed");
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
        assert!(parser.parse(contents).is_err());
        Ok(())
    }

    #[test]
    fn unnamed_blocks_parsed_correctly() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = r#"
        // <block>
        fn say_hello_world() {
          println!("hello world!");
        }
        // </block>

        // <block>
        fn say_hello_world2() {
          println!("hello world!");
        }
        // </block>
        "#;
        let blocks = parser.parse(contents)?;
        assert_eq!(
            blocks,
            vec![
                Block {
                    name: None,
                    starts_at: 2,
                    ends_at: 6,
                    affects: vec![],
                },
                Block {
                    name: None,
                    starts_at: 8,
                    ends_at: 12,
                    affects: vec![],
                }
            ]
        );
        Ok(())
    }

    #[test]
    fn unnamed_nested_blocks_parsed_correctly() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = r#"
        // <block>
        fn say_hello_world() {
          println!("hello world!");
        }
        // <block>
        fn say_hello_world2() {
          println!("hello world!");
        }
        // <block name="foo">
        fn say_hello_world2() {
          println!("hello world!");
        }
        // </block>
        // </block>
        // </block>
        "#;
        let blocks = parser.parse(contents)?;
        assert_eq!(
            blocks,
            vec![
                Block {
                    name: None,
                    starts_at: 2,
                    ends_at: 16,
                    affects: vec![],
                },
                Block {
                    name: None,
                    starts_at: 6,
                    ends_at: 15,
                    affects: vec![],
                },
                Block {
                    name: Some("foo".into()),
                    starts_at: 10,
                    ends_at: 14,
                    affects: vec![],
                }
            ]
        );
        Ok(())
    }
}
