mod bash;
mod c;
mod c_sharp;
mod cpp;
mod css;
mod go;
mod html;
mod java;
mod javascript;
mod kotlin;
mod markdown;
mod php;
mod python;
mod ruby;
mod rust;
mod sql;
mod swift;
mod tag_parser;
mod toml;
mod tsx;
mod typescript;
mod xml;
mod yaml;

use crate::blocks::Block;
use std::collections::HashMap;
use std::rc::Rc;
use std::string::ToString;
use tag_parser::{BlockTag, BlockTagParser, QuickXmlBlockTagParser};
use tree_sitter::{Language, Parser, Query, QueryCursor, StreamingIterator};

/// Parses [`Blocks`] from a source code.
pub trait BlocksParser {
    /// Returns [`Block`]s extracted from the given `contents` string.
    ///
    /// The blocks are required to be sorted by the `starts_at` field in ascending order.
    fn parse(&self, contents: &str) -> anyhow::Result<Vec<Block>>;
}

/// Parses comment strings from a source code.
trait CommentsParser {
    /// Returns a `Vec` of `Comment`s.
    fn parse(&self, source_code: &str) -> anyhow::Result<Vec<Comment>>;
}

/// Returns a map of all available language parsers by their file extensions.
pub fn language_parsers() -> anyhow::Result<HashMap<String, Rc<Box<dyn BlocksParser>>>> {
    let bash_parser = Rc::new(bash::parser()?);
    let c_parser = Rc::new(c::parser()?);
    let c_sharp_parser = Rc::new(c_sharp::parser()?);
    let cpp_parser = Rc::new(cpp::parser()?);
    let css_parser = Rc::new(css::parser()?);
    let go_parser = Rc::new(go::parser()?);
    let html_parser = Rc::new(html::parser()?);
    let java_parser = Rc::new(java::parser()?);
    let js_parser = Rc::new(javascript::parser()?);
    let kotlin_parser = Rc::new(kotlin::parser()?);
    let markdown_parser = Rc::new(markdown::parser()?);
    let php_parser = Rc::new(php::parser()?);
    let python_parser = Rc::new(python::parser()?);
    let ruby_parser = Rc::new(ruby::parser()?);
    let rust_parser = Rc::new(rust::parser()?);
    let sql_parser = Rc::new(sql::parser()?);
    let swift_parser = Rc::new(swift::parser()?);
    let toml_parser = Rc::new(toml::parser()?);
    let typescript_parser = Rc::new(typescript::parser()?);
    let typescript_tsx_parser = Rc::new(tsx::parser()?);
    let xml_parser = Rc::new(xml::parser()?);
    let yaml_parser = Rc::new(yaml::parser()?);
    Ok(HashMap::from([
        // <block affects="README.md:supported-grammar" keep-sorted="asc">
        ("bash".into(), Rc::clone(&bash_parser)),
        ("c".into(), c_parser),
        ("cc".into(), Rc::clone(&cpp_parser)),
        ("cpp".into(), Rc::clone(&cpp_parser)),
        ("cs".into(), c_sharp_parser),
        ("css".into(), css_parser),
        ("d.ts".into(), Rc::clone(&typescript_parser)),
        ("go".into(), go_parser),
        ("h".into(), cpp_parser),
        ("htm".into(), Rc::clone(&html_parser)),
        ("html".into(), html_parser),
        ("java".into(), java_parser),
        ("js".into(), Rc::clone(&js_parser)),
        ("jsx".into(), js_parser),
        ("kt".into(), Rc::clone(&kotlin_parser)),
        ("kts".into(), kotlin_parser),
        ("markdown".into(), Rc::clone(&markdown_parser)),
        ("md".into(), markdown_parser),
        ("php".into(), Rc::clone(&php_parser)),
        ("phtml".into(), php_parser),
        ("py".into(), Rc::clone(&python_parser)),
        ("pyi".into(), python_parser),
        ("rb".into(), ruby_parser),
        ("rs".into(), rust_parser),
        ("sh".into(), bash_parser),
        ("sql".into(), sql_parser),
        ("swift".into(), swift_parser),
        ("toml".into(), toml_parser),
        ("ts".into(), typescript_parser),
        ("tsx".into(), typescript_tsx_parser),
        ("xml".into(), xml_parser),
        ("yaml".into(), Rc::clone(&yaml_parser)),
        ("yml".into(), yaml_parser),
        // </block>
    ]))
}

struct TreeSitterCommentsParser<F: Fn(usize, &str) -> Option<String>> {
    language: Language,
    queries: Vec<(Query, Option<F>)>,
}

impl<F: Fn(usize, &str) -> Option<String>> TreeSitterCommentsParser<F> {
    fn new(language: Language, queries: Vec<(Query, Option<F>)>) -> Self {
        Self { language, queries }
    }
}

