mod affects;
mod check_ai;
mod check_lua;
mod keep_sorted;
mod keep_unique;
mod line_count;
mod line_pattern;
mod same_as;

use crate::Position;
use crate::blocks::{Block, BlockSeverity, BlockWithContext, FileBlocks};
use crate::fs::FileSystem;
use crate::language_parsers::LanguageParsers;
use crate::repo_path::RepoPath;
use crate::validators::affects::AffectsValidatorDetector;
use crate::validators::check_ai::CheckAiValidatorDetector;
use crate::validators::check_lua::CheckLuaValidatorDetector;
use crate::validators::keep_sorted::KeepSortedValidatorDetector;
use crate::validators::keep_unique::KeepUniqueValidatorDetector;
use crate::validators::line_count::LineCountValidatorDetector;
use crate::validators::line_pattern::LinePatternValidatorDetector;
use crate::validators::same_as::SameAsValidatorDetector;
use anyhow::Context;
use async_trait::async_trait;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

/// Validates the given `Context` and returns a list of the violations grouped by filename.
#[async_trait]
pub trait ValidatorAsync: Send + Sync {
    async fn validate(&self, context: Arc<ValidationContext>) -> anyhow::Result<ValidationReport>;
}

pub trait ValidatorSync: Send + Sync {
    fn validate(&self, context: Arc<ValidationContext>) -> anyhow::Result<ValidationReport>;
}

/// Detects a [`ValidatorType`] for the given `block` (if any).
///
/// This is used to determine whether an async runtime (e.g. Tokio) is needed to run the validators.
pub trait ValidatorDetector<Fs: FileSystem> {
    fn detect(
        &self,
        block_with_context: &BlockWithContext,
        file_system: &Arc<Fs>,
    ) -> anyhow::Result<Option<ValidatorType>>;
}

/// Validator type (sync or async).
pub enum ValidatorType {
    Sync(Box<dyn ValidatorSync>),
    Async(Box<dyn ValidatorAsync>),
}

#[derive(Debug)]
pub struct Violation {
    range: ViolationRange,
    code: String,
    message: String,
    severity: BlockSeverity,
    data: Option<serde_json::Value>,
}

impl Violation {
    /// Constructs a new violation record with a name, error message, and optional machine-readable details.
    pub fn new(
        range: ViolationRange,
        code: String,
        message: String,
        severity: BlockSeverity,
        data: Option<serde_json::Value>,
    ) -> Self {
        Self {
            range,
            code,
            message,
            severity,
            data,
        }
    }

    pub fn as_simple_diagnostic(&self) -> SimpleDiagnostic<'_> {
        SimpleDiagnostic {
            range: &self.range,
            code: self.code.as_str(),
            message: self.message.as_str(),
            severity: self.severity,
            data: &self.data,
        }
    }
}

/// What a single validator found, and which blocks it looked at.
///
/// A block is identified by the position of its start tag, because a block may have no name and
/// the position is unique within a file.
#[derive(Debug, Default)]
pub struct ValidationReport {
    /// Violations grouped by the file they were found in.
    pub violations: HashMap<RepoPath, Vec<Violation>>,
    /// The start position of every block this validator checked.
    pub checked_blocks: Vec<(RepoPath, Position)>,
}

impl ValidationReport {
    /// Adds a checked block together with the violations found in it.
    pub fn add_all(&mut self, file: &RepoPath, block: &Block, violations: Vec<Violation>) {
        self.add_checked_block(file, block);
        self.add_violations(file, violations);
    }

    /// Adds `block` in `file` to the blocks this validator checked.
    pub fn add_checked_block(&mut self, file: &RepoPath, block: &Block) {
        self.checked_blocks
            .push((file.clone(), block.start_tag_position_range.start().clone()));
    }

