mod affects;
mod check_ai;
mod keep_sorted;
mod keep_unique;
mod line_count;
mod line_pattern;

use crate::blocks::{Block, BlockSeverity, FileBlocks};
use crate::validators::affects::AffectsValidatorDetector;
use crate::validators::check_ai::CheckAiValidatorDetector;
use crate::validators::keep_sorted::KeepSortedValidatorDetector;
use crate::validators::keep_unique::KeepUniqueValidatorDetector;
use crate::validators::line_count::LineCountValidatorDetector;
use crate::validators::line_pattern::LinePatternValidatorDetector;
use async_trait::async_trait;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Validates the given `Context` and returns a list of the violations grouped by filename.
#[async_trait]
pub trait ValidatorAsync: Send + Sync {
    async fn validate(
        &self,
        context: Arc<ValidationContext>,
    ) -> anyhow::Result<HashMap<String, Vec<Violation>>>;
}

pub trait ValidatorSync: Send + Sync {
    fn validate(
        &self,
        context: Arc<ValidationContext>,
    ) -> anyhow::Result<HashMap<String, Vec<Violation>>>;
}

/// Detects a [`ValidatorType`] for the given `block` (if any).
///
/// This is used to determine whether an async runtime (e.g. Tokio) is needed to run the validators.
pub trait ValidatorDetector {
    fn detect(&self, block: &Block) -> anyhow::Result<Option<ValidatorType>>;
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
    block: Arc<Block>,
    data: Option<serde_json::Value>,
}

impl Violation {
    /// Constructs a new violation record with a name, error message, and optional machine-readable details.
    pub fn new(
        range: ViolationRange,
        code: String,
        message: String,
        block: Arc<Block>,
        data: Option<serde_json::Value>,
    ) -> Self {
        Self {
            range,
            code,
            message,
            block,
            data,
        }
    }

    pub fn as_simple_diagnostic(&self) -> anyhow::Result<SimpleDiagnostic<'_>> {
        Ok(SimpleDiagnostic {
            range: &self.range,
            code: self.code.as_str(),
            message: self.message.as_str(),
            severity: self.block.severity()?,
            data: &self.data,
        })
    }
}

#[derive(Serialize, Debug, PartialEq)]
pub struct ViolationRange {
    start: Position,
    end: Position,
}

impl ViolationRange {
    pub fn new(
        start_line: usize,
        start_character: usize,
        end_line: usize,
        end_character: usize,
    ) -> Self {
        Self {
            start: Position {
                line: start_line,
                character: start_character,
            },
            end: Position {
                line: end_line,
                character: end_character,
            },
        }
    }
}

#[derive(Serialize, Debug, PartialEq)]
struct Position {
    line: usize,
    character: usize,
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
    // Modified blocks with their corresponding source file contents grouped by filename.
    modified_blocks: HashMap<String, FileBlocks>,
}

impl ValidationContext {
    /// Creates a new validation context with modified blocks grouped by filename.
    pub fn new(modified_blocks: HashMap<String, FileBlocks>) -> Self {
        Self { modified_blocks }
    }
}

/// Runs all sync validators concurrently each in a separate thread and returns violations grouped
/// by file paths.
fn run_sync_validators(
    context: Arc<ValidationContext>,
    validators: Vec<Box<dyn ValidatorSync>>,
) -> anyhow::Result<HashMap<String, Vec<Violation>>> {
    let mut handles = Vec::new();
    for validator in validators {
        let context = Arc::clone(&context);
        handles.push(std::thread::spawn(move || validator.validate(context)));
    }

    let mut violations = HashMap::new();
    for handle in handles {
        let result = handle.join();
        match result {
            Ok(Ok(file_violations)) => {
                for (file_path, file_violations) in file_violations {
                    violations
                        .entry(file_path)
                        .or_insert_with(Vec::new)
                        .extend(file_violations);
                }
            }
            Ok(Err(e)) => return Err(e),
            Err(e) => return Err(anyhow::anyhow!("Failed to run validation: {e:?}")),
        }
    }

    Ok(violations)
}

