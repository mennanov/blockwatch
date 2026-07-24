use crate::blocks::{Block, BlockWithContext, FileBlocks, parse_single_file};
use crate::fs::FileSystem;
use crate::validators::{
    self, ValidatorDetector, ValidatorSync, ValidatorType, Violation, ViolationRange,
};
use anyhow::anyhow;
use regex::Regex;
use serde::Serialize;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(crate) struct SameAsValidator<Fs: FileSystem> {
    // Reads files containing referenced target blocks that are not already parsed into the
    // validation context.
    file_system: Arc<Fs>,
}

impl<Fs: FileSystem + 'static> SameAsValidator<Fs> {
    pub(super) fn new(file_system: Arc<Fs>) -> Self {
        Self { file_system }
    }
}

impl<Fs: FileSystem + 'static> ValidatorSync for SameAsValidator<Fs> {
    fn validate(
        &self,
        context: Arc<validators::ValidationContext>,
    ) -> anyhow::Result<HashMap<PathBuf, Vec<Violation>>> {
        let mut violations: HashMap<PathBuf, Vec<Violation>> = HashMap::new();
        // Caches files read from disk so each is parsed at most once. `validate` runs on a single
        // thread, so no synchronization is needed.
        let mut cache: HashMap<PathBuf, FileBlocks> = HashMap::new();
        for (file_path, file_blocks) in &context.blocks {
            for bwc in &file_blocks.blocks_with_context {
                let Some(same_as) = bwc.block.attributes.get("same-as") else {
                    continue;
                };
                let source_items = extract_items(&bwc.block, &file_blocks.file_content)?;
                for (target_file_opt, target_name) in validators::parse_block_references(same_as)? {
                    let target_file = target_file_opt.unwrap_or_else(|| file_path.clone());
                    let target_items = resolve_target_items(
                        &context,
                        self.file_system.as_ref(),
                        &mut cache,
                        &target_file,
                        &target_name,
                    )?;
                    let Some(target_items) = target_items else {
                        violations
                            .entry(file_path.clone())
                            .or_default()
                            .push(create_violation(
                                file_path,
                                &bwc.block,
                                &target_file,
                                &target_name,
                                "target block not found",
                            )?);
                        continue;
                    };
                    if let Some(reason) = disagreement(&source_items, &target_items) {
                        violations
                            .entry(file_path.clone())
                            .or_default()
                            .push(create_violation(
                                file_path,
                                &bwc.block,
                                &target_file,
                                &target_name,
                                &reason,
                            )?);
                    }
                }
            }
        }
        Ok(violations)
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
    let mut items = Vec::new();
    for line in content.lines() {
        if let Some(captures) = regex.captures(line.trim())
            && let Some(matched) = captures.name("value").or_else(|| captures.get(0))
        {
            items.push(matched.as_str().to_string());
        }
    }
    Ok(items)
}

