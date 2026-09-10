use crate::blocks::{Block, BlockWithContext, FileBlocks, every_block, parse_file};
use crate::fs::FileSystem;
use crate::repo_path::RepoPath;
use crate::validators::{
    self, BlockReference, ValidationReport, ValidatorDetector, ValidatorSync, ValidatorType,
    Violation, ViolationRange, parse_number, value_match,
};
use anyhow::{Context, anyhow};
use regex::Regex;
use serde::Serialize;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Enforces `same-as="file:name"`: the block's content must equal that of the blocks it names.
///
/// Where `affects` only checks that linked blocks were edited together, this compares their actual
/// contents, catching a constant or version duplicated in two places that have drifted apart. That
/// is also why it runs on a full-tree scan and not only on the blocks a diff touched.
///
/// A target spelled without a `:` names a whole file, whose entire text is then the thing compared
/// against — so a file with no comments to declare a block in can still be a target.
pub(crate) struct SameAsValidator<Fs: FileSystem> {
    /// Reads files containing referenced target blocks that are not already parsed into the
    /// validation context.
    file_system: Arc<Fs>,
}

impl<Fs: FileSystem + 'static> SameAsValidator<Fs> {
    /// Creates the validator over the filesystem it will read referenced files from.
    pub(super) fn new(file_system: Arc<Fs>) -> Self {
        Self { file_system }
    }
}

