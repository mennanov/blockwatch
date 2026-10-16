use crate::blocks::{Block, BlockWithContext, FileBlocks};
use crate::fs::FileSystem;
use crate::paths_glob::PathsGlob;
use crate::repo_path::RepoPath;
use crate::symbol_path::SymbolPath;
use crate::validators::{
    self, SyncValidator, TargetFiles, TargetReference, TargetResult, ValidationReport, Validator,
    ValidatorType, Violation, compile_pattern, parse_number, target_display,
};
use anyhow::{Context, anyhow, bail};
use heck::ToSnakeCase;
use serde::Serialize;
use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::hash::Hash;
use std::sync::Arc;

/// Enforces `same-as="file:name"`: the block's content must equal that of the blocks it names.
///
/// Where `affects` only checks that linked blocks were edited together, this compares their actual
/// contents, catching a constant or version duplicated in two places that have drifted apart. That
/// is also why it runs on a full-tree scan and not only on the blocks a diff touched.
///
/// A target spelled without a `:` names a whole file, whose entire text is then the thing compared
/// against — so a file with no comments to declare a block in can still be a target.
///
/// A `same-as-paths` glob compares the block with the repository paths that match it, so a list in
/// the docs can't miss a directory that was added later.
struct SameAsValidator<Fs: FileSystem> {
    /// Reads files containing referenced target blocks that are not already parsed into the
    /// validation context.
    file_system: Arc<Fs>,
}

impl<Fs: FileSystem + 'static> SameAsValidator<Fs> {
    /// Creates the validator over the filesystem it will read referenced files from.
    fn new(file_system: Arc<Fs>) -> Self {
        Self { file_system }
    }
}

impl<Fs: FileSystem + 'static> SyncValidator for SameAsValidator<Fs> {
    fn validate(
        &self,
        context: Arc<validators::ValidationContext>,
    ) -> anyhow::Result<ValidationReport> {
        let mut targets = TargetItems::new(&context, self.file_system.as_ref());
        let mut report = ValidationReport::default();
        for (file_path, file_blocks) in &context.blocks {
            for block_with_context in &file_blocks.blocks_with_context {
                let attributes = &block_with_context.block.attributes;
                if !attributes.contains_key("same-as") && !attributes.contains_key("same-as-paths")
                {
                    continue;
                }
                let violations = block_violations(
                    &mut targets,
                    file_path,
                    &file_blocks.file_content,
                    &block_with_context.block,
                )?;
                report.add_all(file_path, &block_with_context.block, violations);
            }
        }
        Ok(report)
    }
}

/// One block's side of a `same-as` comparison: the items it contributes and the rules the two sides
/// are held to. Built once per block and reused for each of its references.
struct Comparison {
    mode: Mode,
    formats: Formats,
    /// `Err` carries the reason the block's own items could not be canonicalized. It is reported
    /// against every reference the block makes rather than aborting the run, since content that does
    /// not meet the asserted shape is a violation like any other.
    source_items: Result<Vec<Record>, String>,
    /// The block's `same-as-pattern`. It also reads a whole-file, symbol or paths target, because
    /// such a target has no block of its own to carry one.
    pattern: Option<String>,
}

impl Comparison {
    fn new(block: &Block, file_content: &str) -> anyhow::Result<Self> {
        let formats = parse_formats(block)?;
        Ok(Self {
            mode: parse_mode(block)?,
            source_items: canonicalize(extract_items(block, file_content)?, &formats),
            pattern: block.attributes.get("same-as-pattern").cloned(),
            formats,
        })
    }

    /// Why the target disagrees with the block, or `None` when the two agree.
    fn disagreement_with(&self, target_items: Vec<Record>) -> Option<String> {
        match &self.source_items {
            Err(reason) => Some(reason.clone()),
            Ok(source) => {
                groups_disagreement(source, &target_items).or_else(|| {
                    match canonicalize(target_items, &self.formats) {
                        Err(reason) => Some(reason),
                        Ok(target) => disagreement(source, &target, self.mode),
                    }
                })
            }
        }
    }
}

/// Every violation one block's `same-as` and `same-as-paths` produce: at most one per reference in
/// `same-as`, then at most one for `same-as-paths`.
fn block_violations<Fs: FileSystem>(
    targets: &mut TargetItems<'_, Fs>,
    file_path: &RepoPath,
    file_content: &str,
    block: &Block,
) -> anyhow::Result<Vec<Violation>> {
    let comparison = Comparison::new(block, file_content).with_context(|| {
        format!(
            "invalid same-as attributes on {}",
            block_display(file_path, block)
        )
    })?;
    let mut violations = Vec::new();
    if let Some(same_as) = block.attributes.get("same-as") {
        for reference in parse_references(file_path, block, same_as)? {
            let violation = reference_violation(targets, &comparison, file_path, block, reference)?;
            violations.extend(violation);
        }
    }
    if let Some(glob) = block.attributes.get("same-as-paths") {
        violations.extend(paths_violation(
            targets.file_system,
            &comparison,
            file_path,
            block,
            glob,
        )?);
    }
    Ok(violations)
}

/// The violation a `same-as-paths` glob produces, or `None` when the block agrees with the
/// repository paths that match `glob`.
///
/// # Errors
/// Returns an error if the block has no `same-as-pattern`, if `glob` is not a valid glob, or if the
/// repository can't be walked.
fn paths_violation(
    file_system: &impl FileSystem,
    comparison: &Comparison,
    file_path: &RepoPath,
    block: &Block,
    glob: &str,
) -> anyhow::Result<Option<Violation>> {
    // Without a pattern, all the paths would make one value. A mismatch would then show the whole
    // list as one string, and every mode would act the same.
    let Some(pattern) = &comparison.pattern else {
        bail!(
            "same-as-paths needs same-as-pattern on {}. To compare whole lines, use \
             same-as-pattern=\".+\"",
            block_display(file_path, block)
        );
    };
    let invalid_glob = || {
        format!(
            "invalid same-as-paths glob on {}",
            block_display(file_path, block)
        )
    };
    // The glob is written the way the block writes its paths, so a directory ends with `/`.
    let paths = match glob.strip_suffix('/') {
        Some(directory_glob) => PathsGlob::new(directory_glob)
            .with_context(invalid_glob)?
            .matching_directories(file_system)?,
        None => PathsGlob::new(glob)
            .with_context(invalid_glob)?
            .matching_files(file_system)?
            .iter()
            .map(ToString::to_string)
            .collect(),
    };
    let reason = if paths.is_empty() {
        Some("no paths match".to_string())
    } else {
        comparison.disagreement_with(extract_items_from(&paths.join("\n"), Some(pattern))?)
    };
    reason
        .map(|reason| {
            create_violation(
                file_path,
                block,
                &format!("paths matching {glob}"),
                SameAsViolation {
                    target_file: None,
                    target_name: None,
                    target_paths: Some(glob),
                    reason: &reason,
                },
            )
        })
        .transpose()
}

/// How an error message shows `block` in `file_path`, such as `block README.md:services at line 3`.
fn block_display(file_path: &RepoPath, block: &Block) -> String {
    format!(
        "block {}:{} at {}",
        file_path,
        block.name_display(),
        block.declared_at()
    )
}

/// Parses the attribute's raw value, naming the block that wrote it so an author can find the
/// offending reference without searching for it.
fn parse_references(
    file_path: &RepoPath,
    block: &Block,
    same_as: &str,
) -> anyhow::Result<Vec<TargetReference>> {
    validators::parse_target_references(same_as).with_context(|| {
        format!(
            "invalid same-as reference on {}",
            block_display(file_path, block)
        )
    })
}

/// The violation one reference produces, or `None` when the block and the target agree.
///
/// A target that cannot be found is reported rather than aborting the run; a target *file* that
/// cannot be read is an `Err`, which is [`TargetItems`]'s call to make.
fn reference_violation<Fs: FileSystem>(
    targets: &mut TargetItems<'_, Fs>,
    comparison: &Comparison,
    file_path: &RepoPath,
    block: &Block,
    reference: TargetReference,
) -> anyhow::Result<Option<Violation>> {
    // A reference without a file, like ":foo" or "#/foo", points into the file the block is in.
    let (target_file, target_name, resolution) = match reference {
        TargetReference::Block { file, name } => {
            let target_file = file.unwrap_or_else(|| file_path.clone());
            let resolution = targets.named_block_items(&target_file, &name)?;
            (target_file, Some(name), resolution)
        }
        TargetReference::File(target_file) => {
            let items = targets.whole_file_items(&target_file, comparison.pattern.as_deref())?;
            (target_file, None, Ok(items))
        }
        TargetReference::Symbol { file, path } => {
            let target_file = file.unwrap_or_else(|| file_path.clone());
            let resolution =
                targets.symbol_items(&target_file, &path, comparison.pattern.as_deref())?;
            (target_file, Some(format!("#{path}")), resolution)
        }
    };
    let target_name = target_name.as_deref();
    let reason = match resolution {
        Ok(target_items) => comparison.disagreement_with(target_items),
        Err(reason) => Some(reason),
    };
    reason
        .map(|reason| {
            create_violation(
                file_path,
                block,
                &target_display(&target_file, target_name),
                SameAsViolation {
                    target_file: Some(&target_file),
                    target_name,
                    target_paths: None,
                    reason: &reason,
                },
            )
        })
        .transpose()
}

/// The group of a whole match, or of a block's whole content. It is the name of the group that
/// picks the value in every `*-pattern` attribute, so `(?P<value>\d+)` and `\d+` give equal records.
const WHOLE_GROUP: &str = "value";

/// One match of a `same-as-pattern`: the text of each of its named groups.
///
/// A pattern without named groups, or no pattern at all, gives the one group `value`, so it can be
/// compared with a pattern whose only group is `value`. Every record of one pattern has the same
/// groups, because a match without one of them is skipped.
#[derive(PartialEq, Eq, Hash)]
struct Record {
    /// The text of each group, by group name. Sorted by name, so two patterns that list the same
    /// groups in a different order give equal records.
    values: BTreeMap<String, String>,
}

impl Record {
    /// The record of a whole match, or of a block's whole content: the one group `value`.
    fn whole(text: String) -> Self {
        Self {
            values: BTreeMap::from([(WHOLE_GROUP.to_string(), text)]),
        }
    }

