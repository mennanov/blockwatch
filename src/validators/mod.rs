//! The rules enforced on blocks (`affects`, `keep-sorted`, …) and the machinery that runs them.

mod affects;
mod check_ai;
mod check_lua;
mod context;
mod keep_sorted;
mod keep_unique;
mod line_count;
mod line_pattern;
mod log;
mod pattern_values;
mod run;
mod same_as;
mod targets;
mod validator;
mod violation;
mod violation_address;

pub(crate) use context::{BlockSelection, ValidationContext};
pub(crate) use log::{ValidationLog, ValidationReport, ValidatorTrace};
pub(crate) use run::{
    ValidatorSelection, create_validator, detect_validators, diff_gated_block_count, run,
};
pub(crate) use validator::Validator;
pub(crate) use violation::{SimpleDiagnostic, Violation, ViolationRange};
pub(crate) use violation_address::ViolationAddress;

use async_trait::async_trait;
use pattern_values::{PatternContent, block_content_for_pattern, line_value};
use std::sync::Arc;
use targets::{TargetFiles, TargetResult};

/// Validates the given `Context` and returns a list of the violations grouped by filename.
#[async_trait]
pub(crate) trait AsyncValidator: Send + Sync {
    async fn validate(&self, context: Arc<ValidationContext>) -> anyhow::Result<ValidationReport>;
}

/// The same contract as [`AsyncValidator`] for validators that need no I/O beyond the filesystem,
/// so the common case runs on plain threads and no Tokio runtime has to be started if no async
/// validators are involved.
pub(crate) trait SyncValidator: Send + Sync {
    fn validate(&self, context: Arc<ValidationContext>) -> anyhow::Result<ValidationReport>;
}

/// A sync or async validator instance.
pub(crate) enum TypedValidator {
    Sync(Box<dyn SyncValidator>),
    Async(Box<dyn AsyncValidator>),
}

#[cfg(test)]
pub(crate) mod test_utils {
    use crate::blocks::FileBlocks;
    use crate::blocks::VirtualBlock;
    use crate::diff::test_utils::diff_changes;
    use crate::diff::{LineChange, LineChangeKind};
    use crate::fs::test_utils::{FakeFileSystem, path_filter_all};
    use crate::languages::LanguageParsers;
    use crate::repo_path::RepoPath;
    use crate::scan::{Scan, ScanMode};
    use crate::validators::{ValidationContext, ValidationReport};
    use std::collections::HashMap;
    use std::sync::Arc;

    /// Creates a [`ValidationContext`] for the given `file_name` with `contents` with all lines
    /// modified.
    pub(crate) fn validation_context(file_name: &str, contents: &str) -> Arc<ValidationContext> {
        let line_changes: Vec<LineChange> = contents
            .lines()
            .enumerate()
            .map(|(line, _)| LineChange {
                line: line + 1,
                kind: LineChangeKind::Added,
            })
            .collect();
        build_validation_context(file_name, contents, line_changes)
    }

    /// Builds a [`ValidationContext`] for `file_name` and `contents`, returning any parse error.
    pub(crate) fn try_validation_context(
        file_name: &str,
        contents: &str,
    ) -> anyhow::Result<Arc<ValidationContext>> {
        try_build_validation_context(file_name, contents, Vec::new())
    }

    /// Creates a [`ValidationContext`] for the given `file_name` with `contents` and specified
    /// `line_changes`, rooted at the current directory.
    pub(crate) fn validation_context_with_changes(
        file_name: &str,
        contents: &str,
        line_changes: Vec<LineChange>,
    ) -> Arc<ValidationContext> {
        build_validation_context(file_name, contents, line_changes)
    }

    fn build_validation_context(
        file_name: &str,
        contents: &str,
        line_changes: Vec<LineChange>,
    ) -> Arc<ValidationContext> {
        try_build_validation_context(file_name, contents, line_changes).unwrap()
    }

    fn try_build_validation_context(
        file_name: &str,
        contents: &str,
        line_changes: Vec<LineChange>,
    ) -> anyhow::Result<Arc<ValidationContext>> {
        let file_system = FakeFileSystem::new(HashMap::from([(
            file_name.to_string(),
            contents.to_string(),
        )]));
        let line_changes_by_file =
            diff_changes([(RepoPath::from_reference(file_name)?, line_changes)]);
        let parsers = LanguageParsers::new();
        Ok(Arc::new(ValidationContext::new(
            Scan::new(&file_system, &path_filter_all(), &parsers, &[])
                .parse_blocks(&line_changes_by_file, ScanMode::OnlyChanged)?
                .blocks,
            parsers,
            line_changes_by_file,
            Vec::new(),
        )))
    }

    /// The start line of every block a validator reported checking, in the order it checked them.
    pub(super) fn checked_lines(report: &ValidationReport) -> Vec<usize> {
        report
            .checked_blocks
            .iter()
            .map(|(_, key)| crate::blocks::test_utils::block_key_start_line(key))
            .collect()
    }

    /// How many violations a validator reported, across every file.
    pub(super) fn violation_count(report: &ValidationReport) -> usize {
        report.violations.values().map(Vec::len).sum()
    }

    /// The JSON of the one trace in `report`.
    ///
    /// # Panics
    /// Panics if `report` doesn't hold exactly one trace.
    pub(super) fn trace_json(report: &ValidationReport) -> serde_json::Value {
        let [(_, _, trace)] = report.traces.as_slice() else {
            panic!("expected one trace, got {}", report.traces.len());
        };
        serde_json::to_value(trace).expect("a trace serializes")
    }

    /// `context` with traces requested.
    pub(super) fn with_traces(context: Arc<ValidationContext>) -> Arc<ValidationContext> {
        let mut context = Arc::into_inner(context).expect("the context is not shared");
        context.request_traces();
        Arc::new(context)
    }

    /// `context` with `virtual_blocks` as the config file's virtual blocks.
    pub(super) fn with_virtual_blocks(
        context: Arc<ValidationContext>,
        virtual_blocks: Vec<VirtualBlock>,
    ) -> Arc<ValidationContext> {
        let context = Arc::into_inner(context).expect("the context is not shared");
        Arc::new(ValidationContext::new(
            context.blocks,
            context.parsers,
            context.line_changes,
            virtual_blocks,
        ))
    }

    /// Combines several single-file contexts into one, so a test can exercise a validator that
    /// resolves references across files.
    pub(crate) fn merge_validation_contexts(
        contexts: Vec<Arc<ValidationContext>>,
    ) -> Arc<ValidationContext> {
        let parsers = LanguageParsers::new();
        let mut merged_modified_blocks = HashMap::new();
        let mut merged_line_changes = HashMap::new();
        for context in contexts {
            for (file_path, file_blocks) in &context.blocks {
                merged_modified_blocks
                    .entry(file_path.clone())
                    .or_insert_with(|| FileBlocks {
                        file_content: file_blocks.file_content.clone(),
                        blocks_with_context: vec![],
                    })
                    .blocks_with_context
                    .extend(file_blocks.blocks_with_context.clone());
            }
            for file_path in context.line_changes.files() {
                merged_line_changes.insert(
                    file_path.clone(),
                    context.line_changes.for_file(file_path).clone(),
                );
            }
        }
        Arc::new(ValidationContext::new(
            merged_modified_blocks,
            parsers,
            merged_line_changes.into_iter().collect(),
            Vec::new(),
        ))
    }
}