/// Runs all async validators concurrently via Tokio and returns violations grouped by file paths.
fn run_async_validators(
    context: Arc<ValidationContext>,
    validators: Vec<Box<dyn ValidatorAsync>>,
) -> anyhow::Result<HashMap<String, Vec<Violation>>> {
    let tokio_runtime = tokio::runtime::Runtime::new()?;
    tokio_runtime.block_on(async move {
        let mut tasks = tokio::task::JoinSet::new();
        for validator in validators {
            let context = Arc::clone(&context);
            tasks.spawn(async move { validator.validate(context).await });
        }

        let mut violations = HashMap::new();
        while let Some(result) = tasks.join_next().await {
            match result {
                Ok(Ok(file_violations)) => {
                    for (file_path, file_violations) in file_violations {
                        violations
                            .entry(file_path)
                            .or_insert_with(Vec::new)
                            .extend(file_violations);
                    }
                }
                Ok(Err(e)) => return Err(e),
                Err(e) => return Err(anyhow::anyhow!("Failed to run validation: {e}")),
            }
        }

        Ok(violations)
    })
}

/// Run the given sync and async validators in separate threads in parallel.
pub fn run(
    context: Arc<ValidationContext>,
    sync_validators: Vec<Box<dyn ValidatorSync>>,
    async_validators: Vec<Box<dyn ValidatorAsync>>,
) -> anyhow::Result<HashMap<String, Vec<Violation>>> {
    if async_validators.is_empty() {
        return run_sync_validators(context, sync_validators);
    }
    // Run sync and async validators concurrently.
    let sync_context = Arc::clone(&context);
    let sync_violations_handle =
        std::thread::spawn(move || run_sync_validators(sync_context, sync_validators));
    let async_violations_handle =
        std::thread::spawn(move || run_async_validators(context, async_validators));
    let sync_violations_result = sync_violations_handle
        .join()
        .map_err(|e| anyhow::anyhow!("Failed to join sync violations thread: {e:?}"))?;
    let mut violations = sync_violations_result?;

    let async_violations_result = async_violations_handle
        .join()
        .map_err(|e| anyhow::anyhow!("Failed to join async violations thread: {e:?}"))?;
    let async_violations = async_violations_result?;

    for (file_path, file_violations) in async_violations {
        violations
            .entry(file_path)
            .or_insert_with(Vec::new)
            .extend(file_violations);
    }

    Ok(violations)
}

type SyncValidators = Vec<Box<dyn ValidatorSync>>;
type AsyncValidators = Vec<Box<dyn ValidatorAsync>>;

type DetectorFactory = fn() -> Box<dyn ValidatorDetector>;

pub const DETECTOR_FACTORIES: &[(&str, DetectorFactory)] = &[
    // <block affects="README.md:validators-list">
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
    // </block>
];

