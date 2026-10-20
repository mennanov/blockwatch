//! The output of the `explain` subcommand.

use crate::blocks::{BlockListing, ContentListing};
use crate::repo_path::RepoPath;
use crate::settings::Settings;
use crate::validators::{ValidationContext, Validator};
use serde::Serialize;
use std::collections::BTreeSet;
use strum::IntoEnumIterator;

/// What `explain` writes to stdout: one block, the content its validators read, and how the
/// settings treat it.
///
/// The fields are declared in alphabetical order, so the keys come out sorted.
#[derive(Serialize, Debug)]
pub(crate) struct Explanation<'a> {
    /// The block, as `list` writes it.
    block: BlockListing<'a>,
    content: ContentListing<'a>,
    /// The validators that the block's attributes ask for, but the settings turn off.
    disabled: BTreeSet<Validator>,
    file: &'a RepoPath,
}

impl<'a> Explanation<'a> {
    /// Describes the one block that `context` keeps, as `settings` treat it.
    ///
    /// # Panics
    /// Panics if `context` doesn't keep exactly one block.
    pub(crate) fn new(context: &'a ValidationContext, settings: &Settings) -> Self {
        assert_eq!(context.blocks().len(), 1, "the context keeps one file");
        let (file, file_blocks) = context.blocks().iter().next().expect("one file");
        let [block_with_context] = file_blocks.blocks_with_context.as_slice() else {
            panic!("the context keeps one block");
        };
        let block = &block_with_context.block;
        Self {
            block: BlockListing::from(block_with_context),
            content: block.content.listing(&file_blocks.file_content),
            disabled: Validator::iter()
                .filter(|validator| {
                    block.attributes.contains_key(validator.as_str())
                        && !settings.validator_selection().includes(*validator)
                })
                .collect(),
            file,
        }
    }
}
