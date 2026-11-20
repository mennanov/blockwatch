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
mod violation;
mod violation_address;

pub(crate) use context::ValidationContext;
pub(crate) use log::{ValidationLog, ValidationReport, ValidatorTrace};
pub(crate) use run::{detect_validators, diff_gated_block_count, run};
pub(crate) use violation::{SimpleDiagnostic, Violation, ViolationRange};
pub(crate) use violation_address::ViolationAddress;

use crate::attributes::{AttributeName, Attributes};
use crate::fs::FileSystem;
use anyhow::Context;
use async_trait::async_trait;
use bigdecimal::BigDecimal;
use pattern_values::{PatternContent, block_content_for_pattern, line_value};
use serde::Serialize;
use std::sync::Arc;
use strum::IntoEnumIterator;
use strum_macros::{Display, EnumIter, EnumString, IntoStaticStr};
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

/// The registered validators that check blocks.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Display,
    EnumString,
    IntoStaticStr,
    EnumIter,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub(crate) enum Validator {
    /* <block name="validator-registry"
    same-as="README.md:available-validators, docs/validators/README.md:validators-index"
    same-as-format="identifier"
    same-as-pattern='^\s*(?P<value>[A-Za-z]+),'> */
    Affects,
    CheckAi,
    CheckLua,
    KeepSorted,
    KeepUnique,
    LineCount,
    LinePattern,
    SameAs,
    // </block>
}

impl Validator {
    /// A one-line description of the validator.
    pub(crate) fn description(self) -> &'static str {
        match self {
            Validator::Affects => "Requires co-dependent blocks to be updated together.",
            Validator::CheckAi => "Checks a block with AI.",
            Validator::CheckLua => "Checks a block with a custom Lua script.",
            Validator::KeepSorted => "Requires the lines of a block to stay in order.",
            Validator::KeepUnique => "Requires the lines of a block to stay free of duplicates.",
            Validator::LineCount => "Constrains how many lines a block has.",
            Validator::LinePattern => {
                "Checks every line of a block against a required or a forbidden pattern."
            }
            Validator::SameAs => "Requires two places to keep holding the same value.",
        }
    }

    /// Whether this validator only ever fires when a diff is supplied.
    pub(crate) fn needs_diff(self) -> bool {
        match self {
            Validator::Affects => true,
            Validator::CheckAi
            | Validator::CheckLua
            | Validator::KeepSorted
            | Validator::KeepUnique
            | Validator::LineCount
            | Validator::LinePattern
            | Validator::SameAs => false,
        }
    }

    /// The attributes that turn this validator on, such as `line-pattern` and
    /// `line-pattern-forbidden` for `line-pattern`. The validator's own name comes first.
    pub(crate) fn attributes(self) -> &'static [AttributeName] {
        match self {
            Validator::Affects => &[AttributeName::Affects],
            Validator::CheckAi => &[AttributeName::CheckAi],
            Validator::CheckLua => &[AttributeName::CheckLua],
            Validator::KeepSorted => &[AttributeName::KeepSorted],
            Validator::KeepUnique => &[AttributeName::KeepUnique],
            Validator::LineCount => &[AttributeName::LineCount],
            Validator::LinePattern => &[
                AttributeName::LinePattern,
                AttributeName::LinePatternForbidden,
            ],
            Validator::SameAs => &[AttributeName::SameAs, AttributeName::SameAsPaths],
        }
    }

    /// Whether `attributes` has one of the [`attributes`](Self::attributes) that turn this
    /// validator on.
    pub(crate) fn is_turned_on_by(self, attributes: &Attributes) -> bool {
        self.attributes().iter().any(|&name| attributes.has(name))
    }

    /// Creates this validator.
    ///
    /// The `match` has no `_` arm, so a new validator can't be left out. The result says whether
    /// the validator is sync or async, which lets the caller start a Tokio runtime only when an
    /// async validator exists.
    ///
    /// # Errors
    /// Returns an error when the validator can't be set up, such as `check-lua` with an unknown
    /// `BLOCKWATCH_LUA_MODE`.
    pub(crate) fn create<Fs: FileSystem + 'static>(
        self,
        file_system: &Arc<Fs>,
    ) -> anyhow::Result<TypedValidator> {
        match self {
            Validator::Affects => Ok(TypedValidator::Sync(Box::new(
                affects::AffectsValidator::new(Arc::clone(file_system)),
            ))),
            Validator::CheckAi => Ok(check_ai::validator()),
            Validator::CheckLua => check_lua::validator(file_system),
            Validator::KeepSorted => Ok(TypedValidator::Sync(Box::new(
                keep_sorted::KeepSortedValidator,
            ))),
            Validator::KeepUnique => Ok(TypedValidator::Sync(Box::new(
                keep_unique::KeepUniqueValidator,
            ))),
            Validator::LineCount => Ok(TypedValidator::Sync(Box::new(
                line_count::LineCountValidator,
            ))),
            Validator::LinePattern => Ok(TypedValidator::Sync(Box::new(
                line_pattern::LinePatternValidator,
            ))),
            Validator::SameAs => Ok(TypedValidator::Sync(Box::new(
                same_as::SameAsValidator::new(Arc::clone(file_system)),
            ))),
        }
    }

    /// The name of this validator as a static string.
    pub(crate) fn as_str(self) -> &'static str {
        self.into()
    }

    /// Looks up the validator called `value`. Returns an error if there is no validator with that
    /// name.
    pub(crate) fn parse(value: &str) -> anyhow::Result<Self> {
        value.parse::<Validator>().with_context(|| {
            let validators: Vec<&str> = Validator::iter().map(|v| v.as_str()).collect();
            format!(
                "unknown validator: {value}. Available validators: {}",
                validators.join(", ")
            )
        })
    }
}