    /// The names of the groups, sorted.
    fn groups(&self) -> Vec<&str> {
        self.values.keys().map(String::as_str).collect()
    }

    /// The value of the only group, or `None` when the record has several groups.
    fn lone_value(&self) -> Option<&String> {
        match self.values.len() {
            1 => self.values.values().next(),
            _ => None,
        }
    }
}

impl fmt::Display for Record {
    /// Shows `"red"` for one group, and `{name: "red", value: "1"}` for several.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.lone_value() {
            // The one group is almost always `value`, which adds nothing to the message.
            Some(value) => write!(f, "{value:?}"),
            None => {
                let fields: Vec<String> = self
                    .values
                    .iter()
                    .map(|(group, value)| format!("{group}: {value:?}"))
                    .collect();
                f.write_str(&["{", &fields.join(", "), "}"].concat())
            }
        }
    }
}

/// Extracts a block's comparable items.
///
/// With a `same-as-pattern` attribute, every match on every content line gives one record: the
/// text of each named group, or the whole match when the pattern has no named groups. Lines that do
/// not match are skipped. A pattern selects which parts of a block take part in the comparison.
/// A match where a group took no part, or captured nothing, has nothing to compare and is skipped.
///
/// The regex runs against each trimmed line rather than the whole block, so `^` and `$` anchor to
/// the entry rather than to its indentation, and the items stay in the order the two sides list
/// them.
///
/// Without a pattern, the block's normalized whole content is a single record.
fn extract_items(block: &Block, file_content: &str) -> anyhow::Result<Vec<Record>> {
    extract_items_from(
        block.content.text(file_content),
        block.attributes.get("same-as-pattern").map(String::as_str),
    )
}

/// Extracts items from `content` for the given `pattern`.
///
/// If `pattern` is `None` then the whole `content` is returned.
fn extract_items_from(content: &str, pattern: Option<&str>) -> anyhow::Result<Vec<Record>> {
    let Some(pattern) = pattern else {
        return Ok(vec![Record::whole(normalize_content(content))]);
    };
    let regex = same_as_regex(pattern)?;
    let groups: Vec<&str> = regex.capture_names().flatten().collect();
    Ok(content
        .lines()
        .flat_map(|line| {
            regex
                .captures_iter(line.trim())
                .filter_map(|captures| record(&groups, &captures))
        })
        .collect())
}

/// Compiles a `same-as-pattern`.
fn same_as_regex(pattern: &str) -> anyhow::Result<regex::Regex> {
    compile_pattern(pattern)
        .map_err(|e| anyhow!("same-as-pattern is not a valid regex ({pattern}): {e}"))
}

/// The groups of every record that `pattern` gives: its named groups, or [`WHOLE_GROUP`] when it
/// has none or when there is no pattern.
fn pattern_groups(pattern: Option<&str>) -> anyhow::Result<Vec<String>> {
    let groups: Vec<String> = match pattern {
        Some(pattern) => same_as_regex(pattern)?
            .capture_names()
            .flatten()
            .map(str::to_string)
            .collect(),
        None => Vec::new(),
    };
    Ok(if groups.is_empty() {
        vec![WHOLE_GROUP.to_string()]
    } else {
        groups
    })
}

/// The record of one match: the text of each group in `groups`, or the whole match when `groups`
/// is empty. Returns `None` when a group took no part in the match or captured nothing.
fn record(groups: &[&str], captures: &regex::Captures<'_>) -> Option<Record> {
    if groups.is_empty() {
        return non_empty(captures.get(0)).map(Record::whole);
    }
    let values = groups
        .iter()
        .map(|group| Some((group.to_string(), non_empty(captures.name(group))?)))
        .collect::<Option<_>>()?;
    Some(Record { values })
}

/// The text of `matched`, or `None` when its group took no part in the match or captured nothing.
fn non_empty(matched: Option<regex::Match<'_>>) -> Option<String> {
    matched
        .map(|matched| matched.as_str())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Trim each line, drop blanks, rejoin — matching `line-pattern` / `keep-unique` normalization.
///
/// Folded rather than joined so the retained lines are never collected into an intermediate `Vec`.
fn normalize_content(content: &str) -> String {
    content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .fold(String::new(), |mut normalized, line| {
            if !normalized.is_empty() {
                normalized.push('\n');
            }
            normalized.push_str(line);
            normalized
        })
}

/// Find a named block in a file and extract its comparable items.
fn extract_named(file_blocks: &FileBlocks, name: &str) -> anyhow::Result<Option<Vec<Record>>> {
    for bwc in &file_blocks.blocks_with_context {
        if bwc.block.name() == Some(name) {
            return Ok(Some(extract_items(&bwc.block, &file_blocks.file_content)?));
        }
    }
    Ok(None)
}

/// Resolves the targets a `same-as` reference names to their comparable items.
struct TargetItems<'a, Fs: FileSystem> {
    context: &'a validators::ValidationContext,
    /// Reads target files that are not already parsed into the validation context.
    file_system: &'a Fs,
    /// Files parsed from the disk while looking for a named target block.
    parsed: HashMap<RepoPath, FileBlocks>,
    /// The text and the symbols of whole-file and symbol targets.
    files: TargetFiles<'a, Fs>,
}

impl<'a, Fs: FileSystem> TargetItems<'a, Fs> {
    fn new(context: &'a validators::ValidationContext, file_system: &'a Fs) -> Self {
        Self {
            context,
            file_system,
            parsed: HashMap::new(),
            files: TargetFiles::new(context, file_system),
        }
    }

    /// Resolves a named target block's comparable items via a three-step lookup:
    /// 1. blocks already parsed into the validation context (no I/O),
    /// 2. the files read earlier during this run,
    /// 3. reading and parsing the file, keeping the result.
    ///
    /// Resolves to a reason instead when the file has no block with that name.
    fn named_block_items(
        &mut self,
        target_file: &RepoPath,
        target_name: &str,
    ) -> anyhow::Result<TargetResult<Vec<Record>>> {
        if let Some(file_blocks) = self.context.blocks.get(target_file)
            && let Some(items) = extract_named(file_blocks, target_name)?
        {
            return Ok(Ok(items));
        }
        let file_blocks = match self.parsed.entry(target_file.clone()) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let parsed = self
                    .context
                    .parse_file(self.file_system, target_file)?
                    .ok_or_else(|| {
                        anyhow!("same-as target file format is unsupported: {target_file}")
                    })?;
                entry.insert(parsed)
            }
        };
        Ok(extract_named(file_blocks, target_name)?
            .ok_or_else(|| "target block not found".to_string()))
    }

    /// Resolves a whole-file target's comparable items from the file's entire text.
    ///
    /// No grammar is involved, which is what lets a `same-as` target be a file blockwatch cannot
    /// parse. A file already in the validation context is read from there; otherwise it is read
    /// through the filesystem and kept. A missing or unreadable file is an `Err`, as it is for a
    /// named target.
    fn whole_file_items(
        &mut self,
        target_file: &RepoPath,
        pattern: Option<&str>,
    ) -> anyhow::Result<Vec<Record>> {
        let content = self
            .files
            .content(target_file)
            .with_context(|| format!("failed to read same-as target file: {target_file}"))?;
        extract_items_from(content, pattern)
    }

    /// Resolves a symbol target's comparable items from the symbol's value, or from the text of
    /// its definition when it has no value.
    ///
    /// Resolves to a reason instead when no symbol, or more than one, has `path`. A file that
    /// cannot be read, has no grammar or no symbols, or does not parse, is an `Err`.
    fn symbol_items(
        &mut self,
        target_file: &RepoPath,
        path: &SymbolPath,
        pattern: Option<&str>,
    ) -> anyhow::Result<TargetResult<Vec<Record>>> {
        let (content, resolution) = self.files.resolve_symbol(target_file, path)?;
        match resolution {
            Ok(symbol) => Ok(Ok(extract_items_from(
                &symbol.value_or_definition(content),
                pattern,
            )?)),
            Err(reason) => Ok(Err(reason)),
        }
    }
}

/// How extracted items are canonicalized before comparison.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    /// Compare items as extracted (the default).
    Verbatim,
    /// Parse each item as a number so different spellings (`"10"`, `"10.0"`) compare equal.
    Numeric,
    /// Split each item into words so naming styles (`"PreExistingFailure"`,
    /// `"pre-existing-failure"`) compare equal.
    Identifier,
    /// Keep only the lower-cased letters and digits of each item, so names whose words break in
    /// different places (`"DarkRed"`, `"darkred"`) compare equal.
    Flat,
}

/// Parses one format, such as `numeric`. An unrecognized value is an error, mirroring
/// [`parse_mode`].
fn parse_format(value: &str) -> anyhow::Result<Format> {
    match value {
        "numeric" => Ok(Format::Numeric),
        "identifier" => Ok(Format::Identifier),
        "flat" => Ok(Format::Flat),
        other => Err(anyhow!(
            "invalid same-as-format \"{other}\" (expected numeric, identifier or flat)"
        )),
    }
}

/// The format of each group of a block's records.
enum Formats {
    /// One format for every group.
    All(Format),
    /// A format for some groups, by group name. Every other group is compared as written, which
    /// is the strictest comparison, so a group left out can't hide a difference.
    ByGroup(HashMap<String, Format>),
}

impl Formats {
    /// The format of `group`.
    fn of(&self, group: &str) -> Format {
        match self {
            Formats::All(format) => *format,
            Formats::ByGroup(formats) => formats.get(group).copied().unwrap_or(Format::Verbatim),
        }
    }
}

