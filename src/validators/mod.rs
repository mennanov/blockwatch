mod affects;
mod check_ai;
mod check_lua;
mod keep_sorted;
mod keep_unique;
mod line_count;
mod line_pattern;
mod same_as;

use crate::Position;
use crate::blocks::{Block, BlockSeverity, BlockWithContext, FileBlocks, parser_for_file_path};
use crate::diff_parser::LineChange;
use crate::fs::FileSystem;
use crate::language_parsers::LanguageParsers;
use crate::repo_path::RepoPath;
use crate::symbol_path::SymbolPath;
use crate::symbols::{ResolveError, Symbol, resolve};
use crate::validators::affects::AffectsValidatorDetector;
use crate::validators::check_ai::CheckAiValidatorDetector;
use crate::validators::check_lua::CheckLuaValidatorDetector;
use crate::validators::keep_sorted::KeepSortedValidatorDetector;
use crate::validators::keep_unique::KeepUniqueValidatorDetector;
use crate::validators::line_count::LineCountValidatorDetector;
use crate::validators::line_pattern::LinePatternValidatorDetector;
use crate::validators::same_as::SameAsValidatorDetector;
use crate::violation_address::ViolationAddress;
use anyhow::{Context, bail};
use async_trait::async_trait;
use bigdecimal::BigDecimal;
use serde::Serialize;
use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ffi::OsString;
use std::ops::Range;
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
    /// A unique address of this violation.
    /// `None` if the block has no name as its address may not be unique.
    address: Option<ViolationAddress>,
    /// Whether `--suppress` matched this violation.
    suppressed: bool,
    /// Validator-specific details for tools, e.g. the expected and actual line count.
    data: Option<serde_json::Value>,
}

impl Violation {
    /// Constructs a new violation record with a name, error message, and optional machine-readable
    /// details.
    ///
    /// Severity and address are derived here from the block and the `code` rather than being passed
    /// in, so a validator cannot report a violation whose address disagrees with the rule that
    /// found it.
    ///
    /// `violation_text` is the text the violation is about — the offending line, or the target a
    /// reference names — and tells one violation of a block from its siblings. Validators that
    /// report at most one violation per block pass `None`.
    pub fn new(
        range: ViolationRange,
        file: &RepoPath,
        block: &Block,
        code: String,
        message: String,
        violation_text: Option<&str>,
        data: Option<serde_json::Value>,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            address: ViolationAddress::new(file, block.name(), &code, violation_text),
            severity: block.severity()?,
            range,
            code,
            message,
            suppressed: false,
            data,
        })
    }

    /// A unique address of this violation or `None` if its block is unnamed.
    pub fn address(&self) -> Option<&ViolationAddress> {
        self.address.as_ref()
    }

    /// Marks this violation as suppressed.
    pub fn suppress(&mut self) {
        self.suppressed = true;
    }

    pub fn as_simple_diagnostic(&self) -> SimpleDiagnostic<'_> {
        SimpleDiagnostic {
            range: &self.range,
            code: self.code.as_str(),
            message: self.message.as_str(),
            severity: self.severity,
            address: self.address.as_ref(),
            suppressed: self.suppressed,
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
            .push((file.clone(), block.start_tag_position_range.start.clone()));
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

/// The span an editor should highlight for a violation. 1-based and half-open: the start is
/// inclusive, the end is exclusive.
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

    /// The first position of the range.
    pub fn start(&self) -> &Position {
        &self.start
    }

    /// The position just past the last one of the range: the end is exclusive.
    pub fn end(&self) -> &Position {
        &self.end
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
    address: Option<&'a ViolationAddress>,
    #[serde(skip_serializing_if = "is_false")]
    suppressed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: &'a Option<serde_json::Value>,
}

/// Keeps a `false` `suppressed` field out of the serialized diagnostic.
fn is_false(value: &bool) -> bool {
    !*value
}

impl<'a> SimpleDiagnostic<'a> {
    /// Whether `--suppress` matched this violation, in which case it must not fail the run.
    pub fn is_suppressed(&self) -> bool {
        self.suppressed
    }

    /// The severity, which the caller uses to decide the process exit code.
    pub fn severity(&self) -> BlockSeverity {
        self.severity
    }

    /// Where to underline in the file.
    pub fn range(&self) -> &'a ViolationRange {
        self.range
    }

    /// The name of the validator that reported it, e.g. `"keep-sorted"`.
    pub fn code(&self) -> &'a str {
        self.code
    }

    /// The human-readable explanation.
    pub fn message(&self) -> &'a str {
        self.message
    }

    /// The address `--suppress` may point at, or `None` when the block has no `name`.
    pub fn address(&self) -> Option<&'a ViolationAddress> {
        self.address
    }

    /// Validator-specific details for tools.
    pub fn data(&self) -> &'a Option<serde_json::Value> {
        self.data
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
    let result = tokio_runtime.block_on(async move {
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
    });

    // Shutdown background tasks, if any.
    tokio_runtime.shutdown_background();
    result
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
        /* <block name="validator-registry" affects="README.md:available-validators"
        keep-unique='\("(?P<value>[^"]+)"'
           same-as="README.md:available-validators, docs/validators/README.md:validators-index"
           same-as-pattern='^\("(?P<value>[a-z-]+)"'> */
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