/// Trim each line, drop blanks, rejoin — matching `line-pattern` / `keep-unique` normalization.
fn normalize_content(content: &str) -> String {
    content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
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
    cache: &mut HashMap<PathBuf, FileBlocks>,
    target_file: &Path,
    target_name: &str,
) -> anyhow::Result<Option<Vec<String>>> {
    if let Some(file_blocks) = context.blocks.get(target_file)
        && let Some(items) = extract_named(file_blocks, target_name)?
    {
        return Ok(Some(items));
    }
    let file_blocks = match cache.entry(target_file.to_path_buf()) {
        Entry::Occupied(entry) => entry.into_mut(),
        Entry::Vacant(entry) => {
            // Referenced target files are resolved without applying extension overrides.
            let parsed =
                parse_single_file(file_system, target_file, context.parsers(), &HashMap::new())?
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

/// Returns `None` when the two sides agree, or `Some(reason)` describing the mismatch.
///
/// The comparison is order- and duplicate-insensitive: the two item lists must form equal sets.
fn disagreement(source: &[String], target: &[String]) -> Option<String> {
    let source_set: HashSet<&String> = source.iter().collect();
    let target_set: HashSet<&String> = target.iter().collect();
    (source_set != target_set).then(|| format!("{source:?} != {target:?}"))
}

#[derive(Serialize)]
struct SameAsViolation<'a> {
    target_file: &'a Path,
    target_name: &'a str,
    reason: &'a str,
}

fn create_violation(
    file_path: &Path,
    block: &Block,
    target_file: &Path,
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

pub(crate) struct SameAsValidatorDetector();

impl SameAsValidatorDetector {
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
    use crate::test_utils::merge_validation_contexts;
    use crate::test_utils::validation_context;
    use crate::test_utils::validation_context_with_changes;

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
    fn block_with_same_as_attribute_runs_without_violations() -> anyhow::Result<()> {
        let context = validation_context(
            "config.py",
            "# <block same-as=\":b\">\nvalue = 10\n# </block>\n# <block name=\"b\">\nvalue = 10\n# </block>",
        );
        assert!(validator(&[]).validate(context)?.is_empty());
        Ok(())
    }

    #[test]
    fn same_file_equal_content_passes() -> anyhow::Result<()> {
        let context = validation_context(
            "config.py",
            "# <block same-as=\":b\">\nvalue = 10\n# </block>\n# <block name=\"b\">\nvalue = 10\n# </block>",
        );
        assert!(validator(&[]).validate(context)?.is_empty());
        Ok(())
    }

    #[test]
    fn same_file_differing_content_fails() -> anyhow::Result<()> {
        let context = validation_context(
            "config.py",
            "# <block same-as=\":b\">\nvalue = 10\n# </block>\n# <block name=\"b\">\nvalue = 20\n# </block>",
        );
        let violations = validator(&[]).validate(context)?;
        let file = violations.get(&PathBuf::from("config.py")).unwrap();
        assert_eq!(file.len(), 1);
        assert_eq!(file[0].code, "same-as");
        Ok(())
    }

    #[test]
    fn cross_file_both_in_scope_compares() -> anyhow::Result<()> {
        let context = merge_validation_contexts(vec![
            validation_context("a.rs", "// <block same-as=\"b.md:doc\">\nX\n// </block>"),
            validation_context(
                "b.md",
                "[//]: # (<block name=\"doc\">)\n\nX\n\n[//]: # (</block>)",
            ),
        ]);
        assert!(validator(&[]).validate(context)?.is_empty());
        Ok(())
    }

    #[test]
    fn missing_target_block_reports_violation() -> anyhow::Result<()> {
        let source = "# <block same-as=\":nope\">\nvalue = 10\n# </block>";
        let context = validation_context("config.py", source);
        // The file exists but has no block named "nope"; confirming its absence requires reading the
        // full file, so the filesystem is seeded with it.
        let violations = validator(&[("config.py", source)]).validate(context)?;
        assert_eq!(
            violations.get(&PathBuf::from("config.py")).unwrap().len(),
            1
        );
        Ok(())
    }

    #[test]
    fn resolves_out_of_scope_target_from_injected_fs() -> anyhow::Result<()> {
        // Only the source is in scope; the target file is provided through the fake filesystem.
        let context = validation_context("a.rs", "// <block same-as=\"b.md:doc\">\nX\n// </block>");
        let v = validator(&[(
            "b.md",
            "[//]: # (<block name=\"doc\">)\n\nX\n\n[//]: # (</block>)",
        )]);
        assert!(v.validate(context)?.is_empty());
        Ok(())
    }

    #[test]
    fn in_scope_file_with_unmodified_sibling_target_resolves_from_disk() -> anyhow::Result<()> {
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
        assert!(v.validate(context)?.is_empty());
        Ok(())
    }

    #[test]
    fn pattern_set_equality_ignores_order() -> anyhow::Result<()> {
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
        assert!(validator(&[]).validate(context)?.is_empty());
        Ok(())
    }

    #[test]
    fn pattern_set_mismatch_fails() -> anyhow::Result<()> {
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
        assert_eq!(validator(&[]).validate(context)?.len(), 1);
        Ok(())
    }
}
