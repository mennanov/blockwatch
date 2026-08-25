use crate::blocks::{Block, BlockWithContext, FileBlocks, every_block, parse_file};
use crate::fs::FileSystem;
use crate::repo_path::RepoPath;
use crate::validators::{
    self, ValidationReport, ValidatorDetector, ValidatorSync, ValidatorType, Violation,
    ViolationRange, parse_number, value_match,
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
        let mut report = ValidationReport::default();
        // Caches files read from disk so each is parsed at most once. `validate` runs on a single
        // thread, so no synchronization is needed.
        let mut cache: HashMap<RepoPath, FileBlocks> = HashMap::new();
        for (file_path, file_blocks) in &context.blocks {
            for bwc in &file_blocks.blocks_with_context {
                let Some(same_as) = bwc.block.attributes.get("same-as") else {
                    continue;
                };
                let mode = parse_mode(&bwc.block)?;
                let format = parse_format(&bwc.block)?;
                let source_items = canonicalize(
                    extract_items(&bwc.block, &file_blocks.file_content)?,
                    &format,
                );
                let references =
                    validators::parse_block_references(same_as).with_context(|| {
                        format!(
                            "invalid same-as reference on block {}:{} at line {}",
                            file_path,
                            bwc.block.name_display(),
                            bwc.block.start_tag_position_range.start().line,
                        )
                    })?;
                let mut block_violations = Vec::new();
                for (target_file_opt, target_name) in references {
                    let target_file = target_file_opt.unwrap_or_else(|| file_path.clone());
                    let target_items = resolve_target_items(
                        &context,
                        self.file_system.as_ref(),
                        &mut cache,
                        &target_file,
                        &target_name,
                    )?;
                    let Some(target_items) = target_items else {
                        block_violations.push(create_violation(
                            file_path,
                            &bwc.block,
                            &target_file,
                            &target_name,
                            "target block not found",
                        )?);
                        continue;
                    };
                    let reason = match &source_items {
                        Err(reason) => Some(reason.clone()),
                        Ok(source) => match canonicalize(target_items, &format) {
                            Err(reason) => Some(reason),
                            Ok(target) => disagreement(source, &target, &mode),
                        },
                    };
                    if let Some(reason) = reason {
                        block_violations.push(create_violation(
                            file_path,
                            &bwc.block,
                            &target_file,
                            &target_name,
                            &reason,
                        )?);
                    }
                }
                report.add_all(file_path, &bwc.block, block_violations);
            }
        }
        Ok(report)
    }
}

/// Extracts a block's comparable items.
///
/// With a `same-as-pattern` attribute, each content line is matched against the regex and the
/// `value` named group (or the whole match, if there is no such group) becomes one item; lines that
/// do not match are skipped. Without a pattern, the comparable value is the block's normalized whole
/// content as a single item.
fn extract_items(block: &Block, file_content: &str) -> anyhow::Result<Vec<String>> {
    let content = block.content(file_content);
    let Some(pattern) = block.attributes.get("same-as-pattern") else {
        return Ok(vec![normalize_content(content)]);
    };
    let regex = Regex::new(pattern)
        .map_err(|e| anyhow!("same-as-pattern is not a valid regex ({pattern}): {e}"))?;
    Ok(content
        .lines()
        .filter_map(|line| {
            let captures = regex.captures(line.trim())?;
            let matched = value_match(&captures)?;
            Some(matched.as_str().to_string())
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

/// Resolves a target block's comparable items via a three-step lookup:
/// 1. blocks already parsed into the validation context (no I/O),
/// 2. the cache of files read earlier during this run,
/// 3. reading and parsing the file through `file_system`, caching the result.
///
/// The validation context may hold a diff-filtered subset of a file's blocks, so a target block
/// absent from step 1 is not necessarily missing — the lookup falls through to a full read of the
/// file. `Ok(None)` is returned only when the fully parsed file contains no block with that name. A
/// missing or unsupported file is an `Err`. Read confinement (rejecting `..`, absolute escapes, and
/// escaping symlinks) is a property of the filesystem implementation, so this function needs no path
/// guard of its own.
fn resolve_target_items<Fs: FileSystem>(
    context: &validators::ValidationContext,
    file_system: &Fs,
    cache: &mut HashMap<RepoPath, FileBlocks>,
    target_file: &RepoPath,
    target_name: &str,
) -> anyhow::Result<Option<Vec<String>>> {
    if let Some(file_blocks) = context.blocks.get(target_file)
        && let Some(items) = extract_named(file_blocks, target_name)?
    {
        return Ok(Some(items));
    }
    let file_blocks = match cache.entry(target_file.clone()) {
        Entry::Occupied(entry) => entry.into_mut(),
        Entry::Vacant(entry) => {
            let parsed = parse_file(
                file_system,
                target_file,
                &[],
                every_block,
                context.parsers(),
                context.extra_file_extensions(),
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
    target_name: &'a str,
    reason: &'a str,
}

fn create_violation(
    file_path: &RepoPath,
    block: &Block,
    target_file: &RepoPath,
    target_name: &str,
    reason: &str,
) -> anyhow::Result<Violation> {
    let line = block.start_tag_position_range.start().line;
    let message = format!(
        "Block {}:{} at line {} disagrees with {}:{}: {reason}",
        file_path.display(),
        block.name_display(),
        line,
        target_file.display(),
        target_name,
    );
    Ok(Violation::new(
        ViolationRange::new(
            block.start_tag_position_range.start().clone(),
            block.start_tag_position_range.end().clone(),
        ),
        "same-as".to_string(),
        message,
        block.severity()?,
        Some(serde_json::to_value(SameAsViolation {
            target_file,
            target_name,
            reason,
        })?),
    ))
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
    use crate::diff_parser::LineChange;
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
                ranges: None,
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