/// Parses a numeric value from string.
///
/// Supports very big numbers (bigger than `f64` may hold) and `_` digits separator.
pub(crate) fn parse_number(value: &str) -> Option<BigDecimal> {
    let bytes = value.as_bytes();
    let separator_positions: Vec<usize> = bytes
        .iter()
        .enumerate()
        .filter(|(_, byte)| **byte == b'_')
        .map(|(index, _)| index)
        .collect();
    // BigDecimal crate does not handle "_" separators parsing well, so we do it here manually.
    let surrounded_by_digits = |index: usize| {
        let previous = index.checked_sub(1).map(|previous| bytes[previous]);
        let next = bytes.get(index + 1).copied();
        matches!((previous, next), (Some(previous), Some(next))
            if previous.is_ascii_digit() && next.is_ascii_digit())
    };
    if !separator_positions
        .iter()
        .copied()
        .all(surrounded_by_digits)
    {
        return None;
    }
    if separator_positions.is_empty() {
        value.parse().ok()
    } else {
        // Remove "_" separators to make BigDecimal parses the actual digits only.
        value.replace('_', "").parse().ok()
    }
}

#[cfg(test)]
mod parse_number_tests {
    use super::parse_number;

    fn parsed(value: &str) -> Option<String> {
        parse_number(value).map(|number| number.normalized().to_string())
    }

    #[test]
    fn plain_decimal_spellings_are_numbers() {
        assert_eq!(parsed("10"), Some("10".to_string()));
        assert_eq!(parsed("-1.5"), Some("-1.5".to_string()));
        assert_eq!(parsed("1e3"), Some("1000".to_string()));
    }

    #[test]
    fn digit_separators_between_digits_are_ignored() {
        assert_eq!(parsed("1_000"), Some("1000".to_string()));
        assert_eq!(parsed("1_000.000_1"), Some("1000.0001".to_string()));
        assert_eq!(parsed("-1_2"), Some("-12".to_string()));
        assert_eq!(parsed("1e1_0"), Some("10000000000".to_string()));
    }

