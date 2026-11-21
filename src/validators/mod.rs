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
pub(crate) use log::{ValidationLog, ValidationReport};
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
    pub(crate) use super::context::test_utils::{
        merge_validation_contexts, validation_context, validation_context_with_changes,
    };
    pub(super) use super::context::test_utils::{with_traces, with_virtual_blocks};
    use crate::validators::ValidationReport;

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
        trace.clone()
    }
}
