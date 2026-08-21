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
use crate::diff_parser::LineChange;
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
use std::ffi::OsString;
use std::sync::Arc;

/// Validates the given `Context` and returns a list of the violations grouped by filename.
#[async_trait]
pub trait ValidatorAsync: Send + Sync {
    async fn validate(&self, context: Arc<ValidationContext>) -> anyhow::Result<ValidationReport>;
}

/// The same contract as [`ValidatorAsync`] for validators that need no I/O beyond the filesystem,
/// so the common case runs on plain threads and no Tokio runtime has to be started if no async
/// validators are involved.
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

/// One rule breach found in one block: what is wrong, where, and how severe it is.
///
/// Fields are private because the public shape of a violation is [`SimpleDiagnostic`], the form
/// that gets serialized for editors and CI.
#[derive(Debug)]
pub struct Violation {
    /// Where to underline in the file — normally the block's start tag, or the offending line.
    range: ViolationRange,
    /// The name of the validator that reported it, e.g. `"keep-sorted"`.
    code: String,
    /// Human-readable explanation, printed to the developer.
    message: String,
    /// Decides whether this breach fails the run; see [`BlockSeverity`].
    severity: BlockSeverity,
    /// Validator-specific details for tools, e.g. the expected and actual line count.
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
    /// Every block that was examined, and by which validators. BTreeMap is used so the
    /// `--verbosity` report comes out in a stable order regardless of the validators order.
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

/// The span an editor should highlight for a violation. Both ends are 1-based and inclusive.
#[derive(Serialize, Debug, PartialEq)]
pub struct ViolationRange {
    start: Position,
    end: Position,
}

impl ViolationRange {
    /// Creates a range from its two endpoints, which must be in the same file.
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
    /// The severity, which the caller uses to decide the process exit code.
    pub fn severity(&self) -> BlockSeverity {
        self.severity
    }
}

/// Everything the validators are given to work with, shared read-only across all of them.
///
/// Built once per run and handed out as an `Arc`, because validators run concurrently on separate
/// threads and each needs the whole picture: a rule such as `affects` has to see blocks in files
/// other than the one it started from.
pub struct ValidationContext {
    /// Blocks with their corresponding source file contents grouped by filename.
    pub(crate) blocks: HashMap<RepoPath, FileBlocks>,
    /// Language parsers per file type, used by validators to parse referenced source files.
    pub(crate) parsers: LanguageParsers,
    /// Every line change from the diff (if any).
    pub(crate) line_changes: HashMap<RepoPath, Vec<LineChange>>,
    /// Extension remappings from the command line.
    pub(crate) extra_file_extensions: HashMap<OsString, OsString>,
}

impl ValidationContext {
    /// Creates a new validation context with modified blocks grouped by filename.
    pub fn new(
        blocks: HashMap<RepoPath, FileBlocks>,
        parsers: LanguageParsers,
        line_changes: HashMap<RepoPath, Vec<LineChange>>,
        extra_file_extensions: HashMap<OsString, OsString>,
    ) -> Self {
        Self {
            blocks,
            parsers,
            line_changes,
            extra_file_extensions,
        }
    }

    /// Returns the language parsers available to validators.
    pub fn parsers(&self) -> &LanguageParsers {
        &self.parsers
    }

    /// The extension remappings the run was given.
    pub(crate) fn extra_file_extensions(&self) -> &HashMap<OsString, OsString> {
        &self.extra_file_extensions
    }

