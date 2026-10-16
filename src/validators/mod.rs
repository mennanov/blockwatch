mod affects;
mod check_ai;
mod check_lua;
mod keep_sorted;
mod keep_unique;
mod line_count;
mod line_pattern;
mod same_as;

use crate::Position;
use crate::blocks::{
    self, Block, BlockAddress, BlockKey, BlockSeverity, BlockWithContext, FileBlocks, every_block,
    parser_for_file_path,
};
use crate::diff_parser::LineChange;
use crate::fs::FileSystem;
use crate::language_parsers::LanguageParsers;
use crate::repo_path::RepoPath;
use crate::settings::BlockSelection;
use crate::symbol_path::SymbolPath;
use crate::symbols::{Symbol, resolve};
use crate::violation_address::ViolationAddress;
use crate::virtual_blocks::VirtualBlock;
use anyhow::{Context, bail};
use async_trait::async_trait;
use bigdecimal::BigDecimal;
use serde::Serialize;
use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ffi::OsString;
use std::ops::Range;
use std::sync::Arc;
use strum::IntoEnumIterator;
use strum_macros::{Display, EnumIter, EnumString, IntoStaticStr};

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

/// Validator type (sync or async).
pub(crate) enum ValidatorType {
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

    /// Creates this validator if `block_with_context` carries one of its attributes. Returns
    /// `None` otherwise.
    ///
    /// The `match` has no `_` arm, so a new validator can't be left out. The result says whether
    /// the validator is sync or async, which lets the caller start a Tokio runtime only when an
    /// async validator exists.
    ///
    /// Returns an error when the validator can't be set up, such as `check-lua` with an unknown
    /// `BLOCKWATCH_LUA_MODE`.
    pub(crate) fn detect<Fs: FileSystem + 'static>(
        self,
        block_with_context: &BlockWithContext,
        file_system: &Arc<Fs>,
    ) -> anyhow::Result<Option<ValidatorType>> {
        match self {
            Validator::Affects => Ok(affects::detect(block_with_context, file_system)),
            Validator::CheckAi => Ok(check_ai::detect(block_with_context)),
            Validator::CheckLua => check_lua::detect(block_with_context, file_system),
            Validator::KeepSorted => Ok(keep_sorted::detect(block_with_context)),
            Validator::KeepUnique => Ok(keep_unique::detect(block_with_context)),
            Validator::LineCount => Ok(line_count::detect(block_with_context)),
            Validator::LinePattern => Ok(line_pattern::detect(block_with_context)),
            Validator::SameAs => Ok(same_as::detect(block_with_context, file_system)),
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
                "Unknown validator: {value}. Available validators: {}",
                validators.join(", ")
            )
        })
    }
}

/// One rule breach found in one block: what is wrong, where, and how severe it is.
///
/// Fields are private because the public shape of a violation is [`SimpleDiagnostic`], the form
/// that gets serialized for editors and CI.
#[derive(Debug)]
pub(crate) struct Violation {
    /// Where to underline in the file — normally the block's start tag, or the offending line.
    range: ViolationRange,
    /// The validator that reported it, e.g. [`Validator::KeepSorted`].
    code: Validator,
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
    /// Constructs a new violation record with a validator, error message, and optional
    /// machine-readable details.
    ///
    /// Severity and address are derived here from the block and the `code` rather than being passed
    /// in, so a validator cannot report a violation whose address disagrees with the rule that
    /// found it.
    ///
    /// `violation_text` is the text the violation is about — the offending line, or the target a
    /// reference names — and tells one violation of a block from its siblings. Validators that
    /// report at most one violation per block pass `None`.
    pub(crate) fn new(
        range: ViolationRange,
        file: &RepoPath,
        block: &Block,
        code: Validator,
        message: String,
        violation_text: Option<&str>,
        data: Option<serde_json::Value>,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            address: ViolationAddress::new(file, block.name(), code, violation_text),
            severity: block.severity()?,
            range,
            code,
            message,
            suppressed: false,
            data,
        })
    }

    /// A unique address of this violation or `None` if its block is unnamed.
    pub(crate) fn address(&self) -> Option<&ViolationAddress> {
        self.address.as_ref()
    }

    /// Marks this violation as suppressed.
    pub(crate) fn suppress(&mut self) {
        self.suppressed = true;
    }

    pub(crate) fn as_simple_diagnostic(&self) -> SimpleDiagnostic<'_> {
        SimpleDiagnostic {
            range: &self.range,
            code: self.code,
            message: self.message.as_str(),
            severity: self.severity,
            address: self.address.as_ref(),
            suppressed: self.suppressed,
            data: self.data.as_ref(),
        }
    }
}

