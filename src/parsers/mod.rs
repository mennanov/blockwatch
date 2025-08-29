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
mod toml;
mod tsx;
mod typescript;
mod xml;
mod yaml;

use crate::blocks::{Block, BlockBuilder};
use anyhow::Context;
use quick_xml::events::Event;
use std::collections::HashMap;
use std::rc::Rc;
use std::string::ToString;
use tree_sitter::{Language, Parser, Query, QueryCursor, StreamingIterator};

/// Parses [`Blocks`] from a source code.
pub(crate) trait BlocksParser {
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

/// Returns a map of all available language parsers by their file extensions.
pub(crate) fn language_parsers() -> anyhow::Result<HashMap<String, Rc<Box<dyn BlocksParser>>>> {
    let bash_parser = Rc::new(bash::parser()?);
    let c_parser = Rc::new(c::parser()?);
    let cpp_parser = Rc::new(cpp::parser()?);
    let c_sharp_parser = Rc::new(c_sharp::parser()?);
    let css_parser = Rc::new(css::parser()?);
    let go_parser = Rc::new(go::parser()?);
    let html_parser = Rc::new(html::parser()?);
    let kotlin_parser = Rc::new(kotlin::parser()?);
    let java_parser = Rc::new(java::parser()?);
    let js_parser = Rc::new(javascript::parser()?);
    let rust_parser = Rc::new(rust::parser()?);
    let markdown_parser = Rc::new(markdown::parser()?);
    let php_parser = Rc::new(php::parser()?);
    let python_parser = Rc::new(python::parser()?);
    let ruby_parser = Rc::new(ruby::parser()?);
    let sql_parser = Rc::new(sql::parser()?);
    let swift_parser = Rc::new(swift::parser()?);
    let toml_parser = Rc::new(toml::parser()?);
    let typescript_parser = Rc::new(typescript::parser()?);
    let typescript_tsx_parser = Rc::new(tsx::parser()?);
    let yaml_parser = Rc::new(yaml::parser()?);
    let xml_parser = Rc::new(xml::parser()?);
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
            let mut matches = query_cursor.matches(query, root_node, source_code.as_bytes());
            while let Some(query_match) = matches.next() {
                for capture in query_match.captures {
                    let node = capture.node;
                    let start_line = node.start_position().row + 1; // Convert to 1-based indexing
                    let comment_text = &source_code[node.start_byte()..node.end_byte()];
                    if let Some(processor) = post_processor {
                        if let Some(out) = processor(capture.index as usize, comment_text) {
                            blocks.push((start_line, out));
                        }
                    } else {
                        blocks.push((start_line, comment_text.to_string()));
                    }
                }
            }
        }

        blocks.sort_by(|(start_line1, _), (start_line2, _)| start_line1.cmp(start_line2));
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
}

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
                    let mut attributes = HashMap::new();
                    for attr in event.attributes() {
                        let attr = attr.context("Failed to parse attribute")?;
                        attributes.insert(
                            String::from_utf8(attr.key.as_ref().into())?,
                            String::from_utf8(attr.value.into())?,
                        );
                    }
                    stack.push(BlockBuilder::new(starts_at, attributes));
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
                    if let Some(block_builder) = stack.pop() {
                        let starts_at = block_builder.starts_at;
                        let content = if ends_at > starts_at {
                            // TODO: optimize by using `match_indices('\n')`
                            contents
                                .lines()
                                .skip(starts_at)
                                .take(ends_at - starts_at - 1)
                                .collect::<Vec<_>>()
                                .join("\n")
                        } else {
                            // Block opened and closed at the same line.
                            String::new()
                        };
                        blocks.push(block_builder.build(ends_at, content));
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
                "Block at line {} is not closed",
                unclosed_block.starts_at
            )));
        }
        blocks.sort_by(|a, b| a.starts_at_line.cmp(&b.starts_at_line));

        Ok(blocks)
    }
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
                Block::new(
                    2,
                    6,
                    HashMap::from([("name".to_string(), "foo".to_string())]),
                    r#"        fn say_hello_world() {
          println!("hello world!");
        }"#
                    .into()
                ),
                Block::new(
                    9,
                    13,
                    HashMap::from([("name".to_string(), "bar".to_string())]),
                    r#"        fn say_hello_world2() {
          println!("hello world 2!");
        }"#
                    .into()
                ),
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
                Block::new(
                    2,
                    25,
                    HashMap::from([("name".to_string(), "foo".to_string())]),
                    r#"        fn say_hello_world() {
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
                    r#"            fn say_hello_world_bar() {
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
                    r#"                fn say_hello_world_bar_bar() {
                  println!("hello world bar bar!");
                }"#
                    .into()
                ),
                Block::new(
                    19,
                    23,
                    HashMap::from([("name".to_string(), "buzz".to_string())]),
                    r#"            fn say_hello_world_buzz() {
              println!("hello world buzz!");
            }"#
                    .into()
                ),
                Block::new(
                    26,
                    27,
                    HashMap::from([("name".to_string(), "fizz".to_string())]),
                    "".into()
                ),
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
            vec![Block::new(
                1,
                5,
                HashMap::from([("name".to_string(), "foo".to_string())]),
                r#"        fn say_hello_world() {
          println!("hello world!");
        }"#
                .into()
            )]
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
                Block::new(
                    1,
                    1,
                    HashMap::from([("name".to_string(), "foo".to_string())]),
                    "".into()
                ),
                Block::new(
                    2,
                    2,
                    HashMap::from([("name".to_string(), "bar".to_string())]),
                    "".into()
                ),
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
    fn attributes_parsed_correctly() -> anyhow::Result<()> {
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
    fn nested_blocks_with_attributes_parsed_correctly() -> anyhow::Result<()> {
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
    fn empty_attributes_parsed_correctly() -> anyhow::Result<()> {
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
}