    /// The line changes the diff reported for the `file_path`.
    pub(crate) fn line_changes_for(&self, file_path: &RepoPath) -> Option<&[LineChange]> {
        self.line_changes.get(file_path).map(Vec::as_slice)
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

/// Validators that only ever fire when a diff touches the corresponding blocks, each paired with
/// the block attribute that selects it.
pub const DIFF_GATED_VALIDATORS: &[(&str, &str)] = &[("affects", "affects")];

/// Whether a validator is enabled/disabled.
fn is_validator_active(
    validator_name: &str,
    disabled_validators: &HashSet<&str>,
    enabled_validators: &HashSet<&str>,
) -> bool {
    if enabled_validators.is_empty() {
        !disabled_validators.contains(validator_name)
    } else {
        enabled_validators.contains(validator_name)
    }
}

/// Counts the blocks in `context` carrying a rule that cannot fire unless a diff is supplied; see
/// [`DIFF_GATED_VALIDATORS`].
pub fn diff_gated_block_count(
    context: &ValidationContext,
    disabled_validators: &HashSet<&str>,
    enabled_validators: &HashSet<&str>,
) -> usize {
    let attributes: Vec<&str> = DIFF_GATED_VALIDATORS
        .iter()
        .filter(|(validator_name, _)| {
            is_validator_active(validator_name, disabled_validators, enabled_validators)
        })
        .map(|(_, attribute)| *attribute)
        .collect();
    context
        .blocks
        .values()
        .flat_map(|file_blocks| &file_blocks.blocks_with_context)
        .filter(|block_with_context| {
            attributes
                .iter()
                .any(|attribute| block_with_context.block.attributes.contains_key(*attribute))
        })
        .count()
}

/// Instantiates exactly the validators the blocks in `context` call for.
///
/// A validator is created at most once, no matter how many blocks use it, and scanning stops as
/// soon as every candidate has been detected. Returning sync and async validators separately lets
/// the caller skip starting a Tokio runtime when no async validator is present.
///
/// `enabled_validators` takes precedence over `disabled_validators`: when it is non-empty, only the
/// validators it names are considered. Passing both is rejected earlier, when the flags are parsed.
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
            is_validator_active(validator_name, disabled_validators, enabled_validators)
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
                                "check-ai".to_string(),
                                "check-ai error message".to_string(),
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
                                "keep-sorted".to_string(),
                                "keep-sorted error message".to_string(),
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
            if block_with_context.block.attributes.contains_key("check-ai") {
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
                .contains_key("keep-sorted")
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
            ("keep-sorted", || Box::new(FakeSyncValidatorDetector {})),
            ("check-ai", || Box::new(FakeAsyncValidatorDetector {})),
        ]
    }