/// The names of every registered validator, in registry order.
pub fn validator_names() -> Vec<&'static str> {
    // The concrete filesystem is irrelevant here (only the names are read), so the `FileSystemImpl`
    // is used to avoid making every caller pass a type parameter.
    detector_factories::<crate::fs::FileSystemImpl>()
        .iter()
        .map(|(validator_name, _)| *validator_name)
        .collect()
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

/// One target named by a reference-based validator's attribute (`affects`, `check-lua`, `same-as`).
#[derive(Debug, PartialEq, Eq)]
pub(in crate::validators) enum TargetReference {
    /// A named block, in `file` or — when `file` is `None` — in the referencing file itself.
    Block {
        file: Option<RepoPath>,
        name: String,
    },
    /// A whole file. Nothing is parsed out of it, which is what lets a reference point at a format
    /// BlockWatch has no grammar for (`.env`, lockfiles, plain-text fixtures).
    File(RepoPath),
    /// A symbol, found by `path` in `file`, or in the referencing file itself when `file` is `None`.
    Symbol {
        file: Option<RepoPath>,
        path: SymbolPath,
    },
}

/// Parses a comma-separated list of the references shared by the reference-based validators
/// (`affects`, `check-lua`, `same-as`).
///
/// A reference can take one of three mutually exclusive forms:
/// - `file#/path` or `#/path`: addresses a symbol by its symbol path.
/// - `file:name` or `:name`: addresses a named block.
/// - `file`: addresses a whole file.
///
/// Combining `#` and `:` in the same reference is rejected. Colons within a symbol path
/// must be percent-encoded as `%3A`.
///
/// A reference whose block name or symbol path is empty (`file.rs:` or `file.json#`) is rejected
/// rather than treated as a whole-file reference, avoiding silently turning a syntax typo
/// into a much coarser rule.
pub(in crate::validators) fn parse_target_references(
    value: &str,
) -> anyhow::Result<Vec<TargetReference>> {
    value.split(',').map(parse_single_reference).collect()
}

/// Parses a single target reference string into a [`TargetReference`].
fn parse_single_reference(raw: &str) -> anyhow::Result<TargetReference> {
    let reference = raw.trim();
    if reference.is_empty() {
        bail!("Invalid target reference: \"{reference}\"");
    }
    if reference.contains('#') && reference.contains(':') {
        bail!(
            "Invalid target reference: \"{reference}\" combines '#' and ':'; \
             these forms are mutually exclusive"
        );
    }
    if let Some((file_part, _)) = reference.split_once('#') {
        return parse_symbol_reference(file_part, &reference[file_part.len()..], reference);
    }
    if let Some((file_part, name_part)) = reference.split_once(':') {
        return parse_block_reference(file_part, name_part, reference);
    }
    // Normalized so `./target.json` and `target.json` resolve to the same file.
    Ok(TargetReference::File(RepoPath::from_reference(reference)?))
}