impl<F: Fn(usize, &str) -> Option<String>> CommentsParser for TreeSitterCommentsParser<F> {
    fn parse(&self, source_code: &str) -> anyhow::Result<Vec<Comment>> {
        let mut parser = Parser::new();
        parser
            .set_language(&self.language)
            .expect("Error setting Tree-sitter language");
        let tree = parser.parse(source_code, None).unwrap();
        let root_node = tree.root_node();

        let mut blocks = vec![];
        for (query, post_processor) in self.queries.iter() {
            let mut query_cursor = QueryCursor::new();
            let mut matches = query_cursor.matches(query, root_node, source_code.as_bytes());
            while let Some(query_match) = matches.next() {
                for capture in query_match.captures {
                    let node = capture.node;
                    let start_line = node.start_position().row + 1; // Convert to 1-based indexing
                    let start_position = node.start_byte();
                    let end_position = node.end_byte();
                    let comment_text = &source_code[node.start_byte()..node.end_byte()];
                    if let Some(processor) = post_processor {
                        if let Some(out) = processor(capture.index as usize, comment_text) {
                            blocks.push(Comment {
                                source_line_number: start_line,
                                source_start_position: start_position,
                                source_end_position: end_position,
                                comment_text: out,
                            });
                        }
                    } else {
                        blocks.push(Comment {
                            source_line_number: start_line,
                            source_start_position: start_position,
                            source_end_position: end_position,
                            comment_text: comment_text.to_string(),
                        });
                    }
                }
            }
        }

        blocks.sort_by(|comment1, comment2| {
            comment1
                .source_start_position
                .cmp(&comment2.source_start_position)
        });
        Ok(blocks)
    }
}

struct BlocksFromCommentsParser<C: CommentsParser> {
    comments_parser: C,
}

#[derive(Debug, PartialEq)]
struct Comment {
    // 1-based line number of the start of this comment in the source.
    source_line_number: usize,
    // Byte offset (i.e. position) of the start of this comment in the source.
    source_start_position: usize,
    // Byte offset (i.e. position) of the end of this comment in the source.
    source_end_position: usize,
    // The `comment_string` is expected to be the actual content of the comment without any
    // language specific symbols like "//", "/**", etc. However, it **should preserve the line
    // breaks**.
    comment_text: String,
}

/// Represents a comment's metadata.
#[derive(PartialEq)]
struct CommentIndex {
    // Comment's start position in **concatenated** comments (not in original source).
    start_position: usize,
    // Comment's end position in **concatenated** comments (not in original source).
    end_position: usize,
    // Comment's 1-based line number in the source.
    source_start_line_number: usize,
    // Comment's starting position in the source.
    source_start_position: usize,
    // Comment's end position in the source.
    source_end_position: usize,
}

impl<C: CommentsParser> BlocksFromCommentsParser<C> {
    fn new(comments_parser: C) -> Self {
        Self { comments_parser }
    }

    /// Returns a string of concatenated `comments` and its corresponding `CommentIndex`.
    fn build_index(comments: &[Comment]) -> (String, Vec<CommentIndex>) {
        let mut concatenated_comments = String::new();
        let mut index = Vec::new();
        for comment in comments {
            index.push(CommentIndex {
                start_position: concatenated_comments.len(),
                end_position: concatenated_comments.len() + comment.comment_text.len(),
                source_start_line_number: comment.source_line_number,
                source_start_position: comment.source_start_position,
                source_end_position: comment.source_end_position,
            });
            concatenated_comments.push_str(&comment.comment_text);
        }

        (concatenated_comments, index)
    }

    fn process_start_tag<'idx>(
        stack: &mut Vec<BlockBuilder<'idx>>,
        concatenated_comments: &str,
        index: &'idx [CommentIndex],
        start_position: usize,
        end_position: usize,
        attributes: HashMap<String, String>,
    ) {
        let comment_index = index
            .get(
                index
                    .binary_search_by(|comment_index| comment_index.end_position.cmp(&end_position))
                    .unwrap_or_else(|e| e),
            )
            .expect("start comment index out of bounds");
        let block_comment =
            &concatenated_comments[comment_index.start_position..start_position - 1];
        let start_line_number =
            comment_index.source_start_line_number + block_comment.lines().count() - 1;
        stack.push(BlockBuilder::new(
            start_line_number,
            comment_index,
            attributes,
        ));
    }

    fn process_end_tag<'idx>(
        stack: &mut Vec<BlockBuilder<'idx>>,
        blocks: &mut Vec<Block>,
        contents: &str,
        concatenated_comments: &str,
        index: &'idx [CommentIndex],
        start_position: usize,
        end_position: usize,
    ) -> anyhow::Result<()> {
        let idx = index
            .binary_search_by(|comment_index| comment_index.end_position.cmp(&end_position))
            .unwrap_or_else(|e| e);
        let comment_index = index.get(idx).expect("end comment index out of bounds");
        if let Some(block_builder) = stack.pop() {
            let block_content = if comment_index != block_builder.start_index {
                &contents[block_builder.start_index.source_end_position
                    ..comment_index.source_start_position]
            } else {
                // Block that starts and ends in the same comment can't have any
                // content.
                ""
            };
            // TODO: get rid of the Block.ends_at_line and use block's source positions instead to compute intersections.
            let block_comment = &concatenated_comments[comment_index.start_position..end_position];
            let end_line_number =
                comment_index.source_start_line_number + block_comment.lines().count() - 1;
            blocks.push(block_builder.build(end_line_number, block_content.to_string()));
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "Unexpected closed block at line {}, position {}",
                comment_index.source_start_line_number,
                comment_index.source_start_position + start_position
            ))
        }
    }
}