pub fn detect_validators(
    context: &ValidationContext,
    detectors: &[(&str, DetectorFactory)],
    disabled_validators: &HashSet<&str>,
) -> anyhow::Result<(SyncValidators, AsyncValidators)> {
    let mut validator_detectors: Vec<Box<dyn ValidatorDetector>> = detectors
        .iter()
        .filter(|(validator_name, _)| !disabled_validators.contains(*validator_name))
        .map(|(_, factory)| factory())
        .collect();
    let mut sync_validators = Vec::new();
    let mut async_validators = Vec::new();
    'outer: for file_blocks in context.modified_blocks.values() {
        for block in &file_blocks.blocks {
            let mut undetected = Vec::new();
            while let Some(detector) = validator_detectors.pop() {
                match detector.detect(block)? {
                    Some(ValidatorType::Sync(validator)) => {
                        sync_validators.push(validator);
                    }
                    Some(ValidatorType::Async(validator)) => {
                        async_validators.push(validator);
                    }
                    None => {
                        undetected.push(detector);
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

#[cfg(test)]
mod tests {
    use crate::blocks::{Block, FileBlocks};
    use crate::validators;
    use crate::validators::{
        DetectorFactory, ValidationContext, ValidatorAsync, ValidatorDetector, ValidatorSync,
        ValidatorType, Violation, ViolationRange, detect_validators,
    };
    use async_trait::async_trait;
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;

    fn empty_testing_block() -> Block {
        Block::new(0, 0, HashMap::new(), 0..0)
    }

    fn empty_testing_violation_range() -> ViolationRange {
        ViolationRange::new(0, 0, 0, 0)
    }

    struct FakeAsyncValidator {
        testing_block: Arc<Block>,
    }

    #[async_trait]
    impl ValidatorAsync for FakeAsyncValidator {
        async fn validate(
            &self,
            context: Arc<ValidationContext>,
        ) -> anyhow::Result<HashMap<String, Vec<Violation>>> {
            Ok(context
                .modified_blocks
                .keys()
                .map(|file_name| {
                    (
                        file_name.clone(),
                        vec![Violation::new(
                            empty_testing_violation_range(),
                            "fake-async".to_string(),
                            "fake-async error message".to_string(),
                            Arc::clone(&self.testing_block),
                            None,
                        )],
                    )
                })
                .collect())
        }
    }

    struct FakeSyncValidator {
        testing_block: Arc<Block>,
    }

    impl ValidatorSync for FakeSyncValidator {
        fn validate(
            &self,
            context: Arc<ValidationContext>,
        ) -> anyhow::Result<HashMap<String, Vec<Violation>>> {
            Ok(context
                .modified_blocks
                .keys()
                .map(|file_name| {
                    (
                        file_name.clone(),
                        vec![Violation::new(
                            empty_testing_violation_range(),
                            "fake-sync".to_string(),
                            "fake-sync error message".to_string(),
                            Arc::clone(&self.testing_block),
                            None,
                        )],
                    )
                })
                .collect())
        }
    }

    struct FakeAsyncValidatorDetector();

    impl ValidatorDetector for FakeAsyncValidatorDetector {
        fn detect(&self, block: &Block) -> anyhow::Result<Option<ValidatorType>> {
            if block.attributes.contains_key("fake-async") {
                Ok(Some(ValidatorType::Async(Box::new(FakeAsyncValidator {
                    testing_block: Arc::new(empty_testing_block()),
                }))))
            } else {
                Ok(None)
            }
        }
    }

    struct FakeSyncValidatorDetector();
    impl ValidatorDetector for FakeSyncValidatorDetector {
        fn detect(&self, block: &Block) -> anyhow::Result<Option<ValidatorType>> {
            if block.attributes.contains_key("fake-sync") {
                Ok(Some(ValidatorType::Sync(Box::new(FakeSyncValidator {
                    testing_block: Arc::new(empty_testing_block()),
                }))))
            } else {
                Ok(None)
            }
        }
    }

    const DETECTOR_FACTORIES: &[(&str, DetectorFactory)] = &[
        ("fake-sync", || Box::new(FakeSyncValidatorDetector {})),
        ("fake-async", || Box::new(FakeAsyncValidatorDetector {})),
    ];

    #[test]
    fn detect_and_run_with_sync_and_async_validators_returns_correct_violations()
    -> anyhow::Result<()> {
        let context = ValidationContext::new(HashMap::from([
            (
                "file1".to_string(),
                FileBlocks {
                    file_contents: "".to_string(),
                    blocks: vec![Arc::new(Block::new(
                        1,
                        6,
                        HashMap::from([
                            ("fake-sync".to_string(), "condition A".to_string()),
                            ("fake-async".to_string(), "condition B".to_string()),
                        ]),
                        0..0,
                    ))],
                },
            ),
            (
                "file2".to_string(),
                FileBlocks {
                    file_contents: "".to_string(),
                    blocks: vec![Arc::new(Block::new(
                        1,
                        6,
                        HashMap::from([
                            ("fake-sync".to_string(), "condition C".to_string()),
                            ("fake-async".to_string(), "condition D".to_string()),
                        ]),
                        0..0,
                    ))],
                },
            ),
        ]));

        let (sync_validators, async_validators) =
            detect_validators(&context, DETECTOR_FACTORIES, &HashSet::new())?;
        let violations = validators::run(Arc::new(context), sync_validators, async_validators)?;

        assert_eq!(violations.len(), 2);
        assert_eq!(violations["file1"].len(), 2);
        let mut file1_violations = violations["file1"]
            .iter()
            .map(|v| v.code.as_str())
            .collect::<Vec<_>>();
        file1_violations.sort();
        assert_eq!(file1_violations, vec!["fake-async", "fake-sync"]);
        assert_eq!(violations["file2"].len(), 2);
        let mut file2_violations = violations["file2"]
            .iter()
            .map(|v| v.code.as_str())
            .collect::<Vec<_>>();
        file2_violations.sort();
        assert_eq!(file2_violations, vec!["fake-async", "fake-sync"]);
        Ok(())
    }

    #[test]
    fn detect_and_run_with_sync_only_validators_returns_correct_violations() -> anyhow::Result<()> {
        let context = ValidationContext::new(HashMap::from([
            (
                "file1".to_string(),
                FileBlocks {
                    file_contents: "".to_string(),
                    blocks: vec![Arc::new(Block::new(
                        1,
                        6,
                        HashMap::from([("fake-sync".to_string(), "condition A".to_string())]),
                        0..0,
                    ))],
                },
            ),
            (
                "file2".to_string(),
                FileBlocks {
                    file_contents: "".to_string(),
                    blocks: vec![Arc::new(Block::new(
                        1,
                        6,
                        HashMap::from([("fake-sync".to_string(), "condition B".to_string())]),
                        0..0,
                    ))],
                },
            ),
        ]));

        let (sync_validators, async_validators) =
            detect_validators(&context, DETECTOR_FACTORIES, &HashSet::new())?;
        let violations = validators::run(Arc::new(context), sync_validators, async_validators)?;

        assert_eq!(violations.len(), 2);
        assert_eq!(violations["file1"].len(), 1);
        assert_eq!(violations["file1"][0].code, "fake-sync");
        assert_eq!(violations["file2"].len(), 1);
        assert_eq!(violations["file2"][0].code, "fake-sync");
        Ok(())
    }

    #[test]
    fn detect_and_run_with_async_only_validators_returns_correct_violations() -> anyhow::Result<()>
    {
        let context = ValidationContext::new(HashMap::from([
            (
                "file1".to_string(),
                FileBlocks {
                    file_contents: "".to_string(),
                    blocks: vec![Arc::new(Block::new(
                        1,
                        6,
                        HashMap::from([("fake-async".to_string(), "condition A".to_string())]),
                        0..0,
                    ))],
                },
            ),
            (
                "file2".to_string(),
                FileBlocks {
                    file_contents: "".to_string(),
                    blocks: vec![Arc::new(Block::new(
                        1,
                        6,
                        HashMap::from([("fake-async".to_string(), "condition B".to_string())]),
                        0..0,
                    ))],
                },
            ),
        ]));

        let (sync_validators, async_validators) =
            detect_validators(&context, DETECTOR_FACTORIES, &HashSet::new())?;
        let violations = validators::run(Arc::new(context), sync_validators, async_validators)?;

        assert_eq!(violations.len(), 2);
        assert_eq!(violations["file1"].len(), 1);
        assert_eq!(violations["file1"][0].code, "fake-async");
        assert_eq!(violations["file2"].len(), 1);
        assert_eq!(violations["file2"][0].code, "fake-async");
        Ok(())
    }

    #[test]
    fn detect_and_run_with_disabled_async_validators_returns_violations_for_sync_validators_only()
    -> anyhow::Result<()> {
        let context = ValidationContext::new(HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_contents: "".to_string(),
                blocks: vec![Arc::new(Block::new(
                    1,
                    6,
                    HashMap::from([
                        ("fake-sync".to_string(), "condition A".to_string()),
                        ("fake-async".to_string(), "condition B".to_string()),
                    ]),
                    0..0,
                ))],
            },
        )]));

        let (sync_validators, async_validators) =
            detect_validators(&context, DETECTOR_FACTORIES, &HashSet::from(["fake-async"]))?;
        let violations = validators::run(Arc::new(context), sync_validators, async_validators)?;

        assert_eq!(violations.len(), 1);
        assert_eq!(violations["file1"].len(), 1);
        assert_eq!(violations["file1"][0].code, "fake-sync");
        Ok(())
    }
}