/// Parses a symbol reference (`file#/path` or `#/path`).
fn parse_symbol_reference(
    file_part: &str,
    fragment: &str,
    reference: &str,
) -> anyhow::Result<TargetReference> {
    let file_part = file_part.trim();
    if fragment == "#" {
        if file_part.is_empty() {
            bail!("Invalid target reference: \"{reference}\"");
        }
        bail!(
            "Invalid target reference: \"{reference}\" has no symbol path after '#'; \
             drop the '#' to reference the whole file"
        );
    }
    let file = parse_optional_repo_path(file_part)?;
    let path = SymbolPath::parse(fragment)?;
    Ok(TargetReference::Symbol { file, path })
}

/// Parses a `:`-separated named block reference (`file:name` or `:name`).
fn parse_block_reference(
    file_part: &str,
    name_part: &str,
    reference: &str,
) -> anyhow::Result<TargetReference> {
    let file_part = file_part.trim();
    let name_part = name_part.trim();
    if name_part.is_empty() {
        bail!(
            "Invalid target reference: \"{reference}\" has no block name after ':'; \
             drop the ':' to reference the whole file"
        );
    }
    let file = parse_optional_repo_path(file_part)?;
    Ok(TargetReference::Block {
        file,
        name: name_part.to_string(),
    })
}

/// Parses a file path into an optional [`RepoPath`], returning `None` if the input is empty.
fn parse_optional_repo_path(file_part: &str) -> anyhow::Result<Option<RepoPath>> {
    if file_part.is_empty() {
        Ok(None)
    } else {
        // Normalized so `./target.py` and `target.py` resolve to the same file.
        Ok(Some(RepoPath::from_reference(file_part)?))
    }
}

/// How a message shows a reference's target: `file:name` for a block, `file#/path` for a symbol,
/// and `file <path>` for a whole file.
///
/// `name` is a block name, or a symbol path with its leading `#`. It is `None` for a whole file.
pub(in crate::validators) fn target_display(file: &RepoPath, name: Option<&str>) -> String {
    match name {
        // A symbol path keeps its `#`, which already separates it from the file.
        Some(name) if name.starts_with('#') => format!("{}{name}", file.display()),
        Some(name) => format!("{}:{name}", file.display()),
        None => format!("file {}", file.display()),
    }
}

/// Why a symbol reference does not resolve to exactly one symbol, as a short phrase such as
/// "symbol not found; did you mean: /version". The phrase does not include the reference itself.
///
/// `source` is the text of the file the symbols were derived from. It places each ambiguous
/// candidate at its line and column.
fn resolve_error_reason(error: &ResolveError, source: &str) -> String {
    match error {
        ResolveError::NotFound { hints } if hints.is_empty() => "symbol not found".to_string(),
        ResolveError::NotFound { hints } => {
            let hints: Vec<String> = hints.iter().map(|hint| hint.to_string()).collect();
            format!("symbol not found; did you mean: {}", hints.join(", "))
        }
        ResolveError::Ambiguous { candidates } => {
            // The candidates share one path, and can share a line too, so only a line and a
            // column tell them apart. Where a candidate is written first is enough for that.
            let positions: Vec<String> = candidates
                .iter()
                .map(|candidate| {
                    let ranges = candidate.position_ranges(source);
                    let start = &ranges[0].start;
                    format!("{}:{}", start.line, start.character)
                })
                .collect();
            format!("ambiguous symbol, defined at {}", positions.join(", "))
        }
    }
}

/// What a reference's target resolves to, or the reason it does not resolve, such as a missing
/// block or a missing symbol. The reason is reported as the reference's violation. A failure that
/// ends the run is the `Err` of an enclosing `anyhow::Result` instead.
pub(in crate::validators) type TargetResult<T> = Result<T, String>;

/// The text and the symbols of the files that references point into, each read and derived at most
/// once.
pub(in crate::validators) struct TargetFiles<'a, Fs: FileSystem> {
    context: &'a ValidationContext,
    /// Reads the target files that are not in `context`.
    file_system: &'a Fs,
    /// The text of the target files that are not in `context`.
    contents: HashMap<RepoPath, String>,
    /// The symbols of every file a symbol reference points into.
    symbols: HashMap<RepoPath, Vec<Symbol>>,
}

