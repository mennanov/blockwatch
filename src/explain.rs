//! The output of the `explain` subcommand.

use crate::blocks::{BlockListing, ContentListing};
use crate::repo_path::RepoPath;
use crate::settings::Settings;
use crate::validators::{ValidationContext, ValidationLog, Validator, ValidatorTrace};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use strum::IntoEnumIterator;

/// What `explain` writes to stdout: one block, the content its validators read, what they saw, and
/// how the settings treat it.
///
/// The fields are declared in alphabetical order, so the keys come out sorted.
#[derive(Serialize, Debug)]
pub(crate) struct Explanation<'a> {
    // <block name="explanation" affects="docs/cli.md:explain-output-example">
    /// The block, as `list` writes it.
    block: BlockListing<'a>,
    /// What each validator saw in the block, by validator.
    checks: BTreeMap<Validator, &'a ValidatorTrace>,
    content: ContentListing<'a>,
    /// The validators that the block's attributes ask for, but the settings turn off.
    disabled: BTreeSet<Validator>,
    file: &'a RepoPath,
    /// Whether the run was given a diff. Without one, no block counts as changed.
    with_diff: bool,
    // </block>
}

impl<'a> Explanation<'a> {
    /// Describes the one block that `context` keeps, as `settings` treat it, with the traces that
    /// `log` holds for it. `with_diff` is whether the run was given a diff.
    ///
    /// # Panics
    /// Panics if `context` doesn't keep exactly one block.
    pub(crate) fn new(
        context: &'a ValidationContext,
        settings: &Settings,
        log: &'a ValidationLog,
        with_diff: bool,
    ) -> Self {
        assert_eq!(context.blocks().len(), 1, "the context keeps one file");
        let (file, file_blocks) = context.blocks().iter().next().expect("one file");
        let [block_with_context] = file_blocks.blocks_with_context.as_slice() else {
            panic!("the context keeps one block");
        };
        let block = &block_with_context.block;
        Self {
            block: BlockListing::from(block_with_context),
            checks: log
                .traces
                .get(&(file.clone(), block.key()))
                .into_iter()
                .flatten()
                .map(|(validator, trace)| (*validator, trace))
                .collect(),
            content: block.content.listing(&file_blocks.file_content),
            disabled: Validator::iter()
                .filter(|validator| {
                    block.attributes.contains_key(validator.as_str())
                        && !settings.validator_selection().includes(*validator)
                })
                .collect(),
            file,
            with_diff,
        }
    }
}