/// What a single validator found, and which blocks it looked at.
///
/// A block is identified by its [`BlockKey`], because a block may have no name.
#[derive(Debug, Default)]
pub(crate) struct ValidationReport {
    /// Violations grouped by the file they were found in. A file without violations has no entry,
    /// so the map is empty when the validator found nothing.
    violations: HashMap<RepoPath, Vec<Violation>>,
    /// The key of every block this validator checked.
    checked_blocks: Vec<(RepoPath, BlockKey)>,
}

impl ValidationReport {
    /// Adds a checked block together with the violations found in it.
    pub(crate) fn add_all(&mut self, file: &RepoPath, block: &Block, violations: Vec<Violation>) {
        self.add_checked_block(file, block);
        self.add_violations(file, violations);
    }

    /// Adds `block` in `file` to the blocks this validator checked.
    fn add_checked_block(&mut self, file: &RepoPath, block: &Block) {
        self.checked_blocks.push((file.clone(), block.key()));
    }

    /// Stores violations found in `file`.
    fn add_violations(&mut self, file: &RepoPath, violations: Vec<Violation>) {
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
    /// Violations grouped by the file they were found in. BTreeMap is used so every output lists
    /// the files in the same order on every run.
    pub(crate) violations: BTreeMap<RepoPath, Vec<Violation>>,
    /// Every block that was examined, and by which validators. BTreeMap is used so the
    /// `--verbosity` report comes out in a stable order regardless of the validators order.
    pub(crate) checked_blocks: BTreeMap<RepoPath, BTreeMap<BlockKey, BTreeSet<Validator>>>,
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
        for (file, violations) in report.violations {
            self.violations.entry(file).or_default().extend(violations);
        }
    }

    /// Adds every checked block and violation from `other` to this log.
    fn merge(&mut self, other: ValidationLog) {
        for (file, blocks) in other.checked_blocks {
            let checked_in_file = self.checked_blocks.entry(file).or_default();
            for (key, validators) in blocks {
                checked_in_file.entry(key).or_default().extend(validators);
            }
        }
        for (file, violations) in other.violations {
            self.violations.entry(file).or_default().extend(violations);
        }
    }
}

/// The span an editor should highlight for a violation. 1-based and half-open: the start is
/// inclusive, the end is exclusive.
#[derive(Serialize, Debug, PartialEq, Eq)]
pub(crate) struct ViolationRange {
    start: Position,
    end: Position,
}

impl ViolationRange {
    /// Creates a range from its two endpoints, which must be in the same file.
    pub(crate) fn new(start: Position, end: Position) -> Self {
        Self { start, end }
    }

    /// The first position of the range.
    pub(crate) fn start(&self) -> &Position {
        &self.start
    }

    /// The position just past the last one of the range: the end is exclusive.
    pub(crate) fn end(&self) -> &Position {
        &self.end
    }
}

impl From<Range<Position>> for ViolationRange {
    fn from(range: Range<Position>) -> Self {
        Self::new(range.start, range.end)
    }
}

/// Represents a simplified, serializable diagnostic message.
///
/// It mimics the [Diagnostic](https://github.com/microsoft/vscode-languageserver-node/blob/3412a17149850f445bf35b4ad71148cfe5f8411e/types/src/main.ts#L688)
/// object but omits some redundant fields and keeps all line numbers 1-based instead of zero-based.
#[derive(Serialize, Debug)]
pub(crate) struct SimpleDiagnostic<'a> {
    range: &'a ViolationRange,
    code: Validator,
    message: &'a str,
    severity: BlockSeverity,
    #[serde(skip_serializing_if = "Option::is_none")]
    address: Option<&'a ViolationAddress>,
    #[serde(skip_serializing_if = "is_false")]
    suppressed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<&'a serde_json::Value>,
}