impl<'a, Fs: FileSystem> TargetFiles<'a, Fs> {
    pub(in crate::validators) fn new(context: &'a ValidationContext, file_system: &'a Fs) -> Self {
        Self {
            context,
            file_system,
            contents: HashMap::new(),
            symbols: HashMap::new(),
        }
    }

    /// The text of `file`: from the validation context when the file is in it, and otherwise read
    /// through the file system.
    ///
    /// # Errors
    /// Returns an error if the file is not in the context and cannot be read.
    pub(in crate::validators) fn content(&mut self, file: &RepoPath) -> anyhow::Result<&str> {
        file_content(self.context, self.file_system, &mut self.contents, file)
    }

    /// Resolves `path` among the symbols of `file`. Returns the text of `file`, together with the
    /// symbol or with the reason why no single symbol has `path`.
    ///
    /// # Errors
    /// Returns an error that shows the reference if `file` cannot be read, has no grammar, is in a
    /// language without symbols, or does not parse.
    pub(in crate::validators) fn resolve_symbol(
        &mut self,
        file: &RepoPath,
        path: &SymbolPath,
    ) -> anyhow::Result<(&str, TargetResult<&Symbol>)> {
        let (content, symbols) = self.content_and_symbols(file).with_context(|| {
            format!(
                "failed to resolve symbol reference {}#{path}",
                file.display()
            )
        })?;
        let resolution =
            resolve(symbols, path).map_err(|error| resolve_error_reason(&error, content));
        Ok((content, resolution))
    }

    /// The text of `file` and the symbols derived from it.
    fn content_and_symbols(&mut self, file: &RepoPath) -> anyhow::Result<(&str, &[Symbol])> {
        let content = file_content(self.context, self.file_system, &mut self.contents, file)?;
        let symbols = match self.symbols.entry(file.clone()) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let parser = parser_for_file_path(
                    file.as_path(),
                    self.context.parsers(),
                    self.context.extra_file_extensions(),
                )
                .context("file format is unsupported")?;
                let symbols = parser
                    .lock()
                    .expect("no active locks")
                    .parse_symbols(content)?;
                entry.insert(symbols)
            }
        };
        Ok((content, symbols))
    }
}

/// The text of `file`: from `context` when the file is in it, and otherwise read through
/// `file_system` once and kept in `contents`.
///
/// # Errors
/// Returns an error if the file is not in `context` and cannot be read.
fn file_content<'c, Fs: FileSystem>(
    context: &'c ValidationContext,
    file_system: &Fs,
    contents: &'c mut HashMap<RepoPath, String>,
    file: &RepoPath,
) -> anyhow::Result<&'c str> {
    if let Some(file_blocks) = context.blocks.get(file) {
        return Ok(&file_blocks.file_content);
    }
    match contents.entry(file.clone()) {
        Entry::Occupied(entry) => Ok(entry.into_mut()),
        Entry::Vacant(entry) => Ok(entry.insert(file_system.read_to_string(file.as_path())?)),
    }
}

/// Returns the captured named regexp group `value`, or the whole match if there is no named group.
pub(in crate::validators) fn value_match<'h>(
    captures: &regex::Captures<'h>,
) -> Option<regex::Match<'h>> {
    captures.name("value").or_else(|| captures.get(0))
}

/// Returns the non-empty trimmed content of `line` together with its 0-based character-column
/// range within `line`, or `None` for a blank line. The range is measured in characters rather than
/// bytes and is half-open: `[start, end)`.
pub(in crate::validators) fn trimmed_line_value(line: &str) -> Option<(&str, Range<usize>)> {
    let trimmed_line = line.trim();
    if trimmed_line.is_empty() {
        None
    } else {
        let byte_offset = trimmed_line.as_ptr() as usize - line.as_ptr() as usize;
        let start = line[..byte_offset].chars().count();
        let end = start + trimmed_line.chars().count();
        Some((trimmed_line, start..end))
    }
}

