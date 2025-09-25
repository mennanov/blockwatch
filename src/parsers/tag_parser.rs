use anyhow::{Context, anyhow};
use quick_xml::events::Event;
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

/// Parses the `block` tags using the `quick-xml` crate.
///
/// Deprecated and unused: `TreeSitterXmlBlockTagParser` is used instead as it is superior.
#[allow(dead_code)]
pub(crate) struct QuickXmlBlockTagParser<'a> {
    reader: quick_xml::Reader<&'a [u8]>,
}

impl<'a> QuickXmlBlockTagParser<'a> {
    #[allow(dead_code)]
    pub(crate) fn new(source: &'a str) -> Self {
        let mut reader = quick_xml::Reader::from_str(source);
        let config = reader.config_mut();
        config.allow_dangling_amp = true;
        config.allow_unmatched_ends = true;
        config.check_end_names = false;
        Self { reader }
    }
}

impl BlockTagParser for QuickXmlBlockTagParser<'_> {
    fn next(&mut self) -> anyhow::Result<Option<BlockTag>> {
        loop {
            let event = self.reader.read_event()?;
            match event {
                Event::Start(start) => {
                    if start.name().as_ref() != b"block" {
                        continue;
                    }
                    let reader_position = self.reader.buffer_position() as usize;
                    let attributes = start
                        .attributes()
                        .map(|attr| {
                            attr.context("Failed to parse attribute").and_then(|attr| {
                                Ok((
                                    String::from_utf8(attr.key.as_ref().into())?,
                                    attr.unescape_value()?.into(),
                                ))
                            })
                        })
                        .collect::<anyhow::Result<HashMap<_, _>>>()?;
                    return Ok(Some(BlockTag::Start {
                        start_position: reader_position - start.len(),
                        end_position: reader_position,
                        attributes,
                    }));
                }
                Event::End(end) => {
                    if end.name().as_ref() != b"block" {
                        continue;
                    }
                    let reader_position = self.reader.buffer_position() as usize;
                    return Ok(Some(BlockTag::End {
                        start_position: reader_position - end.len(),
                        end_position: reader_position,
                    }));
                }
                Event::Eof => {
                    return Ok(None);
                }
                _ => {}
            }
        }
    }
}

pub(crate) struct TreeSitterXmlBlockTagParser<'source> {
    source: &'source str,
    query: Query,
    tree: tree_sitter::Tree,
    last_searched_byte: usize,
}

impl<'source> TreeSitterXmlBlockTagParser<'source> {
    pub(crate) fn new(source: &'source str) -> Self {
        let xml_language = tree_sitter_xml::LANGUAGE_XML.into();
        let query = Query::new(
            &xml_language,
            r#"
            [
              (STag
                (Name) @tag_name
                (#eq? @tag_name "block")) @start_tag
              (ETag
                (Name) @tag_name
                (#eq? @tag_name "block")) @end_tag
              (ERROR) @error_tag
            ]"#,
        )
        .unwrap();
        let mut parser = Parser::new();
        parser.set_language(&xml_language).unwrap();
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
            if child.kind() == "Attribute" {
                if child.child_count() != 3 {
                    return Err(anyhow!(
                        "Invalid attributes: {}",
                        child
                            .utf8_text(self.source.as_bytes())
                            .context("Failed to read attributes")?
                    ));
                }
                // Attribute: child(0) = Name, child(1) = _Eq, child(2) = AttValue
                let name_node = child
                    .child(0)
                    .context("Failed to extract attribute name: no Name node found")?;
                let attr_name = name_node
                    .utf8_text(self.source.as_bytes())
                    .context("Failed to extract attribute name: invalid utf8")?;

                let value = if let Some(att_value) = child.child(2) {
                    self.extract_attribute_value(&att_value)?
                } else {
                    "".to_string()
                };

                if !attr_name.is_empty()
                    && attributes.insert(attr_name.to_string(), value).is_some()
                {
                    return Err(anyhow!("Duplicate attribute: {attr_name}"));
                }
            } else if child.is_error() {
                let attr_name = child
                    .utf8_text(self.source.as_bytes())
                    .context("Failed to extract attribute name from ERROR node")?;
                return Err(anyhow!("Invalid attribute: {attr_name}"));
            }
        }

        Ok(attributes)
    }

    fn extract_attribute_value(&self, node: &Node) -> anyhow::Result<String> {
        let raw = node
            .utf8_text(self.source.as_bytes())
            .context("Failed to extract attribute value")?;
        // Remove surrounding single/double quotes if present
        let unquoted = if raw.len() >= 2 {
            let first = raw.as_bytes()[0];
            let last = raw.as_bytes()[raw.len() - 1];
            if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
                &raw[1..raw.len() - 1]
            } else {
                raw
            }
        } else {
            raw
        };
        // Unescape common XML entities like &quot;
        Ok(html_escape::decode_html_entities(unquoted).into())
    }
}

impl<'source> BlockTagParser for TreeSitterXmlBlockTagParser<'source> {
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