impl<C: CommentsParser> BlocksParser for BlocksFromCommentsParser<C> {
    fn parse(&self, contents: &str) -> anyhow::Result<Vec<Block>> {
        let comments = self.comments_parser.parse(contents)?;
        let (concatenated_comments, index) = Self::build_index(&comments);
        let mut blocks = Vec::new();
        let mut stack = Vec::new();
        let mut parser = QuickXmlBlockTagParser::new(concatenated_comments.as_str());

        while let Some(tag) = parser.next()? {
            match tag {
                BlockTag::Start {
                    start_position,
                    end_position,
                    attributes,
                } => {
                    Self::process_start_tag(
                        &mut stack,
                        &concatenated_comments,
                        &index,
                        start_position,
                        end_position,
                        attributes,
                    );
                }
                BlockTag::End {
                    start_position,
                    end_position,
                } => {
                    Self::process_end_tag(
                        &mut stack,
                        &mut blocks,
                        contents,
                        concatenated_comments.as_str(),
                        &index,
                        start_position,
                        end_position,
                    )?;
                }
            }
        }

        if let Some(unclosed_block) = stack.pop() {
            return Err(anyhow::anyhow!(format!(
                "Block at line {} is not closed",
                unclosed_block.start_index.source_start_line_number
            )));
        }
        blocks.sort_by(|a, b| a.starts_at_line.cmp(&b.starts_at_line));

        Ok(blocks)
    }
}

struct BlockBuilder<'a> {
    starts_at_line: usize,
    start_index: &'a CommentIndex,
    attributes: HashMap<String, String>,
}

impl<'a> BlockBuilder<'a> {
    fn new(
        starts_at_line: usize,
        start_index: &'a CommentIndex,
        attributes: HashMap<String, String>,
    ) -> Self {
        Self {
            starts_at_line,
            start_index,
            attributes,
        }
    }

    pub(crate) fn build(self, ends_at: usize, content: String) -> Block {
        Block::new(self.starts_at_line, ends_at, self.attributes, content)
    }
}

/// C-style comments parser for a query that returns both line and block comments.
fn c_style_comments_parser(
    language: Language,
    query: Query,
) -> TreeSitterCommentsParser<fn(usize, &str) -> Option<String>> {
    TreeSitterCommentsParser::<fn(usize, &str) -> Option<String>>::new(
        language,
        vec![(
            query,
            Some(|_, comment| {
                Some(comment.strip_prefix("//").map_or_else(
                    || c_style_multiline_comment_processor(comment),
                    |c| c.trim().to_string(),
                ))
            }),
        )],
    )
}

/// C-style comments parser for the separate line and block comment queries.
fn c_style_line_and_block_comments_parser(
    language: Language,
    line_comment_query: Query,
    block_comment_query: Query,
) -> TreeSitterCommentsParser<fn(usize, &str) -> Option<String>> {
    TreeSitterCommentsParser::<fn(usize, &str) -> Option<String>>::new(
        language,
        vec![
            (
                line_comment_query,
                Some(|_, comment| Some(comment.strip_prefix("//").unwrap().trim().to_string())),
            ),
            (
                block_comment_query,
                Some(|_, comment| Some(c_style_multiline_comment_processor(comment))),
            ),
        ],
    )
}

/// Python-style comments parser.
fn python_style_comments_parser(
    language: Language,
    comment_query: Query,
) -> TreeSitterCommentsParser<fn(usize, &str) -> Option<String>> {
    TreeSitterCommentsParser::<fn(usize, &str) -> Option<String>>::new(
        language,
        vec![(
            comment_query,
            Some(|_, comment| Some(comment.strip_prefix('#').unwrap().trim().to_string())),
        )],
    )
}