/// Returns the substring `regex` selects from `line` (see [`value_match`]) together with its
/// 0-based character-column range `[start, end)`, or `None` when the line yields no value to compare.
///
/// The regex is applied to the trimmed line so that anchors such as `^` and `$` refer to the entry
/// itself rather than to its indentation, which is what makes a pattern keep working once the
/// entries are nested inside a list or a block of code.
///
/// A blank line, an unmatched line, and a line whose match is empty all return `None`: none of them
/// carry a value.
pub(in crate::validators) fn regex_value<'a>(
    line: &'a str,
    regex: &regex::Regex,
) -> Option<(&'a str, Range<usize>)> {
    let trimmed_line = line.trim();
    if trimmed_line.is_empty() {
        return None;
    }
    let caps = regex.captures(trimmed_line)?;
    let m = value_match(&caps)?;
    if m.is_empty() {
        return None;
    }
    let trimmed_byte_offset = trimmed_line.as_ptr() as usize - line.as_ptr() as usize;
    let start = line[..trimmed_byte_offset + m.start()].chars().count();
    let end = start + m.as_str().chars().count();
    Some((m.as_str(), start..end))
}

/// The content for the `*-pattern` attribute extracted from the block.
pub(in crate::validators) enum PatternContent<'c> {
    /// No `*-pattern` attribute: the block's whole content (trimmed).
    Whole(&'c str),
    /// A `*-pattern` attribute: the value of every match, in the order they appear in the block.
    Matches(Vec<&'c str>),
}

/// Returns the content for the `*-pattern` attribute, e.g. `check-ai-pattern` for `check-ai`.
pub(in crate::validators) fn block_content_for_pattern<'c>(
    block_with_context: &BlockWithContext,
    file_content: &'c str,
    pattern_attribute: &str,
) -> anyhow::Result<PatternContent<'c>> {
    let content = block_with_context.block.content(file_content);
    let Some(pattern) = block_with_context.block.attributes.get(pattern_attribute) else {
        return Ok(PatternContent::Whole(content.trim()));
    };
    let re = regex::Regex::new(pattern)
        .with_context(|| format!("{pattern_attribute} is not a valid regex"))?;
    Ok(PatternContent::Matches(
        re.captures_iter(content)
            .filter_map(|captures| value_match(&captures).map(|matched| matched.as_str()))
            .filter(|value| !value.is_empty())
            .collect(),
    ))
}