/// Keeps a `false` `suppressed` field out of the serialized diagnostic.
fn is_false(value: &bool) -> bool {
    !*value
}

impl<'a> SimpleDiagnostic<'a> {
    /// Whether `--suppress` matched this violation, in which case it must not fail the run.
    pub(crate) fn is_suppressed(&self) -> bool {
        self.suppressed
    }

    /// The severity, which the caller uses to decide the process exit code.
    pub(crate) fn severity(&self) -> BlockSeverity {
        self.severity
    }

    /// Where to underline in the file.
    pub(crate) fn range(&self) -> &'a ViolationRange {
        self.range
    }

    /// The validator that reported it, e.g. [`Validator::KeepSorted`].
    pub(crate) fn code(&self) -> Validator {
        self.code
    }

    /// The human-readable explanation.
    pub(crate) fn message(&self) -> &'a str {
        self.message
    }

    /// The address `--suppress` may point at, or `None` when the block has no `name`.
    pub(crate) fn address(&self) -> Option<&'a ViolationAddress> {
        self.address
    }

    /// Validator-specific details for tools.
    pub(crate) fn data(&self) -> Option<&'a serde_json::Value> {
        self.data
    }
}

/// Everything the validators are given to work with, shared read-only across all of them.
///
/// Built once per run and handed out as an `Arc`, because validators run concurrently on separate
/// threads and each needs the whole picture: a rule such as `affects` has to see blocks in files
/// other than the one it started from.
pub(crate) struct ValidationContext {
    /// Blocks with their corresponding source file contents grouped by filename.
    blocks: HashMap<RepoPath, FileBlocks>,
    /// Language parsers per file type, used by validators to parse referenced source files.
    parsers: LanguageParsers,
    /// Every line change from the diff (if any).
    line_changes: HashMap<RepoPath, Vec<LineChange>>,
    /// Extension remappings from the command line.
    extra_file_extensions: HashMap<OsString, OsString>,
    /// The declarations of every virtual block in the config file, in its order. Unlike `blocks`,
    /// they are not resolved, and they cover the files the run did not keep too.
    virtual_blocks: Vec<VirtualBlock>,
}