impl<Fs: FileSystem + 'static> ValidatorSync for SameAsValidator<Fs> {
    fn validate(
        &self,
        context: Arc<validators::ValidationContext>,
    ) -> anyhow::Result<ValidationReport> {
        let mut targets = TargetItems::new(&context, self.file_system.as_ref());
        let mut report = ValidationReport::default();
        for (file_path, file_blocks) in &context.blocks {
            for block_with_context in &file_blocks.blocks_with_context {
                let Some(same_as) = block_with_context.block.attributes.get("same-as") else {
                    continue;
                };
                let violations = block_violations(
                    &mut targets,
                    file_path,
                    &file_blocks.file_content,
                    &block_with_context.block,
                    same_as,
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
    format: Format,
    /// `Err` carries the reason the block's own items could not be canonicalized. It is reported
    /// against every reference the block makes rather than aborting the run, since content that does
    /// not meet the asserted shape is a violation like any other.
    source_items: Result<Vec<String>, String>,
    /// The block's `same-as-pattern`, which governs a whole-file target too: such a target has no
    /// block of its own to carry one.
    pattern: Option<String>,
}

impl Comparison {
    fn new(block: &Block, file_content: &str) -> anyhow::Result<Self> {
        let format = parse_format(block)?;
        Ok(Self {
            mode: parse_mode(block)?,
            source_items: canonicalize(extract_items(block, file_content)?, &format),
            pattern: block.attributes.get("same-as-pattern").cloned(),
            format,
        })
    }

    /// Why the target disagrees with the block, or `None` when the two agree.
    fn disagreement_with(&self, target_items: Vec<String>) -> Option<String> {
        match &self.source_items {
            Err(reason) => Some(reason.clone()),
            Ok(source) => match canonicalize(target_items, &self.format) {
                Err(reason) => Some(reason),
                Ok(target) => disagreement(source, &target, &self.mode),
            },
        }
    }
}

/// Every violation one block's `same-as` produces: at most one per reference it names.
///
/// `same_as` is the attribute's raw value; the caller has already established that `block` carries
/// it.
fn block_violations<Fs: FileSystem>(
    targets: &mut TargetItems<'_, Fs>,
    file_path: &RepoPath,
    file_content: &str,
    block: &Block,
    same_as: &str,
) -> anyhow::Result<Vec<Violation>> {
    let comparison = Comparison::new(block, file_content)?;
    let mut violations = Vec::new();
    for reference in parse_references(file_path, block, same_as)? {
        let violation = reference_violation(targets, &comparison, file_path, block, reference)?;
        violations.extend(violation);
    }
    Ok(violations)
}

/// Parses the attribute's raw value, naming the block that wrote it so an author can find the
/// offending reference without searching for it.
fn parse_references(
    file_path: &RepoPath,
    block: &Block,
    same_as: &str,
) -> anyhow::Result<Vec<BlockReference>> {
    validators::parse_block_references(same_as).with_context(|| {
        format!(
            "invalid same-as reference on block {}:{} at line {}",
            file_path,
            block.name_display(),
            block.start_tag_position_range.start().line,
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
    reference: BlockReference,
) -> anyhow::Result<Option<Violation>> {
    let (target_file, target_name, target_items) = match reference {
        BlockReference::Block { file, name } => {
            // A reference like ":foo" is resolved relative to the file the block is in.
            let target_file = file.unwrap_or_else(|| file_path.clone());
            let items = targets.named_block_items(&target_file, &name)?;
            (target_file, Some(name), items)
        }
        BlockReference::File(target_file) => {
            let items = targets.whole_file_items(&target_file, comparison.pattern.as_ref())?;
            (target_file, None, Some(items))
        }
    };
    let target_name = target_name.as_deref();
    let Some(target_items) = target_items else {
        return Ok(Some(create_violation(
            file_path,
            block,
            &target_file,
            target_name,
            "target block not found",
        )?));
    };
    comparison
        .disagreement_with(target_items)
        .map(|reason| create_violation(file_path, block, &target_file, target_name, &reason))
        .transpose()
}

/// Extracts a block's comparable items.
///
/// With a `same-as-pattern` attribute, every match on every content line contributes its `value`
/// named group (or the whole match, if there is no such group) as one item; lines that do not match
/// are skipped. A pattern selects which parts of a block take part in the comparison.
/// Matches whose value is empty carry nothing to compare and are skipped.
///
/// The regex runs against each trimmed line rather than the whole block, so `^` and `$` anchor to
/// the entry rather than to its indentation, and the items stay in the order the two sides list
/// them.
///
/// Without a pattern, the comparable value is the block's normalized whole content as a single item.
fn extract_items(block: &Block, file_content: &str) -> anyhow::Result<Vec<String>> {
    extract_items_from(
        block.content(file_content),
        block.attributes.get("same-as-pattern"),
    )
}

/// Extracts items from `content` for the given `pattern`.
///
/// If `pattern` is `None` then the whole `content` is returned.
fn extract_items_from(content: &str, pattern: Option<&String>) -> anyhow::Result<Vec<String>> {
    let Some(pattern) = pattern else {
        return Ok(vec![normalize_content(content)]);
    };
    let regex = Regex::new(pattern)
        .map_err(|e| anyhow!("same-as-pattern is not a valid regex ({pattern}): {e}"))?;
    Ok(content
        .lines()
        .flat_map(|line| {
            regex
                .captures_iter(line.trim())
                .filter_map(|captures| value_match(&captures).map(|matched| matched.as_str()))
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
        .collect())
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
fn extract_named(file_blocks: &FileBlocks, name: &str) -> anyhow::Result<Option<Vec<String>>> {
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
    /// Whole-file targets, which are read but never parsed.
    contents: HashMap<RepoPath, String>,
}

impl<'a, Fs: FileSystem> TargetItems<'a, Fs> {
    fn new(context: &'a validators::ValidationContext, file_system: &'a Fs) -> Self {
        Self {
            context,
            file_system,
            parsed: HashMap::new(),
            contents: HashMap::new(),
        }
    }

    /// Resolves a named target block's comparable items via a three-step lookup:
    /// 1. blocks already parsed into the validation context (no I/O),
    /// 2. the files read earlier during this run,
    /// 3. reading and parsing the file, keeping the result.
    fn named_block_items(
        &mut self,
        target_file: &RepoPath,
        target_name: &str,
    ) -> anyhow::Result<Option<Vec<String>>> {
        if let Some(file_blocks) = self.context.blocks.get(target_file)
            && let Some(items) = extract_named(file_blocks, target_name)?
        {
            return Ok(Some(items));
        }
        let file_blocks = match self.parsed.entry(target_file.clone()) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let parsed = parse_file(
                    self.file_system,
                    target_file,
                    &[],
                    every_block,
                    self.context.parsers(),
                    self.context.extra_file_extensions(),
                )?
                .ok_or_else(|| {
                    anyhow!(
                        "same-as target file format is unsupported: {}",
                        target_file.display()
                    )
                })?;
                entry.insert(parsed)
            }
        };
        extract_named(file_blocks, target_name)
    }

    /// Resolves a whole-file target's comparable items from the file's entire text.
    ///
    /// No grammar is involved, which is what lets a `same-as` target be a file BlockWatch cannot
    /// parse. A file already in the validation context is read from there; otherwise it is read
    /// through the filesystem and kept. A missing or unreadable file is an `Err`, as it is for a
    /// named target.
    fn whole_file_items(
        &mut self,
        target_file: &RepoPath,
        pattern: Option<&String>,
    ) -> anyhow::Result<Vec<String>> {
        if let Some(file_blocks) = self.context.blocks.get(target_file) {
            return extract_items_from(&file_blocks.file_content, pattern);
        }
        let content = match self.contents.entry(target_file.clone()) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => entry.insert(
                self.file_system
                    .read_to_string(target_file.as_path())
                    .with_context(|| {
                        format!(
                            "failed to read same-as target file: {}",
                            target_file.display()
                        )
                    })?,
            ),
        };
        extract_items_from(content, pattern)
    }
}

/// How extracted items are canonicalized before comparison.
enum Format {
    /// Compare items as extracted (the default).
    Verbatim,
    /// Parse each item as a number so different spellings (`"10"`, `"10.0"`) compare equal.
    Numeric,
}

/// Reads a block's `same-as-format` attribute, defaulting to [`Format::Verbatim`]. An unrecognized
/// value is an error, mirroring [`parse_mode`].
fn parse_format(block: &Block) -> anyhow::Result<Format> {
    match block.attributes.get("same-as-format").map(String::as_str) {
        None => Ok(Format::Verbatim),
        Some("numeric") => Ok(Format::Numeric),
        Some(other) => Err(anyhow!(
            "invalid same-as-format \"{other}\" (expected numeric)"
        )),
    }
}

/// Canonicalizes extracted items ahead of comparison.
///
/// Under [`Format::Numeric`] each item is parsed as a number (see [`parse_number`]) and re-formatted
/// so that different spellings of the same value (`"10"`, `"10.0"`, `"1_0"`) compare equal; an item
/// that is not a number yields `Err` with a human-readable reason, which the caller surfaces as a
/// violation rather than aborting the run. Under [`Format::Verbatim`] the items are returned
/// unchanged.
///
/// Normalizing drops the trailing zeros and the exponent notation that would otherwise leave two
/// spellings of one value differing as strings.
fn canonicalize(items: Vec<String>, format: &Format) -> Result<Vec<String>, String> {
    match format {
        Format::Verbatim => Ok(items),
        Format::Numeric => items
            .into_iter()
            .map(|item| {
                parse_number(&item)
                    .map(|number| number.normalized().to_string())
                    .ok_or_else(|| format!("same-as-format=numeric but \"{item}\" is not a number"))
            })
            .collect(),
    }
}

/// How two item lists are compared.
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
/// `Mode::Single` requires exactly one value per side; a side with a different count is itself a
/// mismatch (the block's content does not meet the asserted shape), reported like any other.
fn disagreement(source: &[String], target: &[String], mode: &Mode) -> Option<String> {
    if source.is_empty() && target.is_empty() {
        return Some("same-as-pattern matched no values in this block or its target".to_string());
    }
    match mode {
        Mode::Single => {
            if source.len() != 1 || target.len() != 1 {
                return Some(format!(
                    "same-as-mode=single requires exactly one value per side (got {} and {})",
                    source.len(),
                    target.len()
                ));
            }
            (source[0] != target[0]).then(|| format!("{} != {}", source[0], target[0]))
        }
        Mode::Sequence => (source != target).then(|| format!("{source:?} != {target:?}")),
        Mode::Set => {
            let source_set: HashSet<&String> = source.iter().collect();
            let target_set: HashSet<&String> = target.iter().collect();
            (source_set != target_set).then(|| format!("{source:?} != {target:?}"))
        }
        Mode::Subset => {
            let target_set: HashSet<&String> = target.iter().collect();
            let missing: Vec<&String> = source
                .iter()
                .filter(|item| !target_set.contains(item))
                .collect();
            (!missing.is_empty()).then(|| format!("not a subset; missing from target: {missing:?}"))
        }
    }
}

#[derive(Serialize)]
struct SameAsViolation<'a> {
    target_file: &'a RepoPath,
    /// Absent for a whole-file reference, which has no block name.
    #[serde(skip_serializing_if = "Option::is_none")]
    target_name: Option<&'a str>,
    reason: &'a str,
}

fn create_violation(
    file_path: &RepoPath,
    block: &Block,
    target_file: &RepoPath,
    target_name: Option<&str>,
    reason: &str,
) -> anyhow::Result<Violation> {
    let line = block.start_tag_position_range.start().line;
    let target = match target_name {
        Some(target_name) => format!("{}:{}", target_file.display(), target_name),
        None => format!("file {}", target_file.display()),
    };
    let message = format!(
        "Block {}:{} at line {} disagrees with {target}: {reason}",
        file_path.display(),
        block.name_display(),
        line,
    );
    Violation::new(
        ViolationRange::new(
            block.start_tag_position_range.start().clone(),
            block.start_tag_position_range.end().clone(),
        ),
        file_path,
        block,
        "same-as".to_string(),
        message,
        // A block may be compared against several targets, so the target identifies this violation.
        Some(&target),
        Some(serde_json::to_value(SameAsViolation {
            target_file,
            target_name,
            reason,
        })?),
    )
}

/// Selects [`SameAsValidator`] for blocks carrying a `same-as` attribute.
pub(crate) struct SameAsValidatorDetector();

impl SameAsValidatorDetector {
    /// Creates the detector. Registered in [`validators::detector_factories`].
    pub fn new() -> Self {
        Self()
    }
}

impl<Fs: FileSystem + 'static> ValidatorDetector<Fs> for SameAsValidatorDetector {
    fn detect(
        &self,
        block_with_context: &BlockWithContext,
        file_system: &Arc<Fs>,
    ) -> anyhow::Result<Option<ValidatorType>> {
        if block_with_context.block.attributes.contains_key("same-as") {
            Ok(Some(ValidatorType::Sync(Box::new(SameAsValidator::new(
                Arc::clone(file_system),
            )))))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod validate_tests {
    use super::*;
    use crate::diff_parser::{LineChange, LineChangeKind};
    use crate::fs::test_utils::FakeFileSystem;
    use crate::repo_path::RepoPath;
    use crate::test_utils::validation_context;
    use crate::test_utils::validation_context_with_changes;
    use crate::test_utils::{checked_lines, merge_validation_contexts, violation_count};

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
        assert_eq!(file[0].code, "same-as");
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
        assert_eq!(
            violations
                .get(&RepoPath::from_reference("config.py")?)
                .unwrap()
                .len(),
            1
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
        assert_eq!(file[0].code, "same-as");
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
        assert_eq!(file[0].code, "same-as");
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
        assert_eq!(file[0].code, "same-as");
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
        assert_eq!(file[0].code, "same-as");
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
        assert_eq!(file[0].code, "same-as");
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
    fn unknown_same_as_format_value_returns_an_error() -> anyhow::Result<()> {
        // Unlike bad content, an unrecognized `same-as-format` value is an authoring error and
        // aborts the run, mirroring how an unknown `same-as-mode` is handled.
        let context = validation_context(
            "a.rs",
            "// <block same-as=\":b\" same-as-format=\"number\">\n1\n// </block>\n// <block name=\"b\">\n1\n// </block>",
        );
        assert!(validator(&[]).validate(context).is_err());
        Ok(())
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
            "a whole-file target names no block"
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