/// Parses a numeric value from string.
///
/// Supports very big numbers (bigger than `f64` may hold) and `_` digits separator.
pub(in crate::validators) fn parse_number(value: &str) -> Option<BigDecimal> {
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
mod parse_target_references_tests {
    use crate::repo_path::RepoPath;
    use crate::symbol_path::SymbolPath;
    use crate::validators::{TargetReference, parse_target_references};

    /// A `file:name` reference, for the expected values below.
    fn block(file: &str, name: &str) -> anyhow::Result<TargetReference> {
        Ok(TargetReference::Block {
            file: Some(RepoPath::from_reference(file)?),
            name: name.to_string(),
        })
    }

    /// A `file#path` or `#/path` reference, for the expected values below.
    fn path(file: Option<&str>, fragment: &str) -> anyhow::Result<TargetReference> {
        Ok(TargetReference::Symbol {
            file: file.map(RepoPath::from_reference).transpose()?,
            path: SymbolPath::parse(fragment)?,
        })
    }

    #[test]
    fn single_reference() -> anyhow::Result<()> {
        let result = parse_target_references("file.rs:block_name")?;
        assert_eq!(result, vec![block("file.rs", "block_name")?]);
        Ok(())
    }

    #[test]
    fn multiple_references() -> anyhow::Result<()> {
        let result = parse_target_references("file1.rs:block1, file2.rs:block2")?;
        assert_eq!(
            result,
            vec![block("file1.rs", "block1")?, block("file2.rs", "block2")?]
        );
        Ok(())
    }

    #[test]
    fn empty_filename_returns_none_for_filename() -> anyhow::Result<()> {
        let result = parse_target_references(":block_name")?;
        assert_eq!(
            result,
            vec![TargetReference::Block {
                file: None,
                name: "block_name".to_string()
            }]
        );
        Ok(())
    }

    #[test]
    fn multiple_empty_filename_references_returns_non_for_filename() -> anyhow::Result<()> {
        let result = parse_target_references(":block1, :block2")?;
        assert_eq!(
            result,
            vec![
                TargetReference::Block {
                    file: None,
                    name: "block1".to_string()
                },
                TargetReference::Block {
                    file: None,
                    name: "block2".to_string()
                }
            ]
        );
        Ok(())
    }

    #[test]
    fn a_reference_without_a_colon_is_a_whole_file_reference() -> anyhow::Result<()> {
        let result = parse_target_references("config/schema.json")?;
        assert_eq!(
            result,
            vec![TargetReference::File(RepoPath::from_reference(
                "config/schema.json"
            )?)]
        );
        Ok(())
    }

    #[test]
    fn whole_file_and_block_references_can_be_mixed() -> anyhow::Result<()> {
        let result = parse_target_references("src/lib.rs:languages-code, locales/en.json")?;
        assert_eq!(
            result,
            vec![
                block("src/lib.rs", "languages-code")?,
                TargetReference::File(RepoPath::from_reference("locales/en.json")?)
            ]
        );
        Ok(())
    }

    #[test]
    fn a_whole_file_reference_is_normalized() -> anyhow::Result<()> {
        assert_eq!(
            parse_target_references("./config.json")?,
            parse_target_references("config.json")?
        );
        Ok(())
    }

    #[test]
    fn a_reference_with_an_empty_block_name_returns_error() {
        let err = parse_target_references("config.json:").unwrap_err();
        assert_eq!(
            err.to_string(),
            "Invalid target reference: \"config.json:\" has no block name after ':'; \
             drop the ':' to reference the whole file"
        );
    }

    #[test]
    fn an_empty_reference_returns_error() {
        assert!(parse_target_references("").is_err());
        assert!(parse_target_references("file.rs:block, ").is_err());
    }

    #[test]
    fn path_reference_with_file_parses_successfully() -> anyhow::Result<()> {
        let result = parse_target_references("package.json#/dependencies/inngest")?;
        assert_eq!(
            result,
            vec![path(Some("package.json"), "/dependencies/inngest")?]
        );
        Ok(())
    }

    #[test]
    fn path_reference_without_file_parses_successfully() -> anyhow::Result<()> {
        let result = parse_target_references("#/dependencies/inngest")?;
        assert_eq!(result, vec![path(None, "/dependencies/inngest")?]);
        Ok(())
    }

    #[test]
    fn mixed_reference_types_parse_successfully() -> anyhow::Result<()> {
        let result =
            parse_target_references("package.json#/version, docs/cli.md:version, README.md")?;
        assert_eq!(
            result,
            vec![
                path(Some("package.json"), "/version")?,
                block("docs/cli.md", "version")?,
                TargetReference::File(RepoPath::from_reference("README.md")?),
            ]
        );
        Ok(())
    }

    #[test]
    fn percent_encoded_colon_in_path_reference_parses_successfully() -> anyhow::Result<()> {
        let result = parse_target_references("package.json#/%3A")?;
        assert_eq!(result, vec![path(Some("package.json"), "/:")?]);
        Ok(())
    }

    #[test]
    fn reference_combining_hash_and_colon_returns_error() {
        let err1 = parse_target_references("package.json#/foo:bar").unwrap_err();
        assert!(
            err1.to_string().contains("combines '#' and ':'"),
            "unexpected error message: {err1}"
        );

        let err2 = parse_target_references("file.rs:block#/foo").unwrap_err();
        assert!(
            err2.to_string().contains("combines '#' and ':'"),
            "unexpected error message: {err2}"
        );
    }

    #[test]
    fn unrooted_path_reference_returns_error() {
        let err = parse_target_references("package.json#dependencies/inngest").unwrap_err();
        assert!(
            err.to_string().contains("symbol path must start with '/'"),
            "unexpected error message: {err}"
        );
    }

    #[test]
    fn reference_with_empty_path_returns_error() {
        let err = parse_target_references("package.json#").unwrap_err();
        assert_eq!(
            err.to_string(),
            "Invalid target reference: \"package.json#\" has no symbol path after '#'; \
             drop the '#' to reference the whole file"
        );
        assert!(parse_target_references("#").is_err());
    }
}

#[cfg(test)]
mod resolve_error_reason_tests {
    use super::*;
    use crate::symbols::Symbol;

    fn path(text: &str) -> SymbolPath {
        SymbolPath::parse(text).expect("the path is valid")
    }

    /// A symbol at `path_text` written once, at `def_byte_range` of its source.
    fn symbol(path_text: &str, def_byte_range: Range<usize>) -> Symbol {
        Symbol {
            path: path(path_text),
            def_byte_ranges: vec![def_byte_range],
            value: None,
        }
    }

    #[test]
    fn duplicate_keys_on_one_line_lists_each_line_and_column() {
        let source = r#"{"v": 1, "v": 2}"#;
        let symbols = [symbol("/v", 1..7), symbol("/v", 9..15)];
        let error = ResolveError::Ambiguous {
            candidates: vec![&symbols[0], &symbols[1]],
        };

        assert_eq!(
            resolve_error_reason(&error, source),
            "ambiguous symbol, defined at 1:2, 1:10"
        );
    }

    #[test]
    fn missing_path_with_hints_lists_them_as_written() {
        let hints = [
            path("/dependencies/@types~1node"),
            path("/dependencies/zod"),
        ];
        let error = ResolveError::NotFound {
            hints: vec![&hints[0], &hints[1]],
        };

        assert_eq!(
            resolve_error_reason(&error, ""),
            "symbol not found; did you mean: /dependencies/@types~1node, /dependencies/zod"
        );
    }

    #[test]
    fn missing_path_without_hints_says_only_that() {
        let error = ResolveError::NotFound { hints: vec![] };

        assert_eq!(resolve_error_reason(&error, ""), "symbol not found");
    }
}

#[cfg(test)]
mod target_files_tests {
    use super::*;
    use crate::fs::test_utils::FakeFileSystem;
    use crate::test_utils::validation_context;

    /// The error of resolving `path` in `file`, a file on the disk that holds `content`.
    fn resolve_symbol_error(file: &str, content: &str, path: &str) -> anyhow::Result<String> {
        let context = validation_context("referencing.py", "");
        let file_system =
            FakeFileSystem::new(HashMap::from([(file.to_string(), content.to_string())]));
        let mut files = TargetFiles::new(&context, &file_system);
        let err = files
            .resolve_symbol(&RepoPath::from_reference(file)?, &SymbolPath::parse(path)?)
            .unwrap_err();
        Ok(format!("{err:#}"))
    }

    #[test]
    fn file_without_a_grammar_resolve_symbol_returns_an_error_with_the_reference()
    -> anyhow::Result<()> {
        assert_eq!(
            resolve_symbol_error("VERSION", "1.2.3\n", "/x")?,
            "failed to resolve symbol reference VERSION#/x: file format is unsupported"
        );
        Ok(())
    }

    #[test]
    fn language_without_symbols_resolve_symbol_returns_an_error_with_the_reference()
    -> anyhow::Result<()> {
        let message = resolve_symbol_error("config.py", "x = 1\n", "/x")?;
        assert!(
            message.starts_with("failed to resolve symbol reference config.py#/x: "),
            "unexpected message: {message}"
        );
        Ok(())
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
        Block {
            attributes: HashMap::new(),
            start_tag_position_range: Position::new(0, 0)..Position::new(0, 0),
            content_bytes_range: 0..0,
            content_position_range: Position::new(0, 0)..Position::new(0, 0),
        }
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
                            vec![
                                Violation::new(
                                    empty_testing_violation_range(),
                                    file_name,
                                    &self.testing_block,
                                    "check-ai".to_string(),
                                    "check-ai error message".to_string(),
                                    None,
                                    None,
                                )
                                .expect("the testing block has a valid severity"),
                            ],
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
                            vec![
                                Violation::new(
                                    empty_testing_violation_range(),
                                    file_name,
                                    &self.testing_block,
                                    "keep-sorted".to_string(),
                                    "keep-sorted error message".to_string(),
                                    None,
                                    None,
                                )
                                .expect("the testing block has a valid severity"),
                            ],
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