impl ValidationContext {
    /// Creates a new validation context with modified blocks grouped by filename.
    pub(crate) fn new(
        blocks: HashMap<RepoPath, FileBlocks>,
        parsers: LanguageParsers,
        line_changes: HashMap<RepoPath, Vec<LineChange>>,
        extra_file_extensions: HashMap<OsString, OsString>,
        virtual_blocks: Vec<VirtualBlock>,
    ) -> Self {
        Self {
            blocks,
            parsers,
            line_changes,
            extra_file_extensions,
            virtual_blocks,
        }
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
    fn parse_file(
        &self,
        file_system: &impl FileSystem,
        file_path: &RepoPath,
    ) -> anyhow::Result<Option<FileBlocks>> {
        let virtual_blocks: Vec<&VirtualBlock> = self
            .virtual_blocks
            .iter()
            .filter(|virtual_block| virtual_block.file == *file_path)
            .collect();
        blocks::parse_file(
            file_system,
            file_path.as_path(),
            self.line_changes_for(file_path).unwrap_or(&[]),
            every_block,
            &self.parsers,
            &self.extra_file_extensions,
            &virtual_blocks,
        )
    }

    /// The line changes the diff reported for the `file_path`.
    fn line_changes_for(&self, file_path: &RepoPath) -> Option<&[LineChange]> {
        self.line_changes.get(file_path).map(Vec::as_slice)
    }

    /// Converts the validation context to a serializable report that can be displayed as JSON,
    /// with the files sorted by path.
    pub(crate) fn to_serializable_report(&self) -> BTreeMap<RepoPath, Vec<serde_json::Value>> {
        let mut report = BTreeMap::new();
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
///
/// The violations of each file are sorted by where they start, then by validator name, then by
/// message.
pub(crate) fn run(
    context: Arc<ValidationContext>,
    sync_validators: SyncValidators,
    async_validators: AsyncValidators,
) -> anyhow::Result<ValidationLog> {
    let mut log = if async_validators.is_empty() {
        run_sync_validators(context, sync_validators)?
    } else {
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
        log
    };
    // Violations arrive grouped by validator, in an order that changes between runs.
    for violations in log.violations.values_mut() {
        violations.sort_by(|a, b| {
            (&a.range.start, &a.code, &a.message).cmp(&(&b.range.start, &b.code, &b.message))
        });
    }
    Ok(log)
}

/// Detected validators, each paired with the validator it represents.
type SyncValidators = Vec<(Validator, Box<dyn SyncValidator>)>;
type AsyncValidators = Vec<(Validator, Box<dyn AsyncValidator>)>;

/// Whether a validator is enabled/disabled.
fn is_validator_active(
    validator: Validator,
    disabled_validators: &HashSet<Validator>,
    enabled_validators: &HashSet<Validator>,
) -> bool {
    if enabled_validators.is_empty() {
        !disabled_validators.contains(&validator)
    } else {
        enabled_validators.contains(&validator)
    }
}

/// Counts the blocks in `context` carrying a rule that cannot fire unless a diff is supplied; see
/// [`Validator::needs_diff`].
pub(crate) fn diff_gated_block_count(
    context: &ValidationContext,
    disabled_validators: &HashSet<Validator>,
    enabled_validators: &HashSet<Validator>,
) -> usize {
    let attributes: Vec<&str> = Validator::iter()
        .filter(|validator| {
            validator.needs_diff()
                && is_validator_active(*validator, disabled_validators, enabled_validators)
        })
        .map(|validator| validator.as_str())
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
///
/// `detect` decides whether a validator applies to a block, as [`Validator::detect`] does. It is a
/// parameter so that a test can use validators that don't call a model or read the environment.
pub(crate) fn detect_validators<Fs: FileSystem + 'static>(
    context: &ValidationContext,
    detect: impl Fn(Validator, &BlockWithContext, &Arc<Fs>) -> anyhow::Result<Option<ValidatorType>>,
    disabled_validators: &HashSet<Validator>,
    enabled_validators: &HashSet<Validator>,
    file_system: &Arc<Fs>,
) -> anyhow::Result<(SyncValidators, AsyncValidators)> {
    let mut undetected: Vec<Validator> = Validator::iter()
        .filter(|validator| {
            is_validator_active(*validator, disabled_validators, enabled_validators)
        })
        .collect();
    let mut sync_validators = Vec::new();
    let mut async_validators = Vec::new();
    'outer: for file_blocks in context.blocks.values() {
        for block in &file_blocks.blocks_with_context {
            let mut still_undetected = Vec::new();
            for validator in undetected {
                match detect(validator, block, file_system)? {
                    Some(ValidatorType::Sync(sync_validator)) => {
                        sync_validators.push((validator, sync_validator));
                    }
                    Some(ValidatorType::Async(async_validator)) => {
                        async_validators.push((validator, async_validator));
                    }
                    None => still_undetected.push(validator),
                }
            }
            undetected = still_undetected;
            if undetected.is_empty() {
                // All validators have been detected.
                break 'outer;
            }
        }
    }
    Ok((sync_validators, async_validators))
}

/// One target named by a reference-based validator's attribute (`affects`, `check-lua`, `same-as`).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TargetReference {
    /// A named block, in `file` or — when `file` is `None` — in the referencing file itself.
    Block {
        file: Option<RepoPath>,
        name: String,
    },
    /// A whole file. Nothing is parsed out of it, which is what lets a reference point at a format
    /// blockwatch has no grammar for (`.env`, lockfiles, plain-text fixtures).
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
fn parse_target_references(value: &str) -> anyhow::Result<Vec<TargetReference>> {
    value.split(',').map(parse_single_reference).collect()
}

/// Parses a single target reference string into a [`TargetReference`].
pub(crate) fn parse_single_reference(raw: &str) -> anyhow::Result<TargetReference> {
    let reference = raw.trim();
    if reference.is_empty() {
        bail!("Invalid target reference: \"{reference}\"");
    }
    // A formatter that wraps long comments can break a tag inside a reference. The file or block
    // would then not be found, and that error would not say why.
    if reference.contains('\n') {
        bail!(
            "Invalid target reference: \"{reference}\" has a line break. Put the reference on one line"
        );
    }
    if reference.contains('#') && reference.contains(':') {
        bail!(
            "Invalid target reference: \"{reference}\" combines '#' and ':'; \
             these forms are mutually exclusive"
        );
    }
    if let Some((file_part, _)) = reference.split_once('#') {
        #[expect(clippy::string_slice, reason = "`file_part` ends right before a `#`")]
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
fn target_display(file: &RepoPath, name: Option<&str>) -> String {
    match name {
        // A symbol path keeps its `#`, which already separates it from the file.
        Some(name) if name.starts_with('#') => format!("{file}{name}"),
        Some(name) => format!("{file}:{name}"),
        None => format!("file {file}"),
    }
}

/// What a reference's target resolves to, or the reason it does not resolve, such as a missing
/// block or a missing symbol. The reason is reported as the reference's violation. A failure that
/// ends the run is the `Err` of an enclosing `anyhow::Result` instead.
type TargetResult<T> = Result<T, String>;

/// The text and the symbols of the files that references point into, each read and derived at most
/// once.
struct TargetFiles<'a, Fs: FileSystem> {
    context: &'a ValidationContext,
    /// Reads the target files that are not in `context`.
    file_system: &'a Fs,
    /// The text of the target files that are not in `context`.
    contents: HashMap<RepoPath, String>,
    /// The symbols of every file a symbol reference points into.
    symbols: HashMap<RepoPath, Vec<Symbol>>,
}

impl<'a, Fs: FileSystem> TargetFiles<'a, Fs> {
    fn new(context: &'a ValidationContext, file_system: &'a Fs) -> Self {
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
    fn content(&mut self, file: &RepoPath) -> anyhow::Result<&str> {
        file_content(self.context, self.file_system, &mut self.contents, file)
    }

    /// Resolves `path` among the symbols of `file`. Returns the text of `file`, together with the
    /// symbol or with the reason why no single symbol has `path`.
    ///
    /// # Errors
    /// Returns an error that shows the reference if `file` cannot be read, has no grammar, is in a
    /// language without symbols, or does not parse.
    fn resolve_symbol(
        &mut self,
        file: &RepoPath,
        path: &SymbolPath,
    ) -> anyhow::Result<(&str, TargetResult<&Symbol>)> {
        let (content, symbols) = self
            .content_and_symbols(file)
            .with_context(|| format!("failed to resolve symbol reference {file}#{path}"))?;
        let resolution = resolve(symbols, path).map_err(|error| error.reason(content));
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
                    &self.context.parsers,
                    &self.context.extra_file_extensions,
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

/// Returns the value that `captures`, a match of `regex`, gives: the named group `value` when
/// `regex` has one, or else the whole match. Returns `None` when `regex` has a `value` group that is
/// not in this match.
fn value_match<'h>(
    regex: &regex::Regex,
    captures: &regex::Captures<'h>,
) -> Option<regex::Match<'h>> {
    // Decided by the pattern, not by the match. A pattern can put the group in one branch only,
    // such as `^(?:none|(?P<value>\w+))$`, so that a match of the other branch gives no value.
    if regex.capture_names().any(|name| name == Some("value")) {
        captures.name("value")
    } else {
        captures.get(0)
    }
}

/// Returns the non-empty trimmed content of `line` together with its 0-based character-column
/// range within `line`, or `None` for a blank line. The range is measured in characters rather than
/// bytes and is half-open: `[start, end)`.
fn trimmed_line_value(line: &str) -> Option<(&str, Range<usize>)> {
    let trimmed_line = line.trim();
    if trimmed_line.is_empty() {
        None
    } else {
        let byte_offset = trimmed_line.as_ptr() as usize - line.as_ptr() as usize;
        #[expect(clippy::string_slice, reason = "`trimmed_line` is a slice of `line`")]
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
/// A blank line, an unmatched line, and a line whose match gives no value or an empty one all return
/// `None`: none of them carry a value.
fn regex_value<'a>(line: &'a str, regex: &regex::Regex) -> Option<(&'a str, Range<usize>)> {
    let trimmed_line = line.trim();
    if trimmed_line.is_empty() {
        return None;
    }
    let caps = regex.captures(trimmed_line)?;
    let m = value_match(regex, &caps)?;
    if m.is_empty() {
        return None;
    }
    let trimmed_byte_offset = trimmed_line.as_ptr() as usize - line.as_ptr() as usize;
    #[expect(
        clippy::string_slice,
        reason = "`m` is a match in `trimmed_line`, a slice of `line`"
    )]
    let start = line[..trimmed_byte_offset + m.start()].chars().count();
    let end = start + m.as_str().chars().count();
    Some((m.as_str(), start..end))
}

/// Compiles `pattern`, the regex in a block attribute such as `keep-sorted-pattern`.
///
/// Returns an error when `pattern` has a line break or is not a valid regex.
fn compile_pattern(pattern: &str) -> anyhow::Result<regex::Regex> {
    // A formatter that wraps long comments can break a tag inside a pattern. Where it breaks at a
    // space, the space is gone, so the line break can't be turned back into the text it replaced.
    // Left in, the line break makes the pattern match no line, and a rule such as keep-sorted then
    // checks nothing.
    if pattern.contains('\n') {
        bail!("the pattern has a line break. Put the pattern on one line");
    }
    Ok(regex::Regex::new(pattern)?)
}

/// The content for the `*-pattern` attribute extracted from the block.
enum PatternContent<'c> {
    /// No `*-pattern` attribute: the block's whole content (trimmed).
    Whole(&'c str),
    /// A `*-pattern` attribute: the value of every match, in the order they appear in the block.
    Matches(Vec<&'c str>),
}

/// Returns the content for the `*-pattern` attribute, e.g. `check-ai-pattern` for `check-ai`.
fn block_content_for_pattern<'c>(
    block_with_context: &'c BlockWithContext,
    file_content: &'c str,
    pattern_attribute: &str,
) -> anyhow::Result<PatternContent<'c>> {
    let content = block_with_context.block.content.text(file_content);
    let Some(pattern) = block_with_context.block.attributes.get(pattern_attribute) else {
        return Ok(PatternContent::Whole(content.trim()));
    };
    let re = compile_pattern(pattern)
        .with_context(|| format!("{pattern_attribute} is not a valid regex"))?;
    Ok(PatternContent::Matches(
        re.captures_iter(content)
            .filter_map(|captures| value_match(&re, &captures).map(|matched| matched.as_str()))
            .filter(|value| !value.is_empty())
            .collect(),
    ))
}

/// Parses a numeric value from string.
///
/// Supports very big numbers (bigger than `f64` may hold) and `_` digits separator.
fn parse_number(value: &str) -> Option<BigDecimal> {
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
mod compile_pattern_tests {
    use super::compile_pattern;

    #[test]
    fn pattern_with_an_escaped_line_break_compiles() -> anyhow::Result<()> {
        assert!(compile_pattern(r"a\nb")?.is_match("a\nb"));
        Ok(())
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
    fn multiple_references_with_a_line_break_after_a_comma() -> anyhow::Result<()> {
        let result = parse_target_references("file1.rs:block1,\n   file2.rs:block2")?;
        assert_eq!(
            result,
            vec![block("file1.rs", "block1")?, block("file2.rs", "block2")?]
        );
        Ok(())
    }

    #[test]
    fn a_reference_with_a_line_break_returns_error() {
        let err = parse_target_references("docs/config.\n   md:block").unwrap_err();
        assert!(
            err.to_string().contains("line break"),
            "unexpected error: {err}"
        );
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
    use crate::blocks::{Block, BlockAddress, BlockWithContext, Content, Declaration};
    use crate::fs::FileSystem;
    use crate::fs::test_utils::FakeFileSystem;
    use crate::repo_path::RepoPath;
    use crate::settings::BlockSelection;
    use crate::test_utils::{validation_context, virtual_block};
    use crate::validators::test_utils::{merge_validation_contexts, with_virtual_blocks};
    use crate::validators::{
        AsyncValidator, SyncValidator, ValidationContext, ValidationReport, Validator,
        ValidatorType, Violation, ViolationRange, detect_validators,
    };
    use crate::{Position, validators};
    use async_trait::async_trait;
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;

    fn empty_testing_block() -> Block {
        Block {
            attributes: HashMap::new(),
            start_tag_position_range: Position::new(0, 0)..Position::new(0, 0),
            content: Content::source(0..0, Position::new(0, 0)..Position::new(0, 0)),
            declaration: Declaration::Tags,
        }
    }

    fn empty_testing_violation_range() -> ViolationRange {
        ViolationRange::new(Position::new(0, 0), Position::new(0, 0))
    }

    struct FakeAsyncValidator {
        testing_block: Arc<Block>,
    }

    #[async_trait]
    impl AsyncValidator for FakeAsyncValidator {
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
                                    Validator::CheckAi,
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

    impl SyncValidator for FakeSyncValidator {
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
                                    Validator::KeepSorted,
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

    /// Detects the fakes above in place of `keep-sorted` and `check-ai`. No other validator is
    /// detected.
    fn detect_fakes<Fs: FileSystem>(
        validator: Validator,
        block_with_context: &BlockWithContext,
        _file_system: &Arc<Fs>,
    ) -> anyhow::Result<Option<ValidatorType>> {
        let attributes = &block_with_context.block.attributes;
        Ok(match validator {
            Validator::KeepSorted => attributes.contains_key("keep-sorted").then(|| {
                ValidatorType::Sync(Box::new(FakeSyncValidator {
                    testing_block: Arc::new(empty_testing_block()),
                }))
            }),
            Validator::CheckAi => attributes.contains_key("check-ai").then(|| {
                ValidatorType::Async(Box::new(FakeAsyncValidator {
                    testing_block: Arc::new(empty_testing_block()),
                }))
            }),
            Validator::Affects
            | Validator::CheckLua
            | Validator::KeepUnique
            | Validator::LineCount
            | Validator::LinePattern
            | Validator::SameAs => None,
        })
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
            detect_fakes,
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
            detect_fakes,
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
            Validator::KeepSorted
        );
        assert_eq!(
            violations[&RepoPath::from_reference("example2.py")?].len(),
            1
        );
        assert_eq!(
            violations[&RepoPath::from_reference("example2.py")?][0].code,
            Validator::KeepSorted
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
            detect_fakes,
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
            Validator::CheckAi
        );
        assert_eq!(
            violations[&RepoPath::from_reference("example2.py")?].len(),
            1
        );
        assert_eq!(
            violations[&RepoPath::from_reference("example2.py")?][0].code,
            Validator::CheckAi
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
            detect_fakes,
            &HashSet::from([Validator::CheckAi]),
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
            Validator::KeepSorted
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
            detect_fakes,
            &HashSet::new(),
            &HashSet::from([Validator::CheckAi]),
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
            Validator::CheckAi
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
            detect_fakes,
            &HashSet::from([Validator::KeepSorted]),
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
            Validator::CheckAi
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
            detect_fakes,
            &HashSet::new(),
            &HashSet::from([Validator::KeepSorted]),
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
            Validator::KeepSorted
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

    #[test]
    fn to_serializable_report_lists_files_sorted_by_path() {
        let names = [
            "c.rs", "a.rs", "j.rs", "e.rs", "b.rs", "h.rs", "d.rs", "g.rs", "i.rs", "f.rs",
        ];
        let context = merge_validation_contexts(
            names
                .iter()
                .map(|name| validation_context(name, "// <block>\n// </block>\n"))
                .collect(),
        );

        let paths: Vec<String> = context
            .to_serializable_report()
            .into_keys()
            .map(|path| path.to_string())
            .collect();

        let mut sorted = names.to_vec();
        sorted.sort();
        assert_eq!(paths, sorted);
    }

    #[test]
    fn run_sorts_violations_by_position_then_validator_then_message() -> anyhow::Result<()> {
        // A validator's violations are merged in the order of the block where it was first found, so
        // the first block puts `keep-sorted` first.
        let contents = r#"# <block keep-sorted>
a
b
# </block>
# <block line-pattern="^[a-z]+$">
B
# </block>
# <block keep-unique>
a
a
# </block>
# <block keep-sorted>
b
a
# </block>
# <block line-count="<1" same-as=":red, :blue">
x
# </block>
"#;
        let context = validation_context("example.py", contents);

        let (sync_validators, async_validators) = detect_validators(
            &context,
            Validator::detect,
            &HashSet::new(),
            &HashSet::new(),
            &Arc::new(FakeFileSystem::new(HashMap::from([(
                "example.py".to_string(),
                contents.to_string(),
            )]))),
        )?;
        let violations = validators::run(context, sync_validators, async_validators)?.violations;

        let order: Vec<(usize, &str, &str)> = violations[&RepoPath::from_reference("example.py")?]
            .iter()
            .map(|violation| {
                (
                    violation.range.start.line,
                    violation.code.as_str(),
                    violation.message.as_str(),
                )
            })
            .collect();
        assert_eq!(
            order
                .iter()
                .map(|(line, code, _)| (*line, *code))
                .collect::<Vec<_>>(),
            vec![
                (6, "line-pattern"),
                (10, "keep-unique"),
                (14, "keep-sorted"),
                (16, "line-count"),
                (16, "same-as"),
                (16, "same-as"),
            ]
        );
        assert!(order[4].2.contains(":blue"), "{order:?}");
        assert!(order[5].2.contains(":red"), "{order:?}");
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
                &HashSet::from([Validator::Affects]),
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
                &HashSet::from([Validator::KeepSorted])
            ),
            0
        );
    }

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

    fn block_addresses(addresses: &[&str]) -> Vec<BlockAddress> {
        addresses
            .iter()
            .map(|address| BlockAddress::parse(address).expect("a valid address"))
            .collect()
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

#[cfg(test)]
mod test_utils {
    use crate::blocks::FileBlocks;
    use crate::validators::{ValidationContext, ValidationReport};
    use crate::virtual_blocks::VirtualBlock;
    use std::collections::HashMap;
    use std::sync::Arc;

    /// The start line of every block a validator reported checking, in the order it checked them.
    pub(super) fn checked_lines(report: &ValidationReport) -> Vec<usize> {
        report
            .checked_blocks
            .iter()
            .map(|(_, key)| key.start_line())
            .collect()
    }

    /// How many violations a validator reported, across every file.
    pub(super) fn violation_count(report: &ValidationReport) -> usize {
        report.violations.values().map(Vec::len).sum()
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
            context.extra_file_extensions,
            virtual_blocks,
        ))
    }

    /// Combines several single-file contexts into one, so a test can exercise a validator that
    /// resolves references across files.
    pub(super) fn merge_validation_contexts(
        contexts: Vec<Arc<ValidationContext>>,
    ) -> Arc<ValidationContext> {
        let parsers = contexts
            .first()
            .map(|context| context.parsers.clone())
            .unwrap_or_default();
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
            for (file_path, line_changes) in &context.line_changes {
                merged_line_changes.insert(file_path.clone(), line_changes.to_vec());
            }
        }
        Arc::new(ValidationContext::new(
            merged_modified_blocks,
            parsers,
            merged_line_changes,
            HashMap::new(),
            Vec::new(),
        ))
    }
}
