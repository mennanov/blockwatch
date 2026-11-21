//! What validators report: the violations they found, the blocks they checked, and what they saw
//! in each block.

use super::{Validator, Violation};
use crate::blocks::{Block, BlockKey};
use crate::repo_path::RepoPath;
use anyhow::Context;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// What a single validator found, and which blocks it looked at.
///
/// A block is identified by its [`BlockKey`], because a block may have no name.
#[derive(Debug, Default)]
pub(crate) struct ValidationReport {
    /// Violations grouped by the file they were found in. A file without violations has no entry,
    /// so the map is empty when the validator found nothing.
    pub(super) violations: BTreeMap<RepoPath, Vec<Violation>>,
    /// The key of every block this validator checked.
    pub(super) checked_blocks: Vec<(RepoPath, BlockKey)>,
    /// What the validator saw in each block it checked, as JSON. Empty unless the context asks
    /// for traces.
    ///
    /// JSON, rather than each validator's own trace type, because every validator uses this file,
    /// so this file can't use them.
    pub(super) traces: Vec<(RepoPath, BlockKey, serde_json::Value)>,
}

impl ValidationReport {
    /// Adds a checked block together with the violations found in it.
    pub(crate) fn add_all(&mut self, file: &RepoPath, block: &Block, violations: Vec<Violation>) {
        self.add_checked_block(file, block);
        self.add_violations(file, violations);
    }

    /// Adds `block` in `file` to the blocks this validator checked.
    pub(super) fn add_checked_block(&mut self, file: &RepoPath, block: &Block) {
        self.checked_blocks.push((file.clone(), block.key()));
    }

    /// Adds what the validator saw in `block` in `file`.
    ///
    /// # Errors
    /// Returns an error if `trace` can't be written as JSON, such as a map whose keys are not
    /// strings.
    pub(super) fn add_trace(
        &mut self,
        file: &RepoPath,
        block: &Block,
        trace: &impl Serialize,
    ) -> anyhow::Result<()> {
        let json = serde_json::to_value(trace).context("failed to write a trace as JSON")?;
        self.traces.push((file.clone(), block.key(), json));
        Ok(())
    }

    /// Stores violations found in `file`.
    pub(super) fn add_violations(&mut self, file: &RepoPath, violations: Vec<Violation>) {
        if !violations.is_empty() {
            self.violations
                .entry(file.clone())
                .or_default()
                .extend(violations);
        }
    }
}

/// What a whole run found: which validators checked which blocks, and every violation.
///
/// Checked blocks are keyed by file, then by the [`BlockKey`] of the block, then by the names of
/// the validators that checked it.
#[derive(Debug, Default)]
pub(crate) struct ValidationLog {
    /// Violations grouped by the file they were found in. `BTreeMap` is used so every output lists
    /// the files in the same order on every run.
    pub(crate) violations: BTreeMap<RepoPath, Vec<Violation>>,
    /// Every block that was examined, and by which validators. `BTreeMap` is used so the
    /// `--verbosity` report comes out in a stable order regardless of the validators order.
    pub(crate) checked_blocks: BTreeMap<RepoPath, BTreeMap<BlockKey, BTreeSet<Validator>>>,
    /// What each validator saw in each block, as JSON, by file and block. Empty unless the context
    /// asks for traces.
    pub(crate) traces: BTreeMap<(RepoPath, BlockKey), BTreeMap<Validator, serde_json::Value>>,
}

impl ValidationLog {
    /// Adds one validator's `report` for the corresponding `validator`.
    pub(crate) fn add_validation_report(&mut self, validator: Validator, report: ValidationReport) {
        for (file, key) in report.checked_blocks {
            self.checked_blocks
                .entry(file)
                .or_default()
                .entry(key)
                .or_default()
                .insert(validator);
        }
        for (file, key, trace) in report.traces {
            self.traces
                .entry((file, key))
                .or_default()
                .insert(validator, trace);
        }
        for (file, violations) in report.violations {
            self.violations.entry(file).or_default().extend(violations);
        }
    }

    /// Adds every checked block and violation from `other` to this log.
    pub(super) fn merge(&mut self, other: Self) {
        for (file, blocks) in other.checked_blocks {
            let checked_in_file = self.checked_blocks.entry(file).or_default();
            for (key, validators) in blocks {
                checked_in_file.entry(key).or_default().extend(validators);
            }
        }
        for (block, traces) in other.traces {
            self.traces.entry(block).or_default().extend(traces);
        }
        for (file, violations) in other.violations {
            self.violations.entry(file).or_default().extend(violations);
        }
    }
}