    /// Stores violations found in `file`.
    pub fn add_violations(&mut self, file: &RepoPath, violations: Vec<Violation>) {
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
/// Checked blocks are keyed by file, then by the start position of the block, then by the names of
/// the validators that checked it.
#[derive(Debug, Default)]
pub struct ValidationLog {
    /// Violations grouped by the file they were found in.
    pub violations: HashMap<RepoPath, Vec<Violation>>,
    pub checked_blocks: BTreeMap<RepoPath, BTreeMap<Position, BTreeSet<&'static str>>>,
}

impl ValidationLog {
    /// Adds one validator's `report` for the corresponding `validator` name.
    pub fn add_validation_report(&mut self, validator: &'static str, report: ValidationReport) {
        for (file, position) in report.checked_blocks {
            self.checked_blocks
                .entry(file)
                .or_default()
                .entry(position)
                .or_default()
                .insert(validator);
        }
        for (file, violations) in report.violations {
            self.violations.entry(file).or_default().extend(violations);
        }
    }

    /// Adds every checked block and violation from `other` to this log.
    pub fn merge(&mut self, other: ValidationLog) {
        for (file, blocks) in other.checked_blocks {
            let checked_in_file = self.checked_blocks.entry(file).or_default();
            for (position, validators) in blocks {
                checked_in_file
                    .entry(position)
                    .or_default()
                    .extend(validators);
            }
        }
        for (file, violations) in other.violations {
            self.violations.entry(file).or_default().extend(violations);
        }
    }
}

#[derive(Serialize, Debug, PartialEq)]
pub struct ViolationRange {
    start: Position,
    end: Position,
}

impl ViolationRange {
    pub(crate) fn new(start: Position, end: Position) -> Self {
        Self { start, end }
    }
}

/// Represents a simplified, serializable diagnostic message.
///
/// It mimics the [Diagnostic](https://github.com/microsoft/vscode-languageserver-node/blob/3412a17149850f445bf35b4ad71148cfe5f8411e/types/src/main.ts#L688)
/// object but omits some redundant fields and keeps all line numbers 1-based instead of zero-based.
#[derive(Serialize, Debug)]
pub struct SimpleDiagnostic<'a> {
    range: &'a ViolationRange,
    code: &'a str,
    message: &'a str,
    severity: BlockSeverity,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: &'a Option<serde_json::Value>,
}

impl SimpleDiagnostic<'_> {
    pub fn severity(&self) -> BlockSeverity {
        self.severity
    }
}

pub struct ValidationContext {
    // Blocks with their corresponding source file contents grouped by filename.
    pub(crate) blocks: HashMap<RepoPath, FileBlocks>,
    // Language parsers per file type, used by validators to parse referenced source files.
    pub(crate) parsers: LanguageParsers,
}

impl ValidationContext {
    /// Creates a new validation context with modified blocks grouped by filename.
    pub fn new(blocks: HashMap<RepoPath, FileBlocks>, parsers: LanguageParsers) -> Self {
        Self { blocks, parsers }
    }

    /// Returns the language parsers available to validators.
    pub fn parsers(&self) -> &LanguageParsers {
        &self.parsers
    }

    /// Converts the validation context to a serializable report that can be displayed as JSON.
    pub fn to_serializable_report(&self) -> HashMap<RepoPath, Vec<serde_json::Value>> {
        let mut report = HashMap::new();
        for (path, file_blocks) in &self.blocks {
            report.insert(path.clone(), file_blocks.to_serializable_report());
        }
        report
    }
}

/// Runs all sync validators concurrently each in a separate thread and returns violations grouped
/// by file paths.
fn run_sync_validators(
    context: Arc<ValidationContext>,
    validators: SyncValidators,
) -> anyhow::Result<ValidationLog> {
    let mut handles = Vec::new();
    for (name, validator) in validators {
        let context = Arc::clone(&context);
        handles.push(std::thread::spawn(move || {
            (name, validator.validate(context))
        }));
    }

    let mut log = ValidationLog::default();
    for handle in handles {
        match handle.join() {
            Ok((name, Ok(report))) => log.add_validation_report(name, report),
            Ok((_, Err(e))) => return Err(e),
            Err(e) => return Err(anyhow::anyhow!("Failed to run validation: {e:?}")),
        }
    }

    Ok(log)
}

