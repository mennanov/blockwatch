//! The blocks of a run, and what the validators need to read more files.

use crate::blocks::VirtualBlock;
use crate::blocks::{BlockAddress, FileBlocks};
use crate::diff::DiffChanges;
use crate::fs::FileSystem;
use crate::languages::LanguageParsers;
use crate::repo_path::RepoPath;
use crate::scan::{self, ScanMode};
use anyhow::bail;
use std::collections::HashMap;

/// Everything the validators are given to work with, shared read-only across all of them.
///
/// Built once per run and handed out as an `Arc`, because validators run concurrently on separate
/// threads and each needs the whole picture: a rule such as `affects` has to see blocks in files
/// other than the one it started from.
pub(crate) struct ValidationContext {
    /// Blocks with their corresponding source file contents grouped by filename.
    pub(super) blocks: HashMap<RepoPath, FileBlocks>,
    /// Language parsers per file type, used by validators to parse referenced source files.
    pub(super) parsers: LanguageParsers,
    /// Every line change from the diff (if any).
    pub(super) line_changes: DiffChanges,
    /// The declarations of every virtual block in the config file, in its order. Unlike `blocks`,
    /// they are not resolved, and they cover the files the run did not keep too.
    virtual_blocks: Vec<VirtualBlock>,
    /// Whether the validators describe what they saw, in [`ValidationReport`]'s traces.
    ///
    /// [`ValidationReport`]: super::ValidationReport
    pub(super) traces_requested: bool,
}

#[cfg(test)]
impl std::fmt::Debug for ValidationContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ValidationContext").finish_non_exhaustive()
    }
}

/// Which blocks a run checks. Each address is `FILE:BLOCK_NAME`.
#[derive(Debug, PartialEq)]
pub(crate) enum BlockSelection {
    /// Every block.
    All,
    /// Only the blocks that an address selects.
    Only(Vec<BlockAddress>),
    /// Every block except the ones that an address selects.
    Skip(Vec<BlockAddress>),
}

impl ValidationContext {
    /// Creates a new validation context with modified blocks grouped by filename.
    pub(crate) fn new(
        blocks: HashMap<RepoPath, FileBlocks>,
        parsers: LanguageParsers,
        line_changes: DiffChanges,
        virtual_blocks: Vec<VirtualBlock>,
    ) -> Self {
        Self {
            blocks,
            parsers,
            line_changes,
            virtual_blocks,
            traces_requested: false,
        }
    }

    /// Asks the validators to describe what they saw in each block.
    pub(crate) fn request_traces(&mut self) {
        self.traces_requested = true;
    }

    /// The blocks the run validates, with the text of their files, by file.
    pub(crate) fn blocks(&self) -> &HashMap<RepoPath, FileBlocks> {
        &self.blocks
    }

    /// Keeps only the blocks that `selection` lets through.
    ///
    /// Returns an error if an address matches no block in its file on disk, counting the file's
    /// virtual blocks. The check reads each address's file, so a block the run did not keep still
    /// counts as a match.
    pub(crate) fn apply_block_selection(
        &mut self,
        selection: &BlockSelection,
        file_system: &impl FileSystem,
    ) -> anyhow::Result<()> {
        let (addresses, keep_selected) = match selection {
            BlockSelection::All => return Ok(()),
            BlockSelection::Only(addresses) => (addresses, true),
            BlockSelection::Skip(addresses) => (addresses, false),
        };
        for address in addresses {
            if !self.has_block(address, file_system)? {
                bail!("no block matches \"{address}\"");
            }
        }
        for (file, file_blocks) in &mut self.blocks {
            file_blocks.blocks_with_context.retain(|block| {
                let selected = block
                    .block
                    .address(file)
                    .is_some_and(|address| addresses.contains(&address));
                selected == keep_selected
            });
        }
        Ok(())
    }

    /// Whether the file of `address` on disk has the block it selects, counting the file's
    /// virtual blocks. Returns `false` for a missing file or one with neither a parser nor virtual
    /// blocks.
    fn has_block(
        &self,
        address: &BlockAddress,
        file_system: &impl FileSystem,
    ) -> anyhow::Result<bool> {
        let file = address.file();
        if !file_system.exists(file.as_path()) {
            return Ok(false);
        }
        Ok(match self.parse_file(file_system, file)? {
            Some(file_blocks) => file_blocks
                .blocks_with_context
                .iter()
                .any(|block| block.block.address(file).as_ref() == Some(address)),
            None => false,
        })
    }

    /// Reads and parses `file_path` the way the run parses a file it checks: its tags and its
    /// virtual blocks. Every block is kept, and the diff's changes to the file mark the ones it
    /// touched. Returns `None` if the file's format is unsupported and it has no virtual blocks.
    ///
    /// # Errors
    /// Returns an error if the file can't be read, a tag is malformed, a name is used twice, or a
    /// virtual block's target does not resolve.
    pub(super) fn parse_file(
        &self,
        file_system: &impl FileSystem,
        file_path: &RepoPath,
    ) -> anyhow::Result<Option<FileBlocks>> {
        let virtual_blocks: Vec<&VirtualBlock> = self
            .virtual_blocks
            .iter()
            .filter(|virtual_block| virtual_block.file == *file_path)
            .collect();
        scan::parse_file(
            file_system,
            file_path.as_path(),
            self.line_changes.for_file(file_path),
            ScanMode::All,
            &self.parsers,
            &virtual_blocks,
        )
    }

