use anyhow::Context;
use regex::{Regex, RegexBuilder};
use std::string::ToString;

#[derive(Debug, PartialEq)]
struct Block {
    name: Option<String>,
    starts_at: u32,
    ends_at: u32,
    affects: Vec<(Option<String>, String)>,
    children: Vec<Block>,
}

struct ParserConfig<'a> {
    block_start_definition: &'a str,
    block_end_definition: &'a str,
    single_line_comments: Vec<&'a str>,
    multi_line_comments: Vec<MultiLineComment<'a>>,
}

struct MultiLineComment<'a> {
    start: &'a str,
    continuation: Option<&'a str>,
    end: &'a str,
}

/// Parses [`Blocks`] from a source file.
trait BlocksParser {
    /// Returns [`Blocks`] extracted from the given `contents` string.
    fn parse(self, contents: &str, config: &ParserConfig) -> anyhow::Result<Vec<Block>>;
}

struct BlocksRegexParser {}

impl BlocksRegexParser {
    fn new() -> Self {
        BlocksRegexParser {}
    }

    fn single_line_comment_start_end_regex(
        config: &ParserConfig,
    ) -> anyhow::Result<(Regex, Regex)> {
        let single_line_comments_exp = config
            .single_line_comments
            .iter()
            .map(|c| regex::escape(c))
            .collect::<Vec<_>>()
            .join("|");
        let single_line_start_block_regex = RegexBuilder::new(
            format!(
                r#"({})\s*{}\s*(?<name>[\w-]+)?"#,
                single_line_comments_exp, config.block_start_definition
            )
            .as_str(),
        )
        .build()
        .context("failed to build a single line start block regex")?;

        let single_line_end_block_regex = RegexBuilder::new(
            format!(
                r#"({})\s*{}\s*(?<name>[\w-]+)?"#,
                single_line_comments_exp, config.block_end_definition
            )
            .as_str(),
        )
        .build()
        .context("failed to build a single line end block regex")?;

        Ok((single_line_start_block_regex, single_line_end_block_regex))
    }
}

impl BlocksParser for BlocksRegexParser {
    fn parse(self, contents: &str, config: &ParserConfig) -> anyhow::Result<Vec<Block>> {
        let (single_line_start_block_regex, single_line_end_block_regex) =
            Self::single_line_comment_start_end_regex(config)?;
        let mut blocks: Vec<Block> = Vec::new();
        let mut partial_blocks: Vec<PartialBlock> = Vec::new();
        for (line_number, line) in contents
            .lines()
            .enumerate()
            .map(|(line_no, line)| (line_no as u32 + 1, line))
        {
            if let Some(cap) = single_line_start_block_regex.captures(line) {
                let name = cap.name("name").map(|m| m.as_str().to_string());
                // TODO: handle "affects"
                let partial_block = PartialBlock {
                    name,
                    starts_at: line_number,
                    ends_at: None,
                    affects: Default::default(),
                    children: Default::default(),
                };
                partial_blocks.push(partial_block);
                continue;
            }
            if let Some(cap) = single_line_end_block_regex.captures(line) {
                let name = cap.name("name").map(|m| m.as_str().to_string());
                let mut partial_block = partial_blocks.pop().with_context(|| {
                    format!(
                        "Line: {}, Unexpected end of block {:?}",
                        line_number,
                        name.clone().unwrap_or(UNNAMED_BLOCK_LABEL.into())
                    )
                })?;
                if partial_block.name != name {
                    return Err(anyhow::anyhow!(
                        "Line: {}, Unexpected end of block name: {:?}, \
                        expected: {:?} defined at line {}",
                        line_number,
                        name.unwrap_or(UNNAMED_BLOCK_LABEL.into()),
                        partial_block.name.unwrap_or(UNNAMED_BLOCK_LABEL.into()),
                        partial_block.starts_at
                    ));
                }
                partial_block.ends_at = Some(line_number);
                if let Some(parent) = partial_blocks.last_mut() {
                    parent.children.push(partial_block.into());
                } else {
                    blocks.push(partial_block.into());
                }
            }
        }
        if let Some(block) = partial_blocks.pop() {
            return Err(anyhow::anyhow!(
                "Line: {}, block {:?} is not closed",
                block.starts_at,
                block.name.unwrap_or(UNNAMED_BLOCK_LABEL.into())
            ));
        }

        Ok(blocks)
    }
}