/// XML-style comments parser.
fn xml_style_comments_parser(
    language: Language,
    comment_query: Query,
) -> TreeSitterCommentsParser<fn(usize, &str) -> Option<String>> {
    TreeSitterCommentsParser::<fn(usize, &str) -> Option<String>>::new(
        language,
        vec![(
            comment_query,
            Some(|_, comment| {
                Some(
                    comment
                        .strip_prefix("<!--")
                        .unwrap()
                        .trim_end_matches("-->")
                        .lines()
                        .map(|line| line.trim())
                        .collect::<Vec<_>>()
                        .join("\n"),
                )
            }),
        )],
    )
}

fn c_style_multiline_comment_processor(comment: &str) -> String {
    comment
        .strip_prefix("/*")
        .unwrap()
        .lines()
        .map(|line| {
            line.trim_start()
                .trim_start_matches('*')
                .trim()
                .trim_end_matches('/')
                .trim_end_matches('*')
                .trim()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_parser() -> Box<dyn BlocksParser> {
        rust::parser().unwrap()
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
                " let say = \"hi\"; ".to_string()
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
                "\nlet say = \"hi\";\n".to_string()
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
                " let say = \"hi\"; ".to_string()
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
                2,
                HashMap::new(),
                " let say = \"hi\"; ".to_string()
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
                    "\nprintln!(\"hello1\");\n".to_string()
                ),
                Block::new(
                    4,
                    6,
                    HashMap::new(),
                    "\nprintln!(\"hello2\");\n".to_string()
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
                    "\nprintln!(\"hello1\");\n".to_string()
                ),
                Block::new(
                    3,
                    5,
                    HashMap::new(),
                    "\nprintln!(\"hello2\");\n".to_string()
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
                Block::new(1, 1, HashMap::new(), "println!(\"hello1\");".to_string()),
                Block::new(1, 1, HashMap::new(), "println!(\"hello2\");".to_string())
            ]
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
                    r#"
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

        "#
                    .into()
                ),
                Block::new(
                    7,
                    17,
                    HashMap::from([("name".to_string(), "bar".to_string())]),
                    r#"
            fn say_hello_world_bar() {
              println!("hello world bar!");
            }
                // <block name="bar-bar">
                fn say_hello_world_bar_bar() {
                  println!("hello world bar bar!");
                }
                // </block>

            "#
                    .into()
                ),
                Block::new(
                    11,
                    15,
                    HashMap::from([("name".to_string(), "bar-bar".to_string())]),
                    r#"
                fn say_hello_world_bar_bar() {
                  println!("hello world bar bar!");
                }
                "#
                    .into()
                ),
                Block::new(
                    19,
                    23,
                    HashMap::from([("name".to_string(), "buzz".to_string())]),
                    r#"
            fn say_hello_world_buzz() {
              println!("hello world buzz!");
            }
            "#
                    .into()
                ),
                Block::new(
                    26,
                    27,
                    HashMap::from([("name".to_string(), "fizz".to_string())]),
                    "\n        ".into()
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
        // </block> Some comment.
        "#;
        let blocks = parser.parse(contents)?;
        assert_eq!(
            blocks,
            vec![Block::new(
                1,
                3,
                HashMap::from([("name".to_string(), "foo".to_string())]),
                "\n        let word = \"hello\";\n        ".into()
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
        assert!(parser.parse(contents).is_err());
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
        fn escaped() {}
        // </block>
        "#;
        let blocks = parser.parse(contents)?;
        assert_eq!(blocks[0].attributes["text"], "He said \"Hello\"");
        Ok(())
    }

    #[test]
    fn attributes_with_escaped_quotes() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = r#"
        // <block text="He said &quot;Hello&quot;">
        fn escaped() {}
        // </block>
        "#;
        let blocks = parser.parse(contents)?;
        assert_eq!(blocks[0].attributes["text"], "He said \"Hello\"");
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
    fn empty_attributes() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = r#"
        // <block name="" foo="" bar="">
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
    fn malformed_block_tag_returns_error() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = r#"
        // <block name="foo" affects="file:block" invalid>
        fn foo() {}
        // </block>
        "#;
        assert!(parser.parse(contents).is_err());
        Ok(())
    }

    #[test]
    fn blocks_with_unicode_attributes() -> anyhow::Result<()> {
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
    fn blocks_with_different_line_endings() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = "// <block>\r\nWindows\r\n// </block>\n// <block>\nUnix\n// </block>";
        let blocks = parser.parse(contents)?;
        assert_eq!(blocks.len(), 2);
        assert!(blocks[0].content.contains("\r\n"));
        assert!(blocks[1].content.contains("\n"));
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
    #[ignore] // Invalid XML-like tags in comments are currently not supported.
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