/// Reads a block's `same-as-format` attribute: one format for every group, such as `numeric`, or
/// a format for some groups, such as `name:identifier,value:numeric`. Without the attribute, every
/// group is compared as written.
///
/// # Errors
/// Returns an error for an unknown format, for an attribute that mixes the two forms, and for a
/// group that the block's `same-as-pattern` doesn't have.
fn parse_formats(block: &Block) -> anyhow::Result<Formats> {
    let Some(attribute) = block.attributes.get("same-as-format") else {
        return Ok(Formats::All(Format::Verbatim));
    };
    if !attribute.contains(':') {
        return Ok(Formats::All(parse_format(attribute)?));
    }
    let groups = pattern_groups(block.attributes.get("same-as-pattern").map(String::as_str))?;
    attribute
        .split(',')
        .map(|entry| {
            let (group, format) = entry.trim().split_once(':').ok_or_else(|| {
                anyhow!(
                    "invalid same-as-format \"{attribute}\" (expected one format, or group:format \
                     entries such as value:numeric)"
                )
            })?;
            // A typo in a group would otherwise compare that group as written, with no sign that
            // its format is lost.
            if !groups.iter().any(|known| known == group) {
                bail!(
                    "same-as-format refers to group \"{group}\", which same-as-pattern doesn't have"
                );
            }
            Ok((group.to_string(), parse_format(format)?))
        })
        .collect::<anyhow::Result<_>>()
        .map(Formats::ByGroup)
}

/// Canonicalizes every value of `records` under the format of its group ahead of comparison (see
/// [`canonicalize_value`]). Returns `Err` with the reason of the first value that fails.
fn canonicalize(records: Vec<Record>, formats: &Formats) -> Result<Vec<Record>, String> {
    records
        .into_iter()
        .map(|record| {
            let values = record
                .values
                .into_iter()
                .map(|(group, value)| {
                    let value = canonicalize_value(value, formats.of(&group))?;
                    Ok((group, value))
                })
                .collect::<Result<_, String>>()?;
            Ok(Record { values })
        })
        .collect()
}

/// Canonicalizes one extracted value ahead of comparison.
///
/// Under [`Format::Numeric`] the value is parsed as a number (see [`parse_number`]) and re-formatted
/// so that different spellings of the same value (`"10"`, `"10.0"`, `"1_0"`) compare equal; a value
/// that is not a number yields `Err` with a human-readable reason, which the caller surfaces as a
/// violation rather than aborting the run. Under [`Format::Identifier`] the value becomes its
/// snake_case form (`"HTTPServer"` becomes `"http_server"`) and never fails. Under [`Format::Flat`]
/// the value keeps only its lower-cased letters and digits (`"Sign-In"` becomes `"signin"`) and
/// never fails. Under [`Format::Verbatim`] the value is returned unchanged.
///
/// Normalizing drops the trailing zeros and the exponent notation that would otherwise leave two
/// spellings of one value differing as strings.
fn canonicalize_value(value: String, format: Format) -> Result<String, String> {
    match format {
        Format::Verbatim => Ok(value),
        Format::Numeric => parse_number(&value)
            .map(|number| number.normalized().to_string())
            .ok_or_else(|| format!("same-as-format=numeric but \"{value}\" is not a number")),
        // snake_case keeps the word breaks, so a violation message stays readable, and two names
        // that only split the same letters differently still differ.
        Format::Identifier => Ok(value.to_snake_case()),
        Format::Flat => Ok(value
            .chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect()),
    }
}

/// How two item lists are compared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// Order- and duplicate-insensitive set equality (the default).
    Set,
    /// Order-sensitive list equality.
    Sequence,
    /// Exactly one value per side, compared directly.
    Single,
    /// Directional containment: every value in this block must also appear in the target.
    Subset,
}

/// Reads a block's `same-as-mode` attribute, defaulting to [`Mode::Set`].
fn parse_mode(block: &Block) -> anyhow::Result<Mode> {
    match block.attributes.get("same-as-mode").map(String::as_str) {
        None | Some("set") => Ok(Mode::Set),
        Some("sequence") => Ok(Mode::Sequence),
        Some("single") => Ok(Mode::Single),
        Some("subset") => Ok(Mode::Subset),
        Some(other) => Err(anyhow!(
            "invalid same-as-mode \"{other}\" (expected set, sequence, single, or subset)"
        )),
    }
}

/// Returns `None` when the two sides agree under `mode`, or `Some(reason)` describing the mismatch.
///
/// Set, sequence, and single are symmetric; `Mode::Subset` is directional (`source` ⊆ `target`).
/// A side without records is a mismatch in every mode.
fn disagreement(source: &[Record], target: &[Record], mode: Mode) -> Option<String> {
    if source.is_empty() || target.is_empty() {
        return Some("same-as-pattern matched no values in this block or its target".to_string());
    }
    match mode {
        Mode::Single => single_disagreement(source, target),
        Mode::Sequence => sequence_disagreement(source, target),
        Mode::Set => set_disagreement(source, target),
        Mode::Subset => subset_disagreement(source, target),
    }
}

/// Returns `None` when the records of `source` and `target` have the same groups, or when a side
/// has no records. Records with different groups are never equal, and a message that listed them
/// all would hide why.
///
/// Every record of one side has the same groups, so the first record of each side is enough.
fn groups_disagreement(source: &[Record], target: &[Record]) -> Option<String> {
    match (source.first(), target.first()) {
        (Some(source), Some(target)) => (source.groups() != target.groups()).then(|| {
            format!(
                "same-as-pattern groups differ: {} != {}",
                source.groups().join(", "),
                target.groups().join(", ")
            )
        }),
        // A side without records has no groups. `disagreement` reports that side.
        (None, _) | (_, None) => None,
    }
}

/// Returns `None` when each side holds exactly one record and the two are equal.
///
/// A side with a different count is itself a mismatch: the block's content does not have the
/// shape the mode asks for, so it is reported like any other mismatch.
fn single_disagreement(source: &[Record], target: &[Record]) -> Option<String> {
    match (source, target) {
        ([source], [target]) => (source != target).then(|| {
            match (source.lone_value(), target.lone_value()) {
                // Quotes keep values apart in a list or a record. A lone value on each side needs
                // none.
                (Some(source_value), Some(target_value)) => {
                    format!("{source_value} != {target_value}")
                }
                (None, _) | (_, None) => format!("{source} != {target}"),
            }
        }),
        _ => Some(format!(
            "same-as-mode=single requires exactly one value per side (got {} and {})",
            source.len(),
            target.len()
        )),
    }
}

/// Returns `None` when both sides hold the same records in the same order.
fn sequence_disagreement(source: &[Record], target: &[Record]) -> Option<String> {
    (source != target)
        .then(|| format!("{} != {}", records_display(source), records_display(target)))
}

/// Returns `None` when both sides hold the same records, in any order and with any repeats.
///
/// The reason lists each record once, at its first position. A repeat doesn't change the result,
/// and in a long block it makes a long message.
fn set_disagreement(source: &[Record], target: &[Record]) -> Option<String> {
    let source_set: HashSet<&Record> = source.iter().collect();
    let target_set: HashSet<&Record> = target.iter().collect();
    (source_set != target_set).then(|| {
        format!(
            "{} != {}",
            records_display(unique(source)),
            records_display(unique(target))
        )
    })
}

/// Returns `None` when every record of `source` is also in `target`.
///
/// The reason lists each missing record once, at its first position in `source`. A repeat doesn't
/// change the result, and in a long block it makes a long message.
fn subset_disagreement(source: &[Record], target: &[Record]) -> Option<String> {
    let target_set: HashSet<&Record> = target.iter().collect();
    let missing: Vec<&Record> = unique(source)
        .into_iter()
        .filter(|record| !target_set.contains(record))
        .collect();
    (!missing.is_empty()).then(|| {
        format!(
            "not a subset; missing from target: {}",
            records_display(missing)
        )
    })
}

/// Returns `values` without repeats, each value at the position where it first appears.
fn unique<T: Eq + Hash>(values: &[T]) -> Vec<&T> {
    let mut seen = HashSet::new();
    values.iter().filter(|value| seen.insert(*value)).collect()
}

/// How a message shows a list of records, such as `["red", "blue"]`.
fn records_display<'r>(records: impl IntoIterator<Item = &'r Record>) -> String {
    let records: Vec<String> = records.into_iter().map(ToString::to_string).collect();
    format!("[{}]", records.join(", "))
}

#[derive(Serialize)]
struct SameAsViolation<'a> {
    /// The file of a `same-as` reference. Absent for a `same-as-paths` glob.
    #[serde(skip_serializing_if = "Option::is_none")]
    target_file: Option<&'a RepoPath>,
    /// A block name, or a symbol path with its leading `#`. Absent for a whole-file reference.
    #[serde(skip_serializing_if = "Option::is_none")]
    target_name: Option<&'a str>,
    /// The `same-as-paths` glob. Absent for a `same-as` reference.
    #[serde(skip_serializing_if = "Option::is_none")]
    target_paths: Option<&'a str>,
    reason: &'a str,
}

/// The violation of `block` in `file_path`. `target` is how the message shows the target, such as
/// `file VERSION`.
fn create_violation(
    file_path: &RepoPath,
    block: &Block,
    target: &str,
    data: SameAsViolation<'_>,
) -> anyhow::Result<Violation> {
    let message = format!(
        "Block {file_path}:{} at {} disagrees with {target}: {}",
        block.name_display(),
        block.declared_at(),
        data.reason,
    );
    Ok(Violation::new(
        block.start_tag_position_range.clone().into(),
        file_path,
        block,
        Validator::SameAs,
        message,
        // A block may be compared against several targets, so the target identifies this violation.
        Some(target),
        Some(serde_json::to_value(data)?),
    ))
}

/// Selects [`SameAsValidator`] for blocks carrying a `same-as` or a `same-as-paths` attribute.
pub(super) fn detect<Fs: FileSystem + 'static>(
    block_with_context: &BlockWithContext,
    file_system: &Arc<Fs>,
) -> Option<ValidatorType> {
    let attributes = &block_with_context.block.attributes;
    (attributes.contains_key("same-as") || attributes.contains_key("same-as-paths"))
        .then(|| ValidatorType::Sync(Box::new(SameAsValidator::new(Arc::clone(file_system)))))
}

#[cfg(test)]
mod validate_tests {
    use super::*;
    use crate::diff_parser::{LineChange, LineChangeKind};
    use crate::fs::test_utils::FakeFileSystem;
    use crate::repo_path::RepoPath;
    use crate::test_utils::validation_context;
    use crate::test_utils::validation_context_with_changes;
    use crate::test_utils::virtual_block;
    use crate::validators::test_utils::{
        checked_lines, merge_validation_contexts, violation_count, with_virtual_blocks,
    };

