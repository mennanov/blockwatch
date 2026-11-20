//! A block and its content, as `list`, `explain` and the `--verbosity` report write them in JSON.

use crate::attributes::Attributes;
use crate::blocks::{BlockWithContext, Content};
use crate::position::Position;
use crate::repo_path::RepoPath;
use crate::validators::{ValidationContext, Validator};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

// <block name="block-listing" affects="docs/cli.md:list-output-example">
/// A block, as it is written in JSON.
///
/// serde writes the keys in the order the fields are declared, and the output keeps its keys
/// sorted. So declare the fields in alphabetical order.
#[derive(Serialize, Debug)]
pub(crate) struct BlockListing<'a> {
    attributes: &'a Attributes,
    /// The validators that checked the block. `None` leaves the key out.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) checks: Option<BTreeSet<Validator>>,
    column: usize,
    /// The line of the `[[block]]` entry that declares a virtual block. `None` for a block with
    /// tags.
    #[serde(skip_serializing_if = "Option::is_none")]
    config_line: Option<usize>,
    is_content_modified: bool,
    line: usize,
    name: &'a str,
}
// </block>

/// The content of a block, as it is written in JSON: its text, and where the block's file writes
/// it.
#[derive(Serialize, Debug)]
pub(super) struct ContentListing<'a> {
    /// The position of the content's first character.
    start: Position,
    /// The first position after the content.
    end: Position,
    /// The text the validators read. For a virtual block around a quoted or escaped value, it is
    /// the decoded value.
    text: &'a str,
}

impl<'a> ContentListing<'a> {
    /// Describes `content` for JSON. `source` must be the text of the block's file.
    pub(super) fn new(content: &'a Content, source: &'a str) -> Self {
        Self {
            start: content.positions().start,
            end: content.positions().end,
            text: content.text(source),
        }
    }
}

impl<'a> From<&'a BlockWithContext> for BlockListing<'a> {
    /// Describes the block, with no `checks`.
    fn from(block_with_context: &'a BlockWithContext) -> Self {
        let block = &block_with_context.block;
        Self {
            attributes: &block.attributes,
            checks: None,
            column: block.start_tag_position_range.start.character,
            config_line: block.declaration.config_line(),
            is_content_modified: block_with_context.is_content_modified,
            line: block.start_tag_position_range.start.line,
            name: block.name_display(),
        }
    }
}

/// Describes every block of `context`, with the files sorted by path. The blocks of a file are in
/// source order, as [`crate::scan::parse_file`] keeps them.
pub(crate) fn listings(context: &ValidationContext) -> BTreeMap<&RepoPath, Vec<BlockListing<'_>>> {
    context
        .blocks()
        .iter()
        .map(|(path, file_blocks)| {
            let listings = file_blocks
                .blocks_with_context
                .iter()
                .map(BlockListing::from)
                .collect();
            (path, listings)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attributes::AttributeEntry;
    use crate::blocks::{Block, ConfigEntry, Declaration};
    use crate::validators::test_utils::{merge_validation_contexts, validation_context};
    use std::path::PathBuf;

    fn block_with_context(attributes: Attributes, declaration: Declaration) -> BlockWithContext {
        BlockWithContext {
            block: Block {
                attributes,
                start_tag_position_range: Position::new(2, 4)..Position::new(2, 30),
                content: Content::source(0..0, Position::new(3, 1)..Position::new(3, 1)),
                declaration,
            },
            is_start_tag_modified: false,
            is_content_modified: true,
        }
    }

    #[test]
    fn virtual_block_with_checks_is_written_with_its_keys_sorted() -> anyhow::Result<()> {
        let attributes = Attributes::from_entries([
            ("name".into(), AttributeEntry::Text("n".into())),
            ("line-count".into(), AttributeEntry::Text("<=3".into())),
            ("keep-sorted".into(), AttributeEntry::Text("asc".into())),
            (
                "affects".into(),
                AttributeEntry::List(vec!["a.md:b".into(), "c.md".into()]),
            ),
        ])?;
        let block = block_with_context(
            attributes,
            Declaration::ConfigEntry(ConfigEntry {
                file: PathBuf::from("blockwatch.toml"),
                line: 7,
                index: 0,
            }),
        );
        let mut listing = BlockListing::from(&block);
        listing.checks = Some(BTreeSet::from([Validator::KeepSorted]));

        assert_eq!(
            serde_json::to_string(&listing)?,
            r#"{"attributes":{"affects":["a.md:b","c.md"],"keep-sorted":"asc","line-count":"<=3","name":"n"},"checks":["keep-sorted"],"column":4,"config_line":7,"is_content_modified":true,"line":2,"name":"n"}"#
        );
        Ok(())
    }

    #[test]
    fn unnamed_block_with_tags_is_written_without_checks_and_config_line() -> anyhow::Result<()> {
        let block = block_with_context(
            crate::attributes::test_utils::text_attributes(&[("keep-sorted", "asc")]),
            Declaration::Tags,
        );

        assert_eq!(
            serde_json::to_string(&BlockListing::from(&block))?,
            r#"{"attributes":{"keep-sorted":"asc"},"column":4,"is_content_modified":true,"line":2,"name":"(unnamed)"}"#
        );
        Ok(())
    }

    #[test]
    fn listings_returns_correct_listings() -> anyhow::Result<()> {
        let contents = r#"/* <block name="top"> */ let a = "cc"; /* </block> Block on the first line. */
// <block name="first"> Block on the second line.
fn a() {}
// </block>
//     <block name="second"> Block with indent.
fn b() {}
// </block>
/* <block name="bottom"> */ fn c() {} /* </block> Block on the last line. */"#;
        let context = validation_context("example.rs", contents);
        let report = listings(&context);

        assert_eq!(report.len(), 1);
        let listings = &report[&RepoPath::from_reference("example.rs")?];
        assert_eq!(listings.len(), 4);
        assert_eq!(
            serde_json::to_value(listings)?,
            serde_json::json!([
                {
                    "name": "top",
                    "line": 1,
                    "column": 4,
                    "is_content_modified": true,
                    "attributes": {
                        "name": "top"
                    }
                },
                {
                    "name": "first",
                    "line": 2,
                    "column": 4,
                    "is_content_modified": true,
                    "attributes": {
                        "name": "first",
                    }
                },
                {
                    "name": "second",
                    "line": 5,
                    "column": 8,
                    "is_content_modified": true,
                    "attributes": {
                        "name": "second"
                    }
                },
                {
                    "name": "bottom",
                    "line": 8,
                    "column": 4,
                    "is_content_modified": true,
                    "attributes": {
                        "name": "bottom"
                    }
                }
            ])
        );

        Ok(())
    }

    #[test]
    fn listings_lists_files_sorted_by_path() {
        let names = [
            "c.rs", "a.rs", "j.rs", "e.rs", "b.rs", "h.rs", "d.rs", "g.rs", "i.rs", "f.rs",
        ];
        let context = merge_validation_contexts(
            names
                .iter()
                .map(|name| validation_context(name, "// <block>\n// </block>\n"))
                .collect(),
        );

        let paths: Vec<String> = listings(&context)
            .into_keys()
            .map(|path| path.to_string())
            .collect();

        let mut sorted = names.to_vec();
        sorted.sort();
        assert_eq!(paths, sorted);
    }
}