/// Runs all async validators concurrently via Tokio and returns violations grouped by file paths.
fn run_async_validators(
    context: Arc<ValidationContext>,
    validators: AsyncValidators,
) -> anyhow::Result<ValidationLog> {
    let tokio_runtime = tokio::runtime::Runtime::new()?;
    tokio_runtime.block_on(async move {
        let mut tasks = tokio::task::JoinSet::new();
        for (name, validator) in validators {
            let context = Arc::clone(&context);
            // The name travels with the task because results arrive in completion order.
            tasks.spawn(async move { (name, validator.validate(context).await) });
        }

        let mut log = ValidationLog::default();
        while let Some(result) = tasks.join_next().await {
            match result {
                Ok((name, Ok(report))) => log.add_validation_report(name, report),
                Ok((_, Err(e))) => return Err(e),
                Err(e) => return Err(anyhow::anyhow!("Failed to run validation: {e}")),
            }
        }

        Ok(log)
    })
}

/// Run the given sync and async validators in separate threads in parallel.
pub fn run(
    context: Arc<ValidationContext>,
    sync_validators: SyncValidators,
    async_validators: AsyncValidators,
) -> anyhow::Result<ValidationLog> {
    if async_validators.is_empty() {
        return run_sync_validators(context, sync_validators);
    }
    // Run sync and async validators concurrently.
    let sync_context = Arc::clone(&context);
    let sync_violations_handle =
        std::thread::spawn(move || run_sync_validators(sync_context, sync_validators));
    let async_violations_handle =
        std::thread::spawn(move || run_async_validators(context, async_validators));
    let sync_result = sync_violations_handle
        .join()
        .map_err(|e| anyhow::anyhow!("Failed to join sync violations thread: {e:?}"))?;
    let mut log = sync_result?;

    let async_result = async_violations_handle
        .join()
        .map_err(|e| anyhow::anyhow!("Failed to join async violations thread: {e:?}"))?;
    log.merge(async_result?);

    Ok(log)
}