    /// Build a validator with a fake filesystem seeded with `files` (path, contents). Used by every
    /// same-as unit test; pass `&[]` when the target is in-scope (no disk read).
    fn validator(files: &[(&str, &str)]) -> SameAsValidator<FakeFileSystem> {
        let map = files
            .iter()
            .map(|(p, c)| (p.to_string(), c.to_string()))
            .collect();
        SameAsValidator::new(Arc::new(FakeFileSystem::new(map)))
    }

    #[test]
    fn block_differing_from_a_target_in_the_same_file_returns_a_violation() -> anyhow::Result<()> {
        let context = validation_context(
            "config.py",
            "# <block same-as=\":b\">\nvalue = 10\n# </block>\n# <block name=\"b\">\nvalue = 20\n# </block>",
        );
        let violations = validator(&[]).validate(context)?.violations;
        let file = violations
            .get(&RepoPath::from_reference("config.py")?)
            .unwrap();
        assert_eq!(file.len(), 1);
        assert_eq!(file[0].code, Validator::SameAs);
        Ok(())
    }

    #[test]
    fn block_equal_to_a_target_in_the_same_file_returns_no_violations() -> anyhow::Result<()> {
        let context = validation_context(
            "config.py",
            "# <block same-as=\":b\">\nvalue = 10\n# </block>\n# <block name=\"b\">\nvalue = 10\n# </block>",
        );
        assert!(validator(&[]).validate(context)?.violations.is_empty());
        Ok(())
    }

    #[test]
    fn block_equal_to_a_target_in_another_in_scope_file_returns_no_violations() -> anyhow::Result<()>
    {
        let context = merge_validation_contexts(vec![
            validation_context("a.rs", "// <block same-as=\"b.md:doc\">\nX\n// </block>"),
            validation_context(
                "b.md",
                "[//]: # (<block name=\"doc\">)\n\nX\n\n[//]: # (</block>)",
            ),
        ]);
        assert!(validator(&[]).validate(context)?.violations.is_empty());
        Ok(())
    }

    #[test]
    fn block_referencing_a_missing_target_returns_a_violation() -> anyhow::Result<()> {
        let source = "# <block same-as=\":nope\">\nvalue = 10\n# </block>";
        let context = validation_context("config.py", source);
        // The file exists but has no block named "nope"; confirming its absence requires reading the
        // full file, so the filesystem is seeded with it.
        let violations = validator(&[("config.py", source)])
            .validate(context)?
            .violations;
        let violations = violations
            .get(&RepoPath::from_reference("config.py")?)
            .unwrap();
        assert_eq!(violations.len(), 1);
        assert!(
            violations[0]
                .as_simple_diagnostic()
                .message
                .ends_with("disagrees with config.py:nope: target block not found"),
            "unexpected message: {}",
            violations[0].as_simple_diagnostic().message
        );
        Ok(())
    }

    #[test]
    fn block_referencing_an_out_of_scope_target_resolves_it_from_disk() -> anyhow::Result<()> {
        // Only the source is in scope; the target file is provided through the fake filesystem.
        let context = validation_context("a.rs", "// <block same-as=\"b.md:doc\">\nX\n// </block>");
        let v = validator(&[(
            "b.md",
            "[//]: # (<block name=\"doc\">)\n\nX\n\n[//]: # (</block>)",
        )]);
        assert!(v.validate(context)?.violations.is_empty());
        Ok(())
    }

