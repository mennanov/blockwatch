use anyhow::{Context, anyhow};
use std::collections::HashMap;
use tree_sitter::{Node, Parser, Query, QueryCursor, StreamingIterator};

/// Parses block tags from the concatenated comment string.
///
/// The input string is implicitly bound to the implementing type when it's created.
pub(crate) trait BlockTagParser {
    fn next(&mut self) -> anyhow::Result<Option<BlockTag>>;
}

#[derive(Debug)]
pub(crate) enum BlockTag {
    Start {
        start_position: usize,
        end_position: usize,
        attributes: HashMap<String, String>,
    },
    End {
        start_position: usize,
        end_position: usize,
    },
}

pub(crate) struct TreeSitterHtmlBlockTagParser<'source> {
    source: &'source str,
    query: Query,
    tree: tree_sitter::Tree,
    last_searched_byte: usize,
}

impl<'source> TreeSitterHtmlBlockTagParser<'source> {
    pub(crate) fn new(source: &'source str) -> Self {
        let html_language = tree_sitter_html::LANGUAGE.into();
        let query = Query::new(
            &html_language,
            r#"
            [
              (start_tag
                (tag_name) @tag_name
                (#eq? @tag_name "block")) @start_tag
              (end_tag
                (tag_name) @tag_name
                (#eq? @tag_name "block")) @end_tag
              (ERROR) @error_tag
            ]"#,
        )
        .unwrap();
        let mut parser = Parser::new();
        parser.set_language(&html_language).unwrap();
        let tree = parser.parse(source, None).unwrap();
        Self {
            source,
            query,
            tree,
            last_searched_byte: 0,
        }
    }

    fn parse_attributes(&self, node: &Node) -> anyhow::Result<HashMap<String, String>> {
        let mut attributes = HashMap::new();
        let mut cursor = node.walk();

        for child in node.children(&mut cursor) {
            if child.is_error() {
                let node_text = child
                    .utf8_text(self.source.as_bytes())
                    .context("Failed to read error node text")?;
                return Err(anyhow!("Invalid token inside tag: {node_text}"));
            }
            if child.kind() == "attribute" {
                // 1. Get attribute name (named child 0)
                let name_node = child
                    .named_child(0)
                    .context("Failed to find attribute name node")?;

                let attr_name = name_node
                    .utf8_text(self.source.as_bytes())
                    .context("Failed to extract attribute name: invalid utf8")?;

                // 2. Get attribute value (named child 1, optional)
                let value_node_option = child.named_child(1);

                let value = match value_node_option {
                    Some(value_node) => self.extract_attribute_value(&value_node)?,
                    None => "".to_string(), // Handle attributes with no values.
                };

                if !attr_name.is_empty()
                    && attributes.insert(attr_name.to_string(), value).is_some()
                {
                    return Err(anyhow!("Duplicate attribute: {attr_name}"));
                }
            }
        }

        Ok(attributes)
    }

    fn extract_attribute_value(&self, node: &Node) -> anyhow::Result<String> {
        let value = match node.kind() {
            "attribute_value" => {
                // Unquoted value
                node.utf8_text(self.source.as_bytes())
                    .context("Failed to extract attribute value content")?
            }
            "quoted_attribute_value" => {
                // For quoted attribute values, the actual content is the named child (attribute_value).
                if let Some(content_node) = node.named_child(0) {
                    content_node
                        .utf8_text(self.source.as_bytes())
                        .context("Failed to extract quoted attribute value content")?
                } else {
                    // Empty string attribute value, e.g. foo="" or foo=''
                    ""
                }
            }
            kind => {
                return Err(anyhow!("Unexpected node kind for attribute value: {kind}"));
            }
        };

        // Unescape common HTML/XML entities
        Ok(html_escape::decode_html_entities(&value).into())
    }
}

impl<'source> BlockTagParser for TreeSitterHtmlBlockTagParser<'source> {
    fn next(&mut self) -> anyhow::Result<Option<BlockTag>> {
        let mut cursor = QueryCursor::new();
        cursor.set_byte_range(self.last_searched_byte..self.source.len());
        let root = self.tree.root_node();
        let mut matches = cursor.matches(&self.query, root, self.source.as_bytes());

        while let Some(query_match) = matches.next() {
            for capture in query_match.captures {
                let node = capture.node;
                let start_position = node.start_byte();
                let end_position = node.end_byte();
                let capture_name = self.query.capture_names()[capture.index as usize];
                match capture_name {
                    "start_tag" => {
                        self.last_searched_byte = end_position;
                        return Ok(Some(BlockTag::Start {
                            start_position,
                            end_position,
                            attributes: self.parse_attributes(&node)?,
                        }));
                    }
                    "end_tag" => {
                        self.last_searched_byte = end_position;
                        return Ok(Some(BlockTag::End {
                            start_position,
                            end_position,
                        }));
                    }
                    "error_tag" => {
                        let node_text = node
                            .utf8_text(self.source.as_bytes())
                            .context("Failed to read error tag text")?;
                        // Handle unmatched end block tags.
                        if node_text.trim() == "</block>" {
                            self.last_searched_byte = end_position;
                            return Ok(Some(BlockTag::End {
                                start_position,
                                end_position,
                            }));
                        }
                    }
                    _ => {}
                }
            }
        }

        Ok(None)
    }
}
