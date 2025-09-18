use anyhow::Context;
use quick_xml::events::Event;
use std::collections::HashMap;

/// Parses block tags from the concatenated comment string.
///
/// The input string is implicitly bound to the implementing type when it's created.
pub(crate) trait BlockTagParser {
    fn next(&mut self) -> anyhow::Result<Option<BlockTag>>;
}

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
pub(crate) struct QuickXmlBlockTagParser<'a> {
    reader: quick_xml::Reader<&'a [u8]>,
}

impl<'a> QuickXmlBlockTagParser<'a> {
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