    #[test]
    fn block_equal_to_a_virtual_block_outside_the_scan_returns_no_violations() -> anyhow::Result<()>
    {
        let source = r#"// <block same-as="package.json:version">
1.2.3
// </block>
"#;
        // Only `a.rs` is in scope, so the validator reads `package.json` itself. The block it
        // refers to is declared in the config file. The block of `other.json` must not be looked
        // for in `package.json`.
        let context = with_virtual_blocks(
            validation_context("a.rs", source),
            vec![
                virtual_block("other.json#/x", &[])?,
                virtual_block("package.json#/version", &[("name", "version")])?,
            ],
        );
        let v = validator(&[("package.json", r#"{"version": "1.2.3"}"#)]);

        assert!(v.validate(context)?.violations.is_empty());
        Ok(())
    }

    #[test]
    fn virtual_block_that_does_not_resolve_in_a_file_outside_the_scan_returns_an_error()
    -> anyhow::Result<()> {
        let source = r#"// <block same-as="package.json:version">
1.2.3
// </block>
"#;
        let context = with_virtual_blocks(
            validation_context("a.rs", source),
            vec![
                virtual_block("package.json#/version", &[("name", "version")])?,
                virtual_block("package.json#/versoin", &[])?,
            ],
        );
        let v = validator(&[("package.json", r#"{"version": "1.2.3"}"#)]);

        let error = v.validate(context).unwrap_err();

        assert_eq!(
            format!("{error:#}"),
            "invalid block at line 7 of \"blockwatch.toml\": target package.json#/versoin does \
             not resolve: symbol not found; did you mean: /version"
        );
        Ok(())
    }

    #[test]
    fn block_with_an_unmodified_sibling_target_resolves_it_from_disk() -> anyhow::Result<()> {
        // Diff mode: only the source block is modified, so the sibling target block in the same file
        // is filtered out of the validation context. The target must still be resolved (from disk),
        // not reported as missing.
        let source = "# <block same-as=\":b\">\nvalue = 10\n# </block>\n# <block name=\"b\">\nvalue = 10\n# </block>";
        let context = validation_context_with_changes(
            "config.py",
            source,
            vec![LineChange {
                line: 2,
                kind: LineChangeKind::Added,
            }],
        );
        let v = validator(&[("config.py", source)]);
        assert!(v.validate(context)?.violations.is_empty());
        Ok(())
    }

    #[test]
    fn pattern_in_the_default_mode_ignores_value_order() -> anyhow::Result<()> {
        let context = merge_validation_contexts(vec![
            validation_context(
                "a.rs",
                "// <block same-as=\"b.md:langs\" same-as-pattern=\"(?P<value>[a-z]+)\">\ngo\nrust\n// </block>",
            ),
            validation_context(
                "b.md",
                "[//]: # (<block name=\"langs\" same-as-pattern=\"[a-z]+\">)\n\nrust\ngo\n\n[//]: # (</block>)",
            ),
        ]);
        assert!(validator(&[]).validate(context)?.violations.is_empty());
        Ok(())
    }

    #[test]
    fn pattern_with_a_missing_value_returns_a_violation() -> anyhow::Result<()> {
        let context = merge_validation_contexts(vec![
            validation_context(
                "a.rs",
                "// <block same-as=\"b.md:langs\" same-as-pattern=\"(?P<value>[a-z]+)\">\ngo\nrust\n// </block>",
            ),
            validation_context(
                "b.md",
                "[//]: # (<block name=\"langs\" same-as-pattern=\"[a-z]+\">)\n\ngo\n\n[//]: # (</block>)",
            ),
        ]);
        assert_eq!(validator(&[]).validate(context)?.violations.len(), 1);
        Ok(())
    }

    #[test]
    fn pattern_collects_every_value_on_a_line() -> anyhow::Result<()> {
        // Both sides hold the same two values; only the layout differs.
        let context = merge_validation_contexts(vec![
            validation_context(
                "a.rs",
                "// <block same-as=\"b.md:langs\" same-as-pattern=\"(?P<value>[a-z]+)\">\ngo rust\n// </block>",
            ),
            validation_context(
                "b.md",
                "[//]: # (<block name=\"langs\" same-as-pattern=\"[a-z]+\">)\n\ngo\nrust\n\n[//]: # (</block>)",
            ),
        ]);
        assert!(validator(&[]).validate(context)?.violations.is_empty());
        Ok(())
    }

    #[test]
    fn pattern_compares_values_regardless_of_how_lines_group_them() -> anyhow::Result<()> {
        // Both sides list A, B, C, D in that order, split across lines differently. A pattern
        // compares the values a block yields, not the layout it yields them in, which is what lets
        // two blocks in unrelated formats be compared at all. Regrouping alone is therefore not a
        // disagreement, even in sequence mode.
        let context = merge_validation_contexts(vec![
            validation_context(
                "a.rs",
                "// <block same-as=\"b.md:langs\" same-as-pattern=\"(?P<value>[A-Z]+)\" same-as-mode=\"sequence\">\nA, B\nC, D\n// </block>",
            ),
            validation_context(
                "b.md",
                "[//]: # (<block name=\"langs\" same-as-pattern=\"[A-Z]+\">)\n\nA, B, C\nD\n\n[//]: # (</block>)",
            ),
        ]);
        assert!(validator(&[]).validate(context)?.violations.is_empty());
        Ok(())
    }

    #[test]
    fn pattern_matches_that_are_empty_are_skipped() -> anyhow::Result<()> {
        // "[a-z]*" produces the following matches: "go", "", "", "", "rust", "".
        // The empty ones should be skipped.
        let context = merge_validation_contexts(vec![
            validation_context(
                "a.rs",
                "// <block same-as=\"b.md:langs\" same-as-pattern=\"(?P<value>[a-z]*)\" same-as-mode=\"sequence\">\ngo 1 rust\n// </block>",
            ),
            validation_context(
                "b.md",
                "[//]: # (<block name=\"langs\" same-as-pattern=\"[a-z]*\">)\n\ngo\nrust\n\n[//]: # (</block>)",
            ),
        ]);
        assert!(validator(&[]).validate(context)?.violations.is_empty());
        Ok(())
    }

    #[test]
    fn pattern_with_a_value_group_not_in_the_match_skips_the_match() -> anyhow::Result<()> {
        // The `none` line matches the branch without the `value` group.
        let context = validation_context(
            "a.py",
            r#"# <block same-as=":b" same-as-pattern="^(?:none|(?P<value>\w+))$">
none
red
# </block>
# <block name="b">
red
# </block>
"#,
        );
        assert!(validator(&[]).validate(context)?.violations.is_empty());
        Ok(())
    }

    #[test]
    fn pattern_with_several_groups_and_a_swapped_value_returns_a_violation() -> anyhow::Result<()> {
        // The values alone agree: each side holds 1 and 2. Only `red` and `blue` hold them the other
        // way round.
        let context = validation_context(
            "a.py",
            r#"# <block same-as=":b" same-as-pattern="(?P<name>\w+) = (?P<value>\d+)">
red = 1
blue = 2
# </block>
# <block name="b" same-as-pattern="(?P<name>\w+): (?P<value>\d+)">
red: 2
blue: 1
# </block>
"#,
        );
        let violations = validator(&[]).validate(context)?.violations;
        let file = violations.get(&RepoPath::from_reference("a.py")?).unwrap();
        assert_eq!(file.len(), 1);
        assert!(
            file[0].message.ends_with(
                r#"[{name: "red", value: "1"}, {name: "blue", value: "2"}] != [{name: "red", value: "2"}, {name: "blue", value: "1"}]"#
            ),
            "unexpected message: {}",
            file[0].message
        );
        Ok(())
    }

    #[test]
    fn pattern_with_groups_in_a_different_order_returns_no_violations() -> anyhow::Result<()> {
        // Groups match by name, not by their place in the pattern, so the target can write the
        // value first.
        let context = validation_context(
            "a.py",
            r#"# <block same-as=":b" same-as-pattern="(?P<name>\w+) = (?P<value>\d+)">
red = 1
blue = 2
# </block>
# <block name="b" same-as-pattern="(?P<value>\d+) is (?P<name>\w+)">
2 is blue
1 is red
# </block>
"#,
        );
        assert!(validator(&[]).validate(context)?.violations.is_empty());
        Ok(())
    }

    #[test]
    fn patterns_with_different_groups_return_a_violation() -> anyhow::Result<()> {
        // Records with different groups are never equal, so the message says that instead of
        // listing both sides.
        let context = validation_context(
            "a.py",
            r#"# <block same-as=":b" same-as-pattern="(?P<name>\w+) = (?P<value>\d+)">
red = 1
# </block>
# <block name="b" same-as-pattern="(?P<value>\d+)">
red: 1
# </block>
"#,
        );
        let violations = validator(&[]).validate(context)?.violations;
        let file = violations.get(&RepoPath::from_reference("a.py")?).unwrap();
        assert_eq!(file.len(), 1);
        assert!(
            file[0]
                .message
                .ends_with("same-as-pattern groups differ: name, value != value"),
            "unexpected message: {}",
            file[0].message
        );
        Ok(())
    }

    #[test]
    fn pattern_with_another_group_not_in_the_match_skips_the_match() -> anyhow::Result<()> {
        // The `3` line matches without the `name` group.
        let context = validation_context(
            "a.py",
            r#"# <block same-as=":b" same-as-pattern="^(?:(?P<name>\w+) = )?(?P<value>\d+)$">
red = 1
3
# </block>
# <block name="b" same-as-pattern="(?P<name>\w+): (?P<value>\d+)">
red: 1
# </block>
"#,
        );
        assert!(validator(&[]).validate(context)?.violations.is_empty());
        Ok(())
    }

    #[test]
    fn pattern_matching_nothing_on_both_sides_returns_a_violation() -> anyhow::Result<()> {
        // Neither block contains a digit, so the pattern extracts zero items on both sides.
        let context = merge_validation_contexts(vec![
            validation_context(
                "a.rs",
                "// <block same-as=\"b.md:langs\" same-as-pattern=\"[0-9]+\">\ngo\nrust\n// </block>",
            ),
            validation_context(
                "b.md",
                "[//]: # (<block name=\"langs\" same-as-pattern=\"[0-9]+\">)\n\nrust\ngo\n\n[//]: # (</block>)",
            ),
        ]);
        assert_eq!(validator(&[]).validate(context)?.violations.len(), 1);
        Ok(())
    }

    #[test]
    fn pattern_matching_nothing_in_the_block_returns_a_violation() -> anyhow::Result<()> {
        // An empty list is a subset of any list, so `subset` would pass whatever the target holds.
        let context = validation_context(
            "a.py",
            r#"# <block same-as=":b" same-as-mode="subset" same-as-pattern="id=(?P<value>\d+)">
ids = 1, 2
# </block>
# <block name="b" same-as-pattern="id=(?P<value>\d+)">
id=1
# </block>
"#,
        );
        let violations = validator(&[]).validate(context)?.violations;
        let file = violations.get(&RepoPath::from_reference("a.py")?).unwrap();
        assert_eq!(file.len(), 1);
        assert!(
            file[0].message.contains("matched no values"),
            "unexpected message: {}",
            file[0].message
        );
        Ok(())
    }

    #[test]
    fn pattern_matching_nothing_in_the_target_returns_a_violation() -> anyhow::Result<()> {
        // The values alone, `["1"] != []`, would not say that the target's pattern found nothing.
        let context = validation_context(
            "a.py",
            r#"# <block same-as=":b" same-as-pattern="id=(?P<value>\d+)">
id=1
# </block>
# <block name="b" same-as-pattern="id=(?P<value>\d+)">
ids = 1, 2
# </block>
"#,
        );
        let violations = validator(&[]).validate(context)?.violations;
        let file = violations.get(&RepoPath::from_reference("a.py")?).unwrap();
        assert_eq!(file.len(), 1);
        assert!(
            file[0].message.contains("matched no values"),
            "unexpected message: {}",
            file[0].message
        );
        Ok(())
    }

    #[test]
    fn pattern_with_a_line_break_returns_an_error() {
        let context = validation_context(
            "config.py",
            "# <block same-as=\":b\" same-as-pattern=\"port\n# (?P<value>\\d+)\">\nport 80\n# </block>\n# <block name=\"b\">\n80\n# </block>",
        );

        let err = validator(&[]).validate(context).unwrap_err();
        assert!(
            format!("{err:#}").contains("line break"),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn set_mode_with_repeated_values_lists_each_once_in_first_seen_order() -> anyhow::Result<()> {
        let context = validation_context(
            "a.rs",
            "// <block same-as=\":b\" same-as-pattern=\"(?P<value>[a-z]+)\">\ngamma\nbeta\ngamma\nbeta\n// </block>\n// <block name=\"b\" same-as-pattern=\"(?P<value>[a-z]+)\">\ndelta\nalpha\ndelta\n// </block>",
        );
        let violations = validator(&[]).validate(context)?.violations;
        let file = violations.get(&RepoPath::from_reference("a.rs")?).unwrap();
        assert_eq!(file.len(), 1);
        assert!(
            file[0]
                .message
                .ends_with(r#"["gamma", "beta"] != ["delta", "alpha"]"#),
            "unexpected message: {}",
            file[0].message
        );
        Ok(())
    }

    #[test]
    fn sequence_mode_with_reordered_values_returns_a_violation() -> anyhow::Result<()> {
        let context = merge_validation_contexts(vec![
            validation_context(
                "a.rs",
                "// <block same-as=\"b.md:l\" same-as-pattern=\"(?P<value>[a-z]+)\" same-as-mode=\"sequence\">\ngo\nrust\n// </block>",
            ),
            validation_context(
                "b.md",
                "[//]: # (<block name=\"l\" same-as-pattern=\"[a-z]+\">)\n\nrust\ngo\n\n[//]: # (</block>)",
            ),
        ]);
        assert_eq!(validator(&[]).validate(context)?.violations.len(), 1);
        Ok(())
    }

    #[test]
    fn single_mode_with_an_extra_value_returns_a_violation() -> anyhow::Result<()> {
        let context = validation_context(
            "a.rs",
            "// <block same-as=\":b\" same-as-pattern=\"(?P<value>[0-9]+)\" same-as-mode=\"single\">\n1\n2\n// </block>\n// <block name=\"b\">\n1\n// </block>",
        );
        let violations = validator(&[]).validate(context)?.violations;
        let file = violations.get(&RepoPath::from_reference("a.rs")?).unwrap();
        assert_eq!(file.len(), 1);
        assert_eq!(file[0].code, Validator::SameAs);
        Ok(())
    }

    #[test]
    fn single_mode_with_several_groups_and_a_different_value_returns_a_violation()
    -> anyhow::Result<()> {
        let context = validation_context(
            "a.py",
            r#"# <block same-as=":b" same-as-mode="single" same-as-pattern="(?P<name>\w+) = (?P<value>\d+)">
red = 1
# </block>
# <block name="b" same-as-pattern="(?P<name>\w+): (?P<value>\d+)">
red: 2
# </block>
"#,
        );
        let violations = validator(&[]).validate(context)?.violations;
        let file = violations.get(&RepoPath::from_reference("a.py")?).unwrap();
        assert_eq!(file.len(), 1);
        assert!(
            file[0]
                .message
                .ends_with(r#"{name: "red", value: "1"} != {name: "red", value: "2"}"#),
            "unexpected message: {}",
            file[0].message
        );
        Ok(())
    }

    #[test]
    fn subset_mode_with_all_values_present_returns_no_violations() -> anyhow::Result<()> {
        let context = merge_validation_contexts(vec![
            validation_context(
                "test.rs",
                "// <block same-as=\"src.rs:vars\" same-as-mode=\"subset\" same-as-pattern=\"(?P<value>[A-Z_]+)\">\nAPI_KEY\nAPI_URL\n// </block>",
            ),
            validation_context(
                "src.rs",
                "// <block name=\"vars\" same-as-pattern=\"(?P<value>[A-Z_]+)\">\nAPI_KEY\nAPI_URL\n// </block>",
            ),
        ]);
        assert!(validator(&[]).validate(context)?.violations.is_empty());
        Ok(())
    }

    #[test]
    fn subset_mode_with_a_missing_value_returns_a_violation() -> anyhow::Result<()> {
        let context = merge_validation_contexts(vec![
            validation_context(
                "test.rs",
                "// <block same-as=\"src.rs:vars\" same-as-mode=\"subset\" same-as-pattern=\"(?P<value>[A-Z_]+)\">\nAPI_KEY\nEXTRA\n// </block>",
            ),
            validation_context(
                "src.rs",
                "// <block name=\"vars\" same-as-pattern=\"(?P<value>[A-Z_]+)\">\nAPI_KEY\n// </block>",
            ),
        ]);
        let violations = validator(&[]).validate(context)?.violations;
        let file = violations
            .get(&RepoPath::from_reference("test.rs")?)
            .unwrap();
        assert_eq!(file.len(), 1);
        assert_eq!(file[0].code, Validator::SameAs);
        Ok(())
    }

    #[test]
    fn subset_mode_with_repeated_missing_values_lists_each_once_in_first_seen_order()
    -> anyhow::Result<()> {
        let context = validation_context(
            "a.rs",
            "// <block same-as=\":b\" same-as-mode=\"subset\" same-as-pattern=\"(?P<value>[a-z]+)\">\ngamma\nalpha\nbeta\ngamma\nbeta\n// </block>\n// <block name=\"b\">\nalpha\n// </block>",
        );
        let violations = validator(&[]).validate(context)?.violations;
        let file = violations.get(&RepoPath::from_reference("a.rs")?).unwrap();
        assert_eq!(file.len(), 1);
        assert!(
            file[0]
                .message
                .ends_with(r#"missing from target: ["gamma", "beta"]"#),
            "unexpected message: {}",
            file[0].message
        );
        Ok(())
    }

    #[test]
    fn subset_mode_with_a_superset_source_returns_a_violation() -> anyhow::Result<()> {
        // Subset is not symmetric: a superset on the source side fails even though the reverse
        // relation would hold.
        let context = merge_validation_contexts(vec![
            validation_context(
                "test.rs",
                "// <block same-as=\"src.rs:vars\" same-as-mode=\"subset\" same-as-pattern=\"(?P<value>[A-Z_]+)\">\nAPI_KEY\nAPI_URL\n// </block>",
            ),
            validation_context(
                "src.rs",
                "// <block name=\"vars\" same-as-pattern=\"(?P<value>[A-Z_]+)\">\nAPI_KEY\n// </block>",
            ),
        ]);
        assert_eq!(validator(&[]).validate(context)?.violations.len(), 1);
        Ok(())
    }

    #[test]
    fn numeric_format_with_equal_values_written_differently_returns_no_violations()
    -> anyhow::Result<()> {
        let context = merge_validation_contexts(vec![
            validation_context(
                "a.yaml",
                "# <block same-as=\"b.rs:port\" same-as-pattern=\"(?P<value>[0-9]+)\" same-as-format=\"numeric\" same-as-mode=\"single\">\nport: 8080\n# </block>",
            ),
            validation_context(
                "b.rs",
                "// <block name=\"port\" same-as-pattern=\"(?P<value>[0-9.]+)\">\n8080.0\n// </block>",
            ),
        ]);
        assert!(validator(&[]).validate(context)?.violations.is_empty());
        Ok(())
    }

    #[test]
    fn numeric_format_with_a_non_numeric_value_returns_a_violation() -> anyhow::Result<()> {
        // A non-numeric token under `same-as-format=numeric` is a shape mismatch of the asserted
        // value, reported as a violation on the block rather than aborting the run.
        let context = validation_context(
            "a.rs",
            "// <block same-as=\":b\" same-as-pattern=\"(?P<value>\\w+)\" same-as-format=\"numeric\">\nabc\n// </block>\n// <block name=\"b\">\n1\n// </block>",
        );
        let violations = validator(&[]).validate(context)?.violations;
        let file = violations.get(&RepoPath::from_reference("a.rs")?).unwrap();
        assert_eq!(file.len(), 1);
        assert_eq!(file[0].code, Validator::SameAs);
        Ok(())
    }

    #[test]
    fn numeric_format_with_integers_beyond_float_precision_returns_a_violation()
    -> anyhow::Result<()> {
        // Both identifiers round to the same floating point number, so comparing them as floats
        // would call two different ids equal.
        let context = validation_context(
            "a.py",
            "# <block same-as=\":b\" same-as-pattern=\"(?P<value>\\d+)\" same-as-format=\"numeric\" same-as-mode=\"single\">\nid = 9007199254740992\n# </block>\n# <block name=\"b\" same-as-pattern=\"(?P<value>\\d+)\">\nid = 9007199254740993\n# </block>",
        );
        let violations = validator(&[]).validate(context)?.violations;
        let file = violations.get(&RepoPath::from_reference("a.py")?).unwrap();
        assert_eq!(file.len(), 1);
        assert_eq!(file[0].code, Validator::SameAs);
        assert!(
            file[0]
                .message
                .contains("9007199254740992 != 9007199254740993"),
            "unexpected message: {}",
            file[0].message
        );
        Ok(())
    }

    #[test]
    fn numeric_format_with_values_beyond_float_range_returns_a_violation() -> anyhow::Result<()> {
        // Values this large overflow a floating point number to infinity, which makes every one of
        // them compare equal to the others.
        let context = validation_context(
            "a.py",
            "# <block same-as=\":b\" same-as-format=\"numeric\" same-as-mode=\"single\">\n1e400\n# </block>\n# <block name=\"b\">\n1e500\n# </block>",
        );
        let violations = validator(&[]).validate(context)?.violations;
        let file = violations.get(&RepoPath::from_reference("a.py")?).unwrap();
        assert_eq!(file.len(), 1);
        assert_eq!(file[0].code, Validator::SameAs);
        Ok(())
    }

    #[test]
    fn numeric_format_ignores_digit_separators() -> anyhow::Result<()> {
        // The same quantity spelled with and without separators, as two languages would write it.
        let context = validation_context(
            "a.rs",
            "// <block same-as=\":b\" same-as-format=\"numeric\" same-as-mode=\"single\">\n1_000_000\n// </block>\n// <block name=\"b\">\n1000000\n// </block>",
        );
        assert!(validator(&[]).validate(context)?.violations.is_empty());
        Ok(())
    }

    #[test]
    fn numeric_format_with_a_misplaced_digit_separator_returns_a_violation() -> anyhow::Result<()> {
        let context = validation_context(
            "a.rs",
            "// <block same-as=\":b\" same-as-format=\"numeric\" same-as-mode=\"single\">\n1_000_\n// </block>\n// <block name=\"b\">\n1000\n// </block>",
        );
        let violations = validator(&[]).validate(context)?.violations;
        let file = violations.get(&RepoPath::from_reference("a.rs")?).unwrap();
        assert_eq!(file.len(), 1);
        assert!(
            file[0].message.contains("is not a number"),
            "unexpected message: {}",
            file[0].message
        );
        Ok(())
    }

    #[test]
    fn numeric_format_with_an_infinite_value_returns_a_violation() -> anyhow::Result<()> {
        // Infinity and NaN are float-specific spellings, not numbers a source file can hold, so
        // they are rejected like any other non-numeric token even when both sides spell them the
        // same way.
        let context = validation_context(
            "a.py",
            "# <block same-as=\":b\" same-as-format=\"numeric\" same-as-mode=\"single\">\ninf\n# </block>\n# <block name=\"b\">\ninf\n# </block>",
        );
        let violations = validator(&[]).validate(context)?.violations;
        let file = violations.get(&RepoPath::from_reference("a.py")?).unwrap();
        assert_eq!(file.len(), 1);
        assert!(
            file[0].message.contains("is not a number"),
            "unexpected message: {}",
            file[0].message
        );
        Ok(())
    }

    #[test]
    fn identifier_format_with_one_name_in_different_styles_returns_no_violations()
    -> anyhow::Result<()> {
        let context = validation_context(
            "a.rs",
            r#"// <block same-as=":b" same-as-pattern="(?P<value>\w+)," same-as-format="identifier">
PreExistingFailure,
ToolBlocked,
RetryLimit,
// </block>
// <block name="b" same-as-pattern="(?P<value>\S+)">
pre-existing-failure
TOOL_BLOCKED
retryLimit
// </block>
"#,
        );
        assert!(validator(&[]).validate(context)?.violations.is_empty());
        Ok(())
    }

    #[test]
    fn identifier_format_with_an_acronym_returns_no_violations() -> anyhow::Result<()> {
        // A run of capitals is one word, so `HTTP` is not split letter by letter.
        let context = validation_context(
            "a.rs",
            r#"// <block same-as=":b" same-as-format="identifier">
HTTPServer
// </block>
// <block name="b">
http-server
// </block>
"#,
        );
        assert!(validator(&[]).validate(context)?.violations.is_empty());
        Ok(())
    }

    #[test]
    fn identifier_format_with_words_split_differently_returns_a_violation() -> anyhow::Result<()> {
        // The same letters split into different words are different names. For example, serde
        // turns `Signin` into `signin`, so docs that say `sign-in` are wrong.
        let context = validation_context(
            "a.rs",
            r#"// <block same-as=":b" same-as-format="identifier">
Signin
// </block>
// <block name="b">
sign-in
// </block>
"#,
        );
        let violations = validator(&[]).validate(context)?.violations;
        let file = violations.get(&RepoPath::from_reference("a.rs")?).unwrap();
        assert_eq!(file.len(), 1);
        assert!(
            file[0].message.contains(r#"["signin"] != ["sign_in"]"#),
            "unexpected message: {}",
            file[0].message
        );
        Ok(())
    }

    #[test]
    fn flat_format_with_one_name_in_different_styles_returns_no_violations() -> anyhow::Result<()> {
        let context = validation_context(
            "a.rs",
            r#"// <block same-as=":b" same-as-pattern="(?P<value>\w+)," same-as-format="flat">
DarkRed,
LightBlue,
PaleGreen,
// </block>
// <block name="b" same-as-pattern="(?P<value>\S+)">
darkred
light-blue
PALE_GREEN
// </block>
"#,
        );
        assert!(validator(&[]).validate(context)?.violations.is_empty());
        Ok(())
    }

    #[test]
    fn flat_format_with_words_split_differently_returns_no_violations() -> anyhow::Result<()> {
        // Unlike `same-as-format="identifier"`, where the words break doesn't count.
        let context = validation_context(
            "a.rs",
            r#"// <block same-as=":b" same-as-format="flat">
Signin
// </block>
// <block name="b">
sign-in
// </block>
"#,
        );
        assert!(validator(&[]).validate(context)?.violations.is_empty());
        Ok(())
    }

    #[test]
    fn flat_format_with_a_missing_digit_returns_a_violation() -> anyhow::Result<()> {
        let context = validation_context(
            "a.rs",
            r#"// <block same-as=":b" same-as-format="flat">
DarkRed2
// </block>
// <block name="b">
dark-red
// </block>
"#,
        );
        let violations = validator(&[]).validate(context)?.violations;
        let file = violations.get(&RepoPath::from_reference("a.rs")?).unwrap();
        assert_eq!(file.len(), 1);
        assert!(
            file[0].message.contains(r#"["darkred2"] != ["darkred"]"#),
            "unexpected message: {}",
            file[0].message
        );
        Ok(())
    }

    #[test]
    fn flat_format_with_a_missing_accent_returns_a_violation() -> anyhow::Result<()> {
        let context = validation_context(
            "a.rs",
            r#"// <block same-as=":b" same-as-format="flat">
CAFÉ
// </block>
// <block name="b">
cafe
// </block>
"#,
        );
        let violations = validator(&[]).validate(context)?.violations;
        let file = violations.get(&RepoPath::from_reference("a.rs")?).unwrap();
        assert_eq!(file.len(), 1);
        assert!(
            file[0].message.contains(r#"["café"] != ["cafe"]"#),
            "unexpected message: {}",
            file[0].message
        );
        Ok(())
    }

    #[test]
    fn unknown_same_as_format_value_returns_an_error() {
        // Unlike bad content, an unrecognized `same-as-format` value is an authoring error and
        // aborts the run, mirroring how an unknown `same-as-mode` is handled. The error shows the
        // block, because nothing else in it leads to the tag.
        let context = validation_context(
            "a.rs",
            "// <block same-as=\":b\" same-as-format=\"number\">\n1\n// </block>\n// <block name=\"b\">\n1\n// </block>",
        );

        let error = validator(&[]).validate(context).unwrap_err();

        assert_eq!(
            format!("{error:#}"),
            "invalid same-as attributes on block a.rs:(unnamed) at line 1: invalid same-as-format \
             \"number\" (expected numeric, identifier or flat)"
        );
    }

    #[test]
    fn one_format_with_several_groups_applies_to_every_group() -> anyhow::Result<()> {
        let context = validation_context(
            "a.py",
            r#"# <block same-as=":b" same-as-format="numeric" same-as-pattern="(?P<name>\w+) = (?P<value>\d+)">
red = 1
# </block>
# <block name="b" same-as-pattern="(?P<name>\w+): (?P<value>\d+)">
red: 1
# </block>
"#,
        );
        let violations = validator(&[]).validate(context)?.violations;
        let file = violations.get(&RepoPath::from_reference("a.py")?).unwrap();
        assert_eq!(file.len(), 1);
        assert!(
            file[0]
                .message
                .ends_with(r#"same-as-format=numeric but "red" is not a number"#),
            "unexpected message: {}",
            file[0].message
        );
        Ok(())
    }

    #[test]
    fn format_for_one_group_compares_the_other_groups_as_written() -> anyhow::Result<()> {
        // `1.0` reads as the number 1, but `Red` and `red` differ as written.
        let context = validation_context(
            "a.py",
            r#"# <block same-as=":b" same-as-format="value:numeric" same-as-pattern="(?P<name>\w+) = (?P<value>\d+)">
Red = 1
# </block>
# <block name="b" same-as-pattern="(?P<name>\w+): (?P<value>[\d.]+)">
red: 1.0
# </block>
"#,
        );
        let violations = validator(&[]).validate(context)?.violations;
        let file = violations.get(&RepoPath::from_reference("a.py")?).unwrap();
        assert_eq!(file.len(), 1);
        assert!(
            file[0]
                .message
                .ends_with(r#"[{name: "Red", value: "1"}] != [{name: "red", value: "1"}]"#),
            "unexpected message: {}",
            file[0].message
        );
        Ok(())
    }

    #[test]
    fn formats_for_several_groups_return_no_violations() -> anyhow::Result<()> {
        // A space after the comma is allowed, as it is between `same-as` targets.
        let context = validation_context(
            "a.py",
            r#"# <block same-as=":b" same-as-format="name:identifier, value:numeric" same-as-pattern="(?P<name>\w+) = (?P<value>\d+)">
RedColor = 1
# </block>
# <block name="b" same-as-pattern="(?P<name>[\w-]+): (?P<value>[\d.]+)">
red-color: 1.0
# </block>
"#,
        );
        assert!(validator(&[]).validate(context)?.violations.is_empty());
        Ok(())
    }

    #[test]
    fn format_for_the_value_group_of_a_block_without_a_pattern_returns_no_violations()
    -> anyhow::Result<()> {
        // Without a pattern, the whole content is the one group `value`.
        let context = validation_context(
            "a.py",
            r#"# <block same-as=":b" same-as-format="value:numeric">
1
# </block>
# <block name="b">
1.0
# </block>
"#,
        );
        assert!(validator(&[]).validate(context)?.violations.is_empty());
        Ok(())
    }

    #[test]
    fn format_for_a_group_the_pattern_does_not_have_returns_an_error() {
        // A typo in a group name would otherwise compare that group as written, with no sign that
        // its format is lost.
        let context = validation_context(
            "a.py",
            r#"# <block same-as=":b" same-as-format="vaule:numeric" same-as-pattern="(?P<name>\w+) = (?P<value>\d+)">
red = 1
# </block>
# <block name="b" same-as-pattern="(?P<name>\w+): (?P<value>\d+)">
red: 1
# </block>
"#,
        );

        let error = validator(&[]).validate(context).unwrap_err();

        assert!(
            format!("{error:#}").ends_with(
                r#"same-as-format refers to group "vaule", which same-as-pattern doesn't have"#
            ),
            "unexpected error: {error:#}"
        );
    }

    #[test]
    fn unknown_format_for_a_group_returns_an_error() {
        let context = validation_context(
            "a.py",
            r#"# <block same-as=":b" same-as-format="value:numbr">
1
# </block>
# <block name="b">
1
# </block>
"#,
        );

        let error = validator(&[]).validate(context).unwrap_err();

        assert!(
            format!("{error:#}").ends_with(
                r#"invalid same-as-format "numbr" (expected numeric, identifier or flat)"#
            ),
            "unexpected error: {error:#}"
        );
    }

    #[test]
    fn one_format_mixed_with_formats_for_groups_returns_an_error() {
        let context = validation_context(
            "a.py",
            r#"# <block same-as=":b" same-as-format="identifier,value:numeric" same-as-pattern="(?P<name>\w+) = (?P<value>\d+)">
red = 1
# </block>
# <block name="b" same-as-pattern="(?P<name>\w+): (?P<value>\d+)">
red: 1
# </block>
"#,
        );

        let error = validator(&[]).validate(context).unwrap_err();

        assert!(
            format!("{error:#}").ends_with(
                r#"invalid same-as-format "identifier,value:numeric" (expected one format, or group:format entries such as value:numeric)"#
            ),
            "unexpected error: {error:#}"
        );
    }

    #[test]
    fn block_equal_to_a_whole_file_target_returns_no_violations() -> anyhow::Result<()> {
        // The target is a format with no grammar, so it can only be referenced as a whole file.
        let context = validation_context(
            "version.rs",
            "// <block same-as=\"VERSION\">\n1.2.3\n// </block>",
        );

        let report = validator(&[("VERSION", "1.2.3\n")]).validate(context)?;

        assert_eq!(violation_count(&report), 0);
        Ok(())
    }

    #[test]
    fn block_differing_from_a_whole_file_target_returns_a_violation() -> anyhow::Result<()> {
        let context = validation_context(
            "version.rs",
            "// <block same-as=\"VERSION\">\n1.2.3\n// </block>",
        );

        let violations = validator(&[("VERSION", "9.9.9\n")])
            .validate(context)?
            .violations;

        let file = violations
            .get(&RepoPath::from_reference("version.rs")?)
            .unwrap();
        assert_eq!(file.len(), 1);
        assert!(
            file[0]
                .message
                .starts_with("Block version.rs:(unnamed) at line 1 disagrees with file VERSION:"),
            "unexpected message: {}",
            file[0].message
        );
        assert_eq!(
            file[0].data.as_ref().unwrap()["target_file"],
            serde_json::json!("VERSION")
        );
        assert!(
            file[0].data.as_ref().unwrap().get("target_name").is_none(),
            "a whole-file target has no block name"
        );
        Ok(())
    }

    #[test]
    fn same_as_pattern_selects_values_from_a_whole_file_target() -> anyhow::Result<()> {
        // The file has no block to carry a pattern of its own, so the referencing block's applies.
        let context = validation_context(
            "version.rs",
            "// <block same-as=\"package.json\" same-as-pattern=\"\\d+\\.\\d+\\.\\d+\">\nconst VERSION: &str = \"1.2.3\";\n// </block>",
        );

        let report = validator(&[(
            "package.json",
            "{\n  \"name\": \"example\",\n  \"version\": \"1.2.3\"\n}\n",
        )])
        .validate(context)?;

        assert_eq!(violation_count(&report), 0);
        Ok(())
    }

    #[test]
    fn whole_file_target_that_is_missing_returns_an_error() -> anyhow::Result<()> {
        let context = validation_context(
            "version.rs",
            "// <block same-as=\"gone.json\">\n1.2.3\n// </block>",
        );

        assert!(validator(&[]).validate(context).is_err());
        Ok(())
    }

    /// The target file of the symbol reference tests.
    const PACKAGE_JSON: &str = r#"{"dependencies": {"inngest": "4.18.1"}}"#;

    #[test]
    fn symbol_with_an_equal_value_returns_no_violations() -> anyhow::Result<()> {
        let context = validation_context(
            "deps.py",
            r#"# <block same-as="package.json#/dependencies/inngest">
4.18.1
# </block>"#,
        );

        let report = validator(&[("package.json", PACKAGE_JSON)]).validate(context)?;

        assert_eq!(violation_count(&report), 0);
        Ok(())
    }

    #[test]
    fn symbol_with_a_different_value_returns_a_violation() -> anyhow::Result<()> {
        let context = validation_context(
            "deps.py",
            r#"# <block same-as="package.json#/dependencies/inngest">
4.18.0
# </block>"#,
        );

        let violations = validator(&[("package.json", PACKAGE_JSON)])
            .validate(context)?
            .violations;

        let file = violations
            .get(&RepoPath::from_reference("deps.py")?)
            .unwrap();
        assert_eq!(file.len(), 1);
        assert!(
            file[0]
                .message
                .contains("disagrees with package.json#/dependencies/inngest:"),
            "unexpected message: {}",
            file[0].message
        );
        let data = file[0].data.as_ref().unwrap();
        assert_eq!(data["target_file"], serde_json::json!("package.json"));
        assert_eq!(
            data["target_name"],
            serde_json::json!("#/dependencies/inngest")
        );
        Ok(())
    }

    #[test]
    fn symbol_without_a_value_compares_the_text_of_its_definition() -> anyhow::Result<()> {
        // An object has no value, so the pattern reads the version out of the object's text.
        let context = validation_context(
            "deps.py",
            r#"# <block same-as="package.json#/dependencies" same-as-pattern="\d+\.\d+\.\d+">
4.18.1
# </block>"#,
        );

        let report = validator(&[("package.json", PACKAGE_JSON)]).validate(context)?;

        assert_eq!(violation_count(&report), 0);
        Ok(())
    }

    #[test]
    fn missing_symbol_returns_a_violation_with_hints() -> anyhow::Result<()> {
        let context = validation_context(
            "deps.py",
            r#"# <block same-as="package.json#/dependencies/ingest">
4.18.1
# </block>"#,
        );

        let violations = validator(&[("package.json", PACKAGE_JSON)])
            .validate(context)?
            .violations;

        let message = &violations[&RepoPath::from_reference("deps.py")?][0].message;
        assert!(
            message
                .ends_with("symbol not found; did you mean: /dependencies/inngest, /dependencies"),
            "unexpected message: {message}"
        );
        Ok(())
    }

    #[test]
    fn ambiguous_symbol_returns_a_violation_with_its_positions() -> anyhow::Result<()> {
        let context = validation_context(
            "deps.py",
            r#"# <block same-as="config.json#/v">
1
# </block>"#,
        );

        let violations = validator(&[("config.json", r#"{"v": 1, "v": 2}"#)])
            .validate(context)?
            .violations;

        let message = &violations[&RepoPath::from_reference("deps.py")?][0].message;
        assert!(
            message.ends_with("ambiguous symbol, defined at 1:2, 1:10"),
            "unexpected message: {message}"
        );
        Ok(())
    }

    #[test]
    fn symbol_without_a_file_resolves_in_the_referencing_file() -> anyhow::Result<()> {
        let context = validation_context(
            "config.json",
            r##"{
  // <block same-as="#/version" same-as-pattern="\d+\.\d+\.\d+">
  "docs_version": "1.0.0",
  // </block>
  "version": "1.0.0"
}"##,
        );

        let report = validator(&[]).validate(context)?;

        assert_eq!(violation_count(&report), 0);
        Ok(())
    }

    /// The repository of the `same-as-paths` tests: two services, one with a nested directory.
    const SERVICES: &[(&str, &str)] = &[
        ("services/auth/README.md", ""),
        ("services/billing/README.md", ""),
        ("services/billing/src/main.rs", ""),
    ];

    #[test]
    fn paths_target_with_an_unlisted_directory_returns_a_violation() -> anyhow::Result<()> {
        let context = validation_context(
            "README.md",
            r#"<!-- <block same-as-paths="services/*/" same-as-pattern="services/(?P<value>[a-z]+)"> -->
- [billing](services/billing)
<!-- </block> -->"#,
        );

        let violations = validator(SERVICES).validate(context)?.violations;

        let file = &violations[&RepoPath::from_reference("README.md")?];
        assert_eq!(file.len(), 1);
        assert!(
            file[0].message.ends_with(
                r#"disagrees with paths matching services/*/: ["billing"] != ["auth", "billing"]"#
            ),
            "unexpected message: {}",
            file[0].message
        );
        let data = file[0].data.as_ref().unwrap();
        assert_eq!(data["target_paths"], serde_json::json!("services/*/"));
        assert!(
            data.get("target_file").is_none(),
            "a paths target has no file"
        );
        Ok(())
    }

    #[test]
    fn paths_target_lists_each_match_once_in_sorted_order() -> anyhow::Result<()> {
        // `services/billing/` holds two files but is listed once. `*` stops at `/`, so
        // `services/billing/src/` is not listed. The `sequence` mode shows the repeats that the
        // default mode would hide.
        let context = validation_context(
            "README.md",
            r#"<!-- <block same-as-paths="services/*/" same-as-pattern=".+" same-as-mode="sequence"> -->
services/billing/
<!-- </block> -->"#,
        );

        let violations = validator(SERVICES).validate(context)?.violations;

        let message = &violations[&RepoPath::from_reference("README.md")?][0].message;
        assert!(
            message
                .ends_with(r#"["services/billing/"] != ["services/auth/", "services/billing/"]"#),
            "unexpected message: {message}"
        );
        Ok(())
    }

    #[test]
    fn paths_target_without_a_trailing_slash_lists_files() -> anyhow::Result<()> {
        let context = validation_context(
            "README.md",
            r#"<!-- <block same-as-paths="crates/*/AGENTS.md" same-as-pattern=".+"> -->
crates/b/AGENTS.md
<!-- </block> -->"#,
        );

        let violations = validator(&[
            ("crates/a/AGENTS.md", ""),
            ("crates/b/Cargo.toml", ""),
            ("crates/b/docs/AGENTS.md", ""),
        ])
        .validate(context)?
        .violations;

        let message = &violations[&RepoPath::from_reference("README.md")?][0].message;
        assert!(
            message.ends_with(r#"["crates/b/AGENTS.md"] != ["crates/a/AGENTS.md"]"#),
            "unexpected message: {message}"
        );
        Ok(())
    }

    #[test]
    fn paths_target_without_glob_characters_lists_that_one_file() -> anyhow::Result<()> {
        let context = validation_context(
            "README.md",
            r#"<!-- <block same-as-paths="docs/INDEX.md" same-as-pattern=".+"> -->
docs/other.md
<!-- </block> -->"#,
        );

        let violations = validator(&[("docs/INDEX.md", ""), ("docs/other.md", "")])
            .validate(context)?
            .violations;

        let message = &violations[&RepoPath::from_reference("README.md")?][0].message;
        assert!(
            message.ends_with(r#"["docs/other.md"] != ["docs/INDEX.md"]"#),
            "unexpected message: {message}"
        );
        Ok(())
    }

    #[test]
    fn paths_target_with_braces_lists_every_alternative() -> anyhow::Result<()> {
        let context = validation_context(
            "README.md",
            r#"<!-- <block same-as-paths="{services,workers}/*/" same-as-pattern=".+"> -->
services/auth/
<!-- </block> -->"#,
        );

        let violations = validator(&[("services/auth/x", ""), ("workers/mail/x", "")])
            .validate(context)?
            .violations;

        let message = &violations[&RepoPath::from_reference("README.md")?][0].message;
        assert!(
            message.ends_with(r#"["services/auth/"] != ["services/auth/", "workers/mail/"]"#),
            "unexpected message: {message}"
        );
        Ok(())
    }

    #[test]
    fn paths_target_matching_nothing_returns_a_violation() -> anyhow::Result<()> {
        let context = validation_context(
            "README.md",
            r#"<!-- <block same-as-paths="servces/*/" same-as-pattern=".+"> -->
services/auth/
<!-- </block> -->"#,
        );

        let violations = validator(SERVICES).validate(context)?.violations;

        let message = &violations[&RepoPath::from_reference("README.md")?][0].message;
        assert!(
            message.ends_with("disagrees with paths matching servces/*/: no paths match"),
            "unexpected message: {message}"
        );
        Ok(())
    }

    #[test]
    fn paths_target_without_a_pattern_returns_an_error() {
        let context = validation_context(
            "README.md",
            r#"<!-- <block same-as-paths="services/*/"> -->
services/auth/
<!-- </block> -->"#,
        );

        let error = validator(SERVICES).validate(context).unwrap_err();

        assert_eq!(
            format!("{error:#}"),
            "same-as-paths needs same-as-pattern on block README.md:(unnamed) at line 1. To \
             compare whole lines, use same-as-pattern=\".+\""
        );
    }

    #[test]
    fn paths_target_with_an_invalid_glob_returns_an_error() {
        let context = validation_context(
            "README.md",
            r#"<!-- <block same-as-paths="services/[a" same-as-pattern=".+"> -->
services/auth/
<!-- </block> -->"#,
        );

        let error = validator(SERVICES).validate(context).unwrap_err();

        assert!(
            format!("{error:#}")
                .starts_with("invalid same-as-paths glob on block README.md:(unnamed) at line 1"),
            "unexpected error: {error:#}"
        );
    }

    #[test]
    fn block_with_same_as_and_same_as_paths_is_compared_with_both_targets() -> anyhow::Result<()> {
        let context = validation_context(
            "README.md",
            r#"<!-- <block same-as=":b" same-as-paths="services/*/" same-as-pattern=".+"> -->
services/auth/
<!-- </block> -->
<!-- <block name="b"> -->
services/billing/
<!-- </block> -->"#,
        );

        let violations = validator(SERVICES).validate(context)?.violations;

        let messages: Vec<&str> = violations[&RepoPath::from_reference("README.md")?]
            .iter()
            .map(|violation| violation.message.as_str())
            .collect();
        assert_eq!(messages.len(), 2, "unexpected messages: {messages:?}");
        assert!(messages[0].contains("disagrees with README.md:b:"));
        assert!(messages[1].contains("disagrees with paths matching services/*/:"));
        Ok(())
    }

    #[test]
    fn blocks_with_and_without_same_as_records_a_check_for_the_examined_ones_only()
    -> anyhow::Result<()> {
        let context = validation_context(
            "example.py",
            r#"# <block name="source" same-as=":target">
alpha
# </block>
# <block name="target">
alpha
# </block>"#,
        );

        let report = validator(&[]).validate(context)?;

        // Only the block with the `same-as` attribute is checked. The target block is not.
        assert_eq!(checked_lines(&report), vec![1]);
        assert_eq!(violation_count(&report), 0);
        Ok(())
    }
}