struct PartialBlock {
    name: Option<String>,
    starts_at: u32,
    ends_at: Option<u32>,
    affects: Vec<(Option<String>, String)>,
    children: Vec<Block>,
}

impl Into<Block> for PartialBlock {
    fn into(self) -> Block {
        Block {
            name: self.name,
            starts_at: self.starts_at,
            ends_at: self.ends_at.unwrap(),
            affects: self.affects,
            children: self.children,
        }
    }
}

const UNNAMED_BLOCK_LABEL: &str = "(unnamed)";

#[cfg(test)]
mod tests {
    use super::*;

    fn create_parser() -> impl BlocksParser {
        BlocksRegexParser::new()
    }

    #[test]
    fn no_defined_blocks_returns_empty_blocks() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = r#""
        fn say_hello_world() {
          println!("hello world!");
        }
        "#;
        let config = ParserConfig {
            block_start_definition: "@block",
            block_end_definition: "@endblock",
            single_line_comments: vec!["//", "///"],
            multi_line_comments: vec![],
        };
        let blocks = parser.parse(contents, &config)?;
        assert_eq!(blocks, vec![]);
        Ok(())
    }

    #[test]
    fn blocks_defined_in_single_line_comments_parsed_correctly() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = r#""
        // @block foo
        fn say_hello_world() {
          println!("hello world!");
        }
        // @endblock foo

        /// Doc string for the function below.
        /// @block bar
        fn say_hello_world2() {
          println!("hello world 2!");
        }
        /// @endblock bar
        "#;
        let config = ParserConfig {
            block_start_definition: "@block",
            block_end_definition: "@endblock",
            single_line_comments: vec!["//", "///"],
            multi_line_comments: vec![],
        };
        let blocks = parser.parse(contents, &config)?;
        assert_eq!(
            blocks,
            vec![
                Block {
                    name: Some("foo".into()),
                    starts_at: 2,
                    ends_at: 6,
                    affects: vec![],
                    children: vec![]
                },
                Block {
                    name: Some("bar".into()),
                    starts_at: 9,
                    ends_at: 13,
                    affects: vec![],
                    children: vec![]
                }
            ]
        );
        Ok(())
    }

    #[test]
    fn nested_blocks_parsed_correctly() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = r#""
        // @block foo
        fn say_hello_world() {
          println!("hello world!");
        }

            // @block bar
            fn say_hello_world_bar() {
              println!("hello world bar!");
            }
                // @block bar-bar
                fn say_hello_world_bar_bar() {
                  println!("hello world bar bar!");
                }
                // @endblock bar-bar

            // @endblock bar

            // @block buzz
            fn say_hello_world_buzz() {
              println!("hello world buzz!");
            }
            // @endblock buzz

        // @endblock foo
        // @block fizz
        // @endblock fizz
        "#;
        let config = ParserConfig {
            block_start_definition: "@block",
            block_end_definition: "@endblock",
            single_line_comments: vec!["//", "///"],
            multi_line_comments: vec![],
        };
        let blocks = parser.parse(contents, &config)?;
        assert_eq!(
            blocks,
            vec![
                Block {
                    name: Some("foo".into()),
                    starts_at: 2,
                    ends_at: 25,
                    affects: vec![],
                    children: vec![
                        Block {
                            name: Some("bar".into()),
                            starts_at: 7,
                            ends_at: 17,
                            affects: vec![],
                            children: vec![Block {
                                name: Some("bar-bar".into()),
                                starts_at: 11,
                                ends_at: 15,
                                affects: vec![],
                                children: vec![]
                            }]
                        },
                        Block {
                            name: Some("buzz".into()),
                            starts_at: 19,
                            ends_at: 23,
                            affects: vec![],
                            children: vec![]
                        }
                    ]
                },
                Block {
                    name: Some("fizz".into()),
                    starts_at: 26,
                    ends_at: 27,
                    affects: vec![],
                    children: vec![]
                }
            ]
        );
        Ok(())
    }

    #[test]
    fn unclosed_block_returns_error() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = r#""
        // @block foo
        fn say_hello_world() {
          println!("hello world!");
        }
        "#;
        let config = ParserConfig {
            block_start_definition: "@block",
            block_end_definition: "@endblock",
            single_line_comments: vec!["//", "///"],
            multi_line_comments: vec![],
        };
        let error_message = parser.parse(contents, &config).unwrap_err().to_string();
        assert_eq!(error_message, "Line: 2, block \"foo\" is not closed");
        Ok(())
    }

    #[test]
    fn unclosed_nested_block_returns_error() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = r#""
        // @block foo
        fn say_hello_world() {
          println!("hello world!");
        }
            // @block bar
            fn say_hello_world_bar() {
            }
            
        // @endblock foo
        "#;
        let config = ParserConfig {
            block_start_definition: "@block",
            block_end_definition: "@endblock",
            single_line_comments: vec!["//", "///"],
            multi_line_comments: vec![],
        };
        let error_message = parser.parse(contents, &config).unwrap_err().to_string();
        assert_eq!(
            error_message,
            "Line: 10, Unexpected end of block name: \"foo\", expected: \"bar\" defined at line 6"
        );
        Ok(())
    }

    #[test]
    fn incorrect_endblock_name_returns_error() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = r#""
        fn say_hello_world() {
          println!("hello world!");
        }
        // @endblock foo
        "#;
        let config = ParserConfig {
            block_start_definition: "@block",
            block_end_definition: "@endblock",
            single_line_comments: vec!["//", "///"],
            multi_line_comments: vec![],
        };
        let error_message = parser.parse(contents, &config).unwrap_err().to_string();
        assert_eq!(error_message, "Line: 5, Unexpected end of block \"foo\"");
        Ok(())
    }

    #[test]
    fn unnamed_blocks_parsed_correctly() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = r#""
        // @block
        fn say_hello_world() {
          println!("hello world!");
        }
        // @endblock
        
        // @block
        fn say_hello_world2() {
          println!("hello world!");
        }
        // @endblock
        "#;
        let config = ParserConfig {
            block_start_definition: "@block",
            block_end_definition: "@endblock",
            single_line_comments: vec!["//", "///"],
            multi_line_comments: vec![],
        };
        let blocks = parser.parse(contents, &config)?;
        assert_eq!(
            blocks,
            vec![
                Block {
                    name: None,
                    starts_at: 2,
                    ends_at: 6,
                    affects: vec![],
                    children: vec![]
                },
                Block {
                    name: None,
                    starts_at: 8,
                    ends_at: 12,
                    affects: vec![],
                    children: vec![]
                }
            ]
        );
        Ok(())
    }

    #[test]
    fn unnamed_nested_blocks_parsed_correctly() -> anyhow::Result<()> {
        let parser = create_parser();
        let contents = r#""
        // @block
        fn say_hello_world() {
          println!("hello world!");
        }
        // @block
        fn say_hello_world2() {
          println!("hello world!");
        }
        // @block foo
        fn say_hello_world2() {
          println!("hello world!");
        }
        // @endblock foo
        // @endblock
        // @endblock
        "#;
        let config = ParserConfig {
            block_start_definition: "@block",
            block_end_definition: "@endblock",
            single_line_comments: vec!["//", "///"],
            multi_line_comments: vec![],
        };
        let blocks = parser.parse(contents, &config)?;
        assert_eq!(
            blocks,
            vec![Block {
                name: None,
                starts_at: 2,
                ends_at: 16,
                affects: vec![],
                children: vec![Block {
                    name: None,
                    starts_at: 6,
                    ends_at: 15,
                    affects: vec![],
                    children: vec![Block {
                        name: Some("foo".into()),
                        starts_at: 10,
                        ends_at: 14,
                        affects: vec![],
                        children: vec![]
                    }]
                }]
            },]
        );
        Ok(())
    }
}