    #[test]
    fn detect_and_run_with_sync_and_async_validators_returns_correct_violations()
    -> anyhow::Result<()> {
        let context = merge_validation_contexts(vec![
            validation_context(
                "example1.py",
                r#"# <block keep-sorted="condition A" check-ai="condition B">
    # </block>"#,
            ),
            validation_context(
                "example2.py",
                r#"# <block keep-sorted="condition C" check-ai="condition D">
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
        assert_eq!(file1_violations, vec!["check-ai", "keep-sorted"]);
        assert_eq!(
            violations[&RepoPath::from_reference("example2.py")?].len(),
            2
        );
        let mut file2_violations = violations[&RepoPath::from_reference("example2.py")?]
            .iter()
            .map(|v| v.code.as_str())
            .collect::<Vec<_>>();
        file2_violations.sort();
        assert_eq!(file2_violations, vec!["check-ai", "keep-sorted"]);
        Ok(())
    }

    #[test]
    fn detect_and_run_with_sync_only_validators_returns_correct_violations() -> anyhow::Result<()> {
        let context = merge_validation_contexts(vec![
            validation_context(
                "example1.py",
                r#"# <block keep-sorted="condition A">
    # </block>"#,
            ),
            validation_context(
                "example2.py",
                r#"# <block keep-sorted="condition B">
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
            "keep-sorted"
        );
        assert_eq!(
            violations[&RepoPath::from_reference("example2.py")?].len(),
            1
        );
        assert_eq!(
            violations[&RepoPath::from_reference("example2.py")?][0].code,
            "keep-sorted"
        );
        Ok(())
    }

    #[test]
    fn detect_and_run_with_async_only_validators_returns_correct_violations() -> anyhow::Result<()>
    {
        let context = merge_validation_contexts(vec![
            validation_context(
                "example1.py",
                r#"# <block check-ai="condition A">
    # </block>"#,
            ),
            validation_context(
                "example2.py",
                r#"# <block check-ai="condition B">
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
            "check-ai"
        );
        assert_eq!(
            violations[&RepoPath::from_reference("example2.py")?].len(),
            1
        );
        assert_eq!(
            violations[&RepoPath::from_reference("example2.py")?][0].code,
            "check-ai"
        );
        Ok(())
    }

    #[test]
    fn detect_and_run_with_disabled_async_validators_returns_violations_for_sync_validators_only()
    -> anyhow::Result<()> {
        let context = validation_context(
            "example1.py",
            r#"# <block keep-sorted="condition A" check-ai="condition B">
    # </block>"#,
        );
        let (sync_validators, async_validators) = detect_validators(
            &context,
            &detector_factories(),
            &HashSet::from(["check-ai"]),
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
            "keep-sorted"
        );
        Ok(())
    }

    #[test]
    fn detect_and_run_with_enabled_async_validators_returns_violations_for_async_validators_only()
    -> anyhow::Result<()> {
        let context = validation_context(
            "example1.py",
            r#"# <block keep-sorted="condition A" check-ai="condition B">
    # </block>"#,
        );

        let (sync_validators, async_validators) = detect_validators(
            &context,
            &detector_factories(),
            &HashSet::new(),
            &HashSet::from(["check-ai"]),
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
            "check-ai"
        );
        Ok(())
    }

    #[test]
    fn detect_and_run_with_disabled_sync_validators_returns_violations_for_async_validators_only()
    -> anyhow::Result<()> {
        let context = validation_context(
            "example1.py",
            r#"# <block keep-sorted="condition A" check-ai="condition B">
    # </block>"#,
        );
        let (sync_validators, async_validators) = detect_validators(
            &context,
            &detector_factories(),
            &HashSet::from(["keep-sorted"]),
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
            "check-ai"
        );
        Ok(())
    }

    #[test]
    fn detect_and_run_with_enabled_sync_validators_returns_violations_for_sync_validators_only()
    -> anyhow::Result<()> {
        let context = validation_context(
            "example1.py",
            r#"# <block keep-sorted="condition A" check-ai="condition B">
    # </block>"#,
        );

        let (sync_validators, async_validators) = detect_validators(
            &context,
            &detector_factories(),
            &HashSet::new(),
            &HashSet::from(["keep-sorted"]),
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
            "keep-sorted"
        );
        Ok(())
    }

    #[test]
    fn to_serializable_report_returns_correct_listings() -> anyhow::Result<()> {
        let contents = r#"/* <block name="top"> */ let a = "cc"; /* </block> Block on the first line. */
// <block name="first"> Block on the second line.
fn a() {}
// </block>
//     <block name="second"> Block with indent.
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
                        "name": "first",
                    }
                }),
                serde_json::json!({
                    "name": "second",
                    "line": 5,
                    "column": 8,
                    "is_content_modified": true,
                    "attributes": {
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

    /// A file holding a diff-gated rule and the block it points at: only the block carrying the
    /// attribute has a rule that a run without a diff leaves unchecked.
    fn diff_gated_context() -> Arc<ValidationContext> {
        validation_context(
            "file1.py",
            r#"# <block affects=":target">
print("source")
# </block>

# <block name="target">
print("target")
# </block>
"#,
        )
    }

    #[test]
    fn diff_gated_block_count_counts_the_blocks_carrying_the_rule() {
        assert_eq!(
            validators::diff_gated_block_count(
                &diff_gated_context(),
                &HashSet::new(),
                &HashSet::new()
            ),
            1
        );
    }

    #[test]
    fn diff_gated_block_count_with_the_validator_disabled_returns_zero() {
        assert_eq!(
            validators::diff_gated_block_count(
                &diff_gated_context(),
                &HashSet::from(["affects"]),
                &HashSet::new()
            ),
            0
        );
    }

    #[test]
    fn diff_gated_block_count_with_other_validators_enabled_returns_zero() {
        assert_eq!(
            validators::diff_gated_block_count(
                &diff_gated_context(),
                &HashSet::new(),
                &HashSet::from(["keep-sorted"])
            ),
            0
        );
    }
}