/// Detected validators, each paired with the name it is known by.
type SyncValidators = Vec<(&'static str, Box<dyn ValidatorSync>)>;
type AsyncValidators = Vec<(&'static str, Box<dyn ValidatorAsync>)>;

type DetectorFactory<Fs> = fn() -> Box<dyn ValidatorDetector<Fs>>;

/// Builds the ordered detector registry for a concrete filesystem `Fs`.
///
/// This is a generic function rather than a `const` because each [`DetectorFactory`] is now
/// parameterized by the filesystem type its detectors receive, so the registry has to be
/// instantiated per `Fs` (the production `FileSystemImpl`, a `FakeFileSystem` in tests).
pub fn detector_factories<Fs: FileSystem + 'static>() -> Vec<(&'static str, DetectorFactory<Fs>)> {
    vec![
        // <block affects="README.md:available-validators">
        ("affects", || Box::new(AffectsValidatorDetector::new())),
        ("keep-sorted", || {
            Box::new(KeepSortedValidatorDetector::new())
        }),
        ("keep-unique", || {
            Box::new(KeepUniqueValidatorDetector::new())
        }),
        ("line-pattern", || {
            Box::new(LinePatternValidatorDetector::new())
        }),
        ("line-count", || Box::new(LineCountValidatorDetector::new())),
        ("check-ai", || Box::new(CheckAiValidatorDetector::new())),
        ("check-lua", || Box::new(CheckLuaValidatorDetector::new())),
        ("same-as", || Box::new(SameAsValidatorDetector::new())),
        // </block>
    ]
}

pub fn detect_validators<Fs: FileSystem + 'static>(
    context: &ValidationContext,
    detectors: &[(&'static str, DetectorFactory<Fs>)],
    disabled_validators: &HashSet<&str>,
    enabled_validators: &HashSet<&str>,
    file_system: &Arc<Fs>,
) -> anyhow::Result<(SyncValidators, AsyncValidators)> {
    let mut validator_detectors: Vec<(&'static str, Box<dyn ValidatorDetector<Fs>>)> = detectors
        .iter()
        .filter(|(validator_name, _)| {
            if !enabled_validators.is_empty() {
                enabled_validators.contains(validator_name)
            } else {
                !disabled_validators.contains(validator_name)
            }
        })
        .map(|(name, factory)| (*name, factory()))
        .collect();
    let mut sync_validators = Vec::new();
    let mut async_validators = Vec::new();
    'outer: for file_blocks in context.blocks.values() {
        for block in &file_blocks.blocks_with_context {
            let mut undetected = Vec::new();
            while let Some((name, detector)) = validator_detectors.pop() {
                match detector.detect(block, file_system)? {
                    Some(ValidatorType::Sync(validator)) => {
                        sync_validators.push((name, validator));
                    }
                    Some(ValidatorType::Async(validator)) => {
                        async_validators.push((name, validator));
                    }
                    None => {
                        undetected.push((name, detector));
                    }
                }
            }
            if undetected.is_empty() {
                // All validators have been detected.
                break 'outer;
            }
            validator_detectors.extend(undetected);
        }
    }
    Ok((sync_validators, async_validators))
}

/// Parses a comma-separated list of block references in the `file:name` (or `:name` for the same
/// file) syntax shared by the reference-based validators (`affects`, `check-lua`, `same-as`).
///
/// Returns each reference as an `(optional file path, block name)` pair; an empty file part yields
/// `None`, meaning "a block in the same file".
pub(in crate::validators) fn parse_block_references(
    value: &str,
) -> anyhow::Result<Vec<(Option<RepoPath>, String)>> {
    let mut result = Vec::new();
    for block_ref in value.split(',') {
        let block = block_ref.trim();
        let (mut filename, block_name) = block
            .split_once(":")
            .context(format!("Invalid block reference: \"{block}\"",))?;
        filename = filename.trim();
        result.push((
            if filename.is_empty() {
                None
            } else {
                // Normalized so `./target.py` and `target.py` resolve to the same block.
                Some(RepoPath::from_reference(filename)?)
            },
            block_name.trim().to_string(),
        ));
    }
    Ok(result)
}

#[cfg(test)]
mod parse_block_references_tests {
    use crate::repo_path::RepoPath;
    use crate::validators::parse_block_references;
    #[test]
    fn single_reference() -> anyhow::Result<()> {
        let result = parse_block_references("file.rs:block_name")?;
        assert_eq!(
            result,
            vec![(
                Some(RepoPath::from_reference("file.rs")?),
                "block_name".to_string()
            )]
        );
        Ok(())
    }

    #[test]
    fn multiple_references() -> anyhow::Result<()> {
        let result = parse_block_references("file1.rs:block1, file2.rs:block2")?;
        assert_eq!(
            result,
            vec![
                (
                    Some(RepoPath::from_reference("file1.rs")?),
                    "block1".to_string()
                ),
                (
                    Some(RepoPath::from_reference("file2.rs")?),
                    "block2".to_string()
                )
            ]
        );
        Ok(())
    }

    #[test]
    fn empty_filename_returns_none_for_filename() -> anyhow::Result<()> {
        let result = parse_block_references(":block_name")?;
        assert_eq!(result, vec![(None, "block_name".to_string())]);
        Ok(())
    }

    #[test]
    fn multiple_empty_filename_references_returns_non_for_filename() -> anyhow::Result<()> {
        let result = parse_block_references(":block1, :block2")?;
        assert_eq!(
            result,
            vec![(None, "block1".to_string()), (None, "block2".to_string())]
        );
        Ok(())
    }

    #[test]
    fn invalid_block_returns_error() {
        let result = parse_block_references("invalid_reference");
        assert!(result.is_err());
    }
}

#[cfg(test)]
mod tests {
    use crate::blocks::{Block, BlockWithContext};
    use crate::fs::FileSystem;
    use crate::fs::test_utils::FakeFileSystem;
    use crate::repo_path::RepoPath;
    use crate::test_utils::{merge_validation_contexts, validation_context};
    use crate::validators::{
        DetectorFactory, ValidationContext, ValidationReport, ValidatorAsync, ValidatorDetector,
        ValidatorSync, ValidatorType, Violation, ViolationRange, detect_validators,
    };
    use crate::{Position, validators};
    use async_trait::async_trait;
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;

    fn empty_testing_block() -> Block {
        Block::new(
            HashMap::new(),
            Position::new(0, 0)..=Position::new(0, 0),
            0..0,
            Position::new(0, 0)..Position::new(0, 0),
        )
    }

    fn empty_testing_violation_range() -> ViolationRange {
        ViolationRange::new(Position::new(0, 0), Position::new(0, 0))
    }

    struct FakeAsyncValidator {
        testing_block: Arc<Block>,
    }

    #[async_trait]
    impl ValidatorAsync for FakeAsyncValidator {
        async fn validate(
            &self,
            context: Arc<ValidationContext>,
        ) -> anyhow::Result<ValidationReport> {
            Ok(ValidationReport {
                violations: context
                    .blocks
                    .keys()
                    .map(|file_name| {
                        (
                            file_name.clone(),
                            vec![Violation::new(
                                empty_testing_violation_range(),
                                "fake-async".to_string(),
                                "fake-async error message".to_string(),
                                self.testing_block.severity().unwrap(),
                                None,
                            )],
                        )
                    })
                    .collect(),
                checked_blocks: Vec::new(),
            })
        }
    }

    struct FakeSyncValidator {
        testing_block: Arc<Block>,
    }

    impl ValidatorSync for FakeSyncValidator {
        fn validate(&self, context: Arc<ValidationContext>) -> anyhow::Result<ValidationReport> {
            Ok(ValidationReport {
                violations: context
                    .blocks
                    .keys()
                    .map(|file_name| {
                        (
                            file_name.clone(),
                            vec![Violation::new(
                                empty_testing_violation_range(),
                                "fake-sync".to_string(),
                                "fake-sync error message".to_string(),
                                self.testing_block.severity().unwrap(),
                                None,
                            )],
                        )
                    })
                    .collect(),
                checked_blocks: Vec::new(),
            })
        }
    }

    struct FakeAsyncValidatorDetector();

    impl<Fs: FileSystem> ValidatorDetector<Fs> for FakeAsyncValidatorDetector {
        fn detect(
            &self,
            block_with_context: &BlockWithContext,
            _file_system: &Arc<Fs>,
        ) -> anyhow::Result<Option<ValidatorType>> {
            if block_with_context
                .block
                .attributes
                .contains_key("fake-async")
            {
                Ok(Some(ValidatorType::Async(Box::new(FakeAsyncValidator {
                    testing_block: Arc::new(empty_testing_block()),
                }))))
            } else {
                Ok(None)
            }
        }
    }

    struct FakeSyncValidatorDetector();
    impl<Fs: FileSystem> ValidatorDetector<Fs> for FakeSyncValidatorDetector {
        fn detect(
            &self,
            block_with_context: &BlockWithContext,
            _file_system: &Arc<Fs>,
        ) -> anyhow::Result<Option<ValidatorType>> {
            if block_with_context
                .block
                .attributes
                .contains_key("fake-sync")
            {
                Ok(Some(ValidatorType::Sync(Box::new(FakeSyncValidator {
                    testing_block: Arc::new(empty_testing_block()),
                }))))
            } else {
                Ok(None)
            }
        }
    }

    fn detector_factories<Fs: FileSystem + 'static>() -> Vec<(&'static str, DetectorFactory<Fs>)> {
        vec![
            ("fake-sync", || Box::new(FakeSyncValidatorDetector {})),
            ("fake-async", || Box::new(FakeAsyncValidatorDetector {})),
        ]
    }

    #[test]
    fn detect_and_run_with_sync_and_async_validators_returns_correct_violations()
    -> anyhow::Result<()> {
        let context = merge_validation_contexts(vec![
            validation_context(
                "example1.py",
                r#"# <block fake-sync="condition A" fake-async="condition B">
    # </block>"#,
            ),
            validation_context(
                "example2.py",
                r#"# <block fake-sync="condition C" fake-async="condition D">
    # </block>"#,
            ),
        ]);

        let (sync_validators, async_validators) = detect_validators(
            &context,
            &detector_factories(),
            &HashSet::new(),
            &HashSet::new(),
            &Arc::new(FakeFileSystem::new(HashMap::new())),
        )?;
        let violations = validators::run(context, sync_validators, async_validators)?.violations;

        assert_eq!(violations.len(), 2);
        assert_eq!(
            violations[&RepoPath::from_reference("example1.py")?].len(),
            2
        );
        let mut file1_violations = violations[&RepoPath::from_reference("example1.py")?]
            .iter()
            .map(|v| v.code.as_str())
            .collect::<Vec<_>>();
        file1_violations.sort();
        assert_eq!(file1_violations, vec!["fake-async", "fake-sync"]);
        assert_eq!(
            violations[&RepoPath::from_reference("example2.py")?].len(),
            2
        );
        let mut file2_violations = violations[&RepoPath::from_reference("example2.py")?]
            .iter()
            .map(|v| v.code.as_str())
            .collect::<Vec<_>>();
        file2_violations.sort();
        assert_eq!(file2_violations, vec!["fake-async", "fake-sync"]);
        Ok(())
    }

    #[test]
    fn detect_and_run_with_sync_only_validators_returns_correct_violations() -> anyhow::Result<()> {
        let context = merge_validation_contexts(vec![
            validation_context(
                "example1.py",
                r#"# <block fake-sync="condition A">
    # </block>"#,
            ),
            validation_context(
                "example2.py",
                r#"# <block fake-sync="condition B">
    # </block>"#,
            ),
        ]);

        let (sync_validators, async_validators) = detect_validators(
            &context,
            &detector_factories(),
            &HashSet::new(),
            &HashSet::new(),
            &Arc::new(FakeFileSystem::new(HashMap::new())),
        )?;
        let violations = validators::run(context, sync_validators, async_validators)?.violations;

        assert_eq!(violations.len(), 2);
        assert_eq!(
            violations[&RepoPath::from_reference("example1.py")?].len(),
            1
        );
        assert_eq!(
            violations[&RepoPath::from_reference("example1.py")?][0].code,
            "fake-sync"
        );
        assert_eq!(
            violations[&RepoPath::from_reference("example2.py")?].len(),
            1
        );
        assert_eq!(
            violations[&RepoPath::from_reference("example2.py")?][0].code,
            "fake-sync"
        );
        Ok(())
    }

    #[test]
    fn detect_and_run_with_async_only_validators_returns_correct_violations() -> anyhow::Result<()>
    {
        let context = merge_validation_contexts(vec![
            validation_context(
                "example1.py",
                r#"# <block fake-async="condition A">
    # </block>"#,
            ),
            validation_context(
                "example2.py",
                r#"# <block fake-async="condition B">
    # </block>"#,
            ),
        ]);
        let (sync_validators, async_validators) = detect_validators(
            &context,
            &detector_factories(),
            &HashSet::new(),
            &HashSet::new(),
            &Arc::new(FakeFileSystem::new(HashMap::new())),
        )?;
        let violations = validators::run(context, sync_validators, async_validators)?.violations;

        assert_eq!(violations.len(), 2);
        assert_eq!(
            violations[&RepoPath::from_reference("example1.py")?].len(),
            1
        );
        assert_eq!(
            violations[&RepoPath::from_reference("example1.py")?][0].code,
            "fake-async"
        );
        assert_eq!(
            violations[&RepoPath::from_reference("example2.py")?].len(),
            1
        );
        assert_eq!(
            violations[&RepoPath::from_reference("example2.py")?][0].code,
            "fake-async"
        );
        Ok(())
    }

    #[test]
    fn detect_and_run_with_disabled_async_validators_returns_violations_for_sync_validators_only()
    -> anyhow::Result<()> {
        let context = validation_context(
            "example1.py",
            r#"# <block fake-sync="condition A" fake-async="condition B">
    # </block>"#,
        );
        let (sync_validators, async_validators) = detect_validators(
            &context,
            &detector_factories(),
            &HashSet::from(["fake-async"]),
            &HashSet::new(),
            &Arc::new(FakeFileSystem::new(HashMap::new())),
        )?;
        let violations = validators::run(context, sync_validators, async_validators)?.violations;

        assert_eq!(violations.len(), 1);
        assert_eq!(
            violations[&RepoPath::from_reference("example1.py")?].len(),
            1
        );
        assert_eq!(
            violations[&RepoPath::from_reference("example1.py")?][0].code,
            "fake-sync"
        );
        Ok(())
    }

    #[test]
    fn detect_and_run_with_enabled_async_validators_returns_violations_for_async_validators_only()
    -> anyhow::Result<()> {
        let context = validation_context(
            "example1.py",
            r#"# <block fake-sync="condition A" fake-async="condition B">
    # </block>"#,
        );

        let (sync_validators, async_validators) = detect_validators(
            &context,
            &detector_factories(),
            &HashSet::new(),
            &HashSet::from(["fake-async"]),
            &Arc::new(FakeFileSystem::new(HashMap::new())),
        )?;
        let violations = validators::run(context, sync_validators, async_validators)?.violations;

        assert_eq!(violations.len(), 1);
        assert_eq!(
            violations[&RepoPath::from_reference("example1.py")?].len(),
            1
        );
        assert_eq!(
            violations[&RepoPath::from_reference("example1.py")?][0].code,
            "fake-async"
        );
        Ok(())
    }

    #[test]
    fn detect_and_run_with_disabled_sync_validators_returns_violations_for_async_validators_only()
    -> anyhow::Result<()> {
        let context = validation_context(
            "example1.py",
            r#"# <block fake-sync="condition A" fake-async="condition B">
    # </block>"#,
        );
        let (sync_validators, async_validators) = detect_validators(
            &context,
            &detector_factories(),
            &HashSet::from(["fake-sync"]),
            &HashSet::new(),
            &Arc::new(FakeFileSystem::new(HashMap::new())),
        )?;
        let violations = validators::run(context, sync_validators, async_validators)?.violations;

        assert_eq!(violations.len(), 1);
        assert_eq!(
            violations[&RepoPath::from_reference("example1.py")?].len(),
            1
        );
        assert_eq!(
            violations[&RepoPath::from_reference("example1.py")?][0].code,
            "fake-async"
        );
        Ok(())
    }

    #[test]
    fn detect_and_run_with_enabled_sync_validators_returns_violations_for_sync_validators_only()
    -> anyhow::Result<()> {
        let context = validation_context(
            "example1.py",
            r#"# <block fake-sync="condition A" fake-async="condition B">
    # </block>"#,
        );

        let (sync_validators, async_validators) = detect_validators(
            &context,
            &detector_factories(),
            &HashSet::new(),
            &HashSet::from(["fake-sync"]),
            &Arc::new(FakeFileSystem::new(HashMap::new())),
        )?;
        let violations = validators::run(context, sync_validators, async_validators)?.violations;

        assert_eq!(violations.len(), 1);
        assert_eq!(
            violations[&RepoPath::from_reference("example1.py")?].len(),
            1
        );
        assert_eq!(
            violations[&RepoPath::from_reference("example1.py")?][0].code,
            "fake-sync"
        );
        Ok(())
    }

    #[test]
    fn to_serializable_report_returns_correct_listings() -> anyhow::Result<()> {
        let contents = r#"/* <block name="top"> */ let a = "cc"; /* </block> Block on the first line. */
// <block name="first" attr1="val1"> Block on the second line.
fn a() {}
// </block>
//     <block name="second" attr2="val2"> Block with indent.
fn b() {}
// </block>
/* <block name="bottom"> */ fn c() {} /* </block> Block on the last line. */"#;
        let context = validation_context("example.rs", contents);
        let report = context.to_serializable_report();

        assert_eq!(report.len(), 1);
        let listings = &report[&RepoPath::from_reference("example.rs")?];
        assert_eq!(listings.len(), 4);
        assert_eq!(
            listings,
            &vec![
                serde_json::json!({
                    "name": "top",
                    "line": 1,
                    "column": 4,
                    "is_content_modified": true,
                    "attributes": {
                        "name": "top"
                    }
                }),
                serde_json::json!({
                    "name": "first",
                    "line": 2,
                    "column": 4,
                    "is_content_modified": true,
                    "attributes": {
                        "attr1": "val1",
                        "name": "first",
                    }
                }),
                serde_json::json!({
                    "name": "second",
                    "line": 5,
                    "column": 8,
                    "is_content_modified": true,
                    "attributes": {
                        "attr2": "val2",
                        "name": "second"
                    }
                }),
                serde_json::json!({
                    "name": "bottom",
                    "line": 8,
                    "column": 4,
                    "is_content_modified": true,
                    "attributes": {
                        "name": "bottom"
                    }
                })
            ]
        );

        Ok(())
    }
}