    /// The changes the diff reported, by file.
    pub(super) fn line_changes(&self) -> &DiffChanges {
        &self.line_changes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks::test_utils::block_addresses;
    use crate::blocks::test_utils::virtual_block;
    use crate::fs::test_utils::FakeFileSystem;
    use crate::validators::test_utils::{
        merge_validation_contexts, validation_context, with_virtual_blocks,
    };
    use std::sync::Arc;

    /// The files of the block selection tests, as paths and contents. `a.py` has two named blocks
    /// and an unnamed one. `b.py` has a block with the same name as one in `a.py`.
    const SELECTION_FILES: &[(&str, &str)] = &[
        (
            "a.py",
            "# <block name=\"fruits\">\na\n# </block>\n\
             # <block name=\"vegetables\">\nb\n# </block>\n\
             # <block>\nc\n# </block>\n",
        ),
        ("b.py", "# <block name=\"fruits\">\na\n# </block>\n"),
        ("notes.txt", "notes\n"),
    ];

    /// A context that keeps every block of `files`, which must be in [`SELECTION_FILES`], and a
    /// file system with every file of [`SELECTION_FILES`].
    fn selection_context(files: &[&str]) -> (ValidationContext, FakeFileSystem) {
        let contexts = SELECTION_FILES
            .iter()
            .filter(|(file, _)| files.contains(file))
            .map(|(file, contents)| validation_context(file, contents))
            .collect();
        let context = Arc::into_inner(merge_validation_contexts(contexts))
            .expect("the context is not shared");
        let file_system = FakeFileSystem::new(
            SELECTION_FILES
                .iter()
                .map(|(file, contents)| (file.to_string(), contents.to_string()))
                .collect(),
        );
        (context, file_system)
    }

    /// The blocks `context` keeps, as `FILE:NAME`, sorted. An unnamed block shows as
    /// `FILE:(unnamed)`.
    fn kept_blocks(context: &ValidationContext) -> Vec<String> {
        let mut blocks: Vec<String> = context
            .blocks
            .iter()
            .flat_map(|(file, file_blocks)| {
                file_blocks
                    .blocks_with_context
                    .iter()
                    .map(move |block| format!("{file}:{}", block.block.name_display()))
            })
            .collect();
        blocks.sort();
        blocks
    }

    #[test]
    fn only_selection_keeps_just_the_listed_blocks() -> anyhow::Result<()> {
        let (mut context, file_system) = selection_context(&["a.py", "b.py"]);
        let selection = BlockSelection::Only(block_addresses(&["a.py:fruits", "a.py:vegetables"]));

        context.apply_block_selection(&selection, &file_system)?;

        assert_eq!(kept_blocks(&context), ["a.py:fruits", "a.py:vegetables"]);
        Ok(())
    }

    #[test]
    fn skip_selection_keeps_every_block_but_the_listed_ones() -> anyhow::Result<()> {
        let (mut context, file_system) = selection_context(&["a.py", "b.py"]);
        let selection = BlockSelection::Skip(block_addresses(&["a.py:fruits"]));

        context.apply_block_selection(&selection, &file_system)?;

        assert_eq!(
            kept_blocks(&context),
            ["a.py:(unnamed)", "a.py:vegetables", "b.py:fruits"]
        );
        Ok(())
    }

    #[test]
    fn address_that_matches_no_block_is_an_error() {
        for address in [
            "a.py:missing",      // no block with that name
            "missing.py:fruits", // no such file
            "notes.txt:fruits",  // a file without a parser
        ] {
            for selection in [
                BlockSelection::Only(block_addresses(&[address])),
                BlockSelection::Skip(block_addresses(&[address])),
            ] {
                let (mut context, file_system) = selection_context(&["a.py"]);

                let error = context
                    .apply_block_selection(&selection, &file_system)
                    .expect_err("the address matches no block");

                assert_eq!(
                    format!("{error:#}"),
                    format!("no block matches \"{address}\"")
                );
            }
        }
    }

    #[test]
    fn address_of_a_block_the_run_did_not_keep_is_not_an_error() -> anyhow::Result<()> {
        // `b.py` is on disk, but the run kept only the blocks of `a.py`.
        let (mut context, file_system) = selection_context(&["a.py"]);
        let selection = BlockSelection::Only(block_addresses(&["b.py:fruits"]));

        context.apply_block_selection(&selection, &file_system)?;

        assert!(kept_blocks(&context).is_empty());
        Ok(())
    }

    #[test]
    fn address_of_a_virtual_block_is_not_an_error() -> anyhow::Result<()> {
        // The block is declared in the config file, and the run did not keep its file.
        let (context, _) = selection_context(&["a.py"]);
        let context = with_virtual_blocks(
            Arc::new(context),
            vec![virtual_block(
                "package.json#/version",
                &[("name", "version")],
            )?],
        );
        let mut context = Arc::into_inner(context).expect("the context is not shared");
        let file_system = FakeFileSystem::new(HashMap::from([(
            "package.json".to_string(),
            r#"{"version": "1.2.3"}"#.to_string(),
        )]));
        let selection = BlockSelection::Only(block_addresses(&["package.json:version"]));

        context.apply_block_selection(&selection, &file_system)?;

        assert!(kept_blocks(&context).is_empty());
        Ok(())
    }
}