    #[test]
    fn a_digit_separator_not_between_two_digits_is_not_a_number() {
        // Each of these is rejected by the languages that spell separators this way, so accepting
        // them here would let a typo pass as a number.
        for value in ["_1", "1_", "1__0", "1_.0", "1._0", "1_e3", "1e_3", "-_1"] {
            assert_eq!(parsed(value), None, "{value} unexpectedly parsed");
        }
    }

    #[test]
    fn a_separator_only_input_is_not_a_number() {
        assert_eq!(parsed("_"), None);
        assert_eq!(parsed(""), None);
    }

    #[test]
    fn float_specific_spellings_are_not_numbers() {
        assert_eq!(parsed("inf"), None);
        assert_eq!(parsed("NaN"), None);
    }

    #[test]
    fn an_exponent_beyond_the_supported_range_is_not_a_number() {
        assert_eq!(parsed("1e99999999999999999999"), None);
    }
}

#[cfg(test)]
mod tests {
    use crate::attributes::{AttributeName, Attributes};
    use crate::validators::Validator;
    use strum::IntoEnumIterator;

    #[test]
    fn validator_is_turned_on_by_each_of_its_attributes_and_no_other() {
        // `explain` and the diff-gated count must find a validator from any attribute that makes
        // the run check it, not only from the validator's own name. Any other attribute, such as
        // `keep-sorted-pattern`, turns no validator on.
        let turned_on_by = [
            (AttributeName::Affects, Validator::Affects),
            (AttributeName::CheckAi, Validator::CheckAi),
            (AttributeName::CheckLua, Validator::CheckLua),
            (AttributeName::KeepSorted, Validator::KeepSorted),
            (AttributeName::KeepUnique, Validator::KeepUnique),
            (AttributeName::LineCount, Validator::LineCount),
            (AttributeName::LinePattern, Validator::LinePattern),
            (AttributeName::LinePatternForbidden, Validator::LinePattern),
            (AttributeName::SameAs, Validator::SameAs),
            (AttributeName::SameAsPaths, Validator::SameAs),
        ];
        for (attribute, entry) in crate::attributes::test_utils::sample_entries() {
            let expected: Vec<Validator> = turned_on_by
                .iter()
                .filter(|(turning_on, _)| *turning_on == attribute)
                .map(|(_, validator)| *validator)
                .collect();
            let listed: Vec<Validator> = Validator::iter()
                .filter(|validator| validator.attributes().contains(&attribute))
                .collect();
            assert_eq!(listed, expected, "{attribute}");
            if let Ok(attributes) =
                Attributes::from_entries([(attribute.as_str().to_string(), entry)])
            {
                let turned_on: Vec<Validator> = Validator::iter()
                    .filter(|validator| validator.is_turned_on_by(&attributes))
                    .collect();
                assert_eq!(turned_on, expected, "{attribute}");
            }
        }
    }

    #[test]
    fn validator_lists_its_own_name_first() {
        // An error such as "`keep-sorted-format` without `keep-sorted`" shows the validator's own
        // name first.
        for validator in Validator::iter() {
            assert_eq!(
                validator.attributes()[0].as_str(),
                validator.as_str(),
                "{validator}"
            );
        }
    }
}

#[cfg(test)]
pub(crate) mod test_utils {
    use crate::blocks::FileBlocks;
    use crate::diff_parser::test_utils::diff_changes;
    use crate::diff_parser::{LineChange, LineChangeKind};
    use crate::fs::test_utils::{FakeFileSystem, path_filter_all};
    use crate::language_parsers::LanguageParsers;
    use crate::repo_path::RepoPath;
    use crate::scan::{Scan, ScanMode};
    use crate::validators::{ValidationContext, ValidationReport};
    use crate::virtual_blocks::VirtualBlock;
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
    pub(super) fn merge_validation_contexts(
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
