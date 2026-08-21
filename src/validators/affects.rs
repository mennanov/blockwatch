use crate::blocks::{Block, BlockWithContext, FileBlocks, every_block, parse_file};
use crate::fs::FileSystem;
use crate::repo_path::RepoPath;
use crate::validators;
use crate::validators::{ValidationReport, ValidatorType, Violation, ViolationRange};
use anyhow::Context;
use serde::Serialize;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Enforces `affects="file:name"`: when a block's content changes, every block it declares it
/// affects must have changed in the same diff.
///
/// E.g., catches a constant edited without its documentation, or an enum extended without its
/// switch statement.
pub(crate) struct AffectsValidator<Fs: FileSystem> {
    /// Reads target files that the run's scope excluded but the diff still names.
    file_system: Arc<Fs>,
}

impl<Fs: FileSystem + 'static> AffectsValidator<Fs> {
    /// Creates the validator over the filesystem it will read excluded target files from.
    pub(super) fn new(file_system: Arc<Fs>) -> Self {
        Self { file_system }
    }
}

#[derive(Serialize)]
struct AffectsViolation<'a> {
    affected_block_file_path: &'a RepoPath,
    affected_block_name: &'a str,
}

impl<Fs: FileSystem + 'static> validators::ValidatorSync for AffectsValidator<Fs> {
    fn validate(
        &self,
        context: Arc<validators::ValidationContext>,
    ) -> anyhow::Result<ValidationReport> {
        let modified_names = modified_block_names(&context);
        // Caches target files read from disk so each is parsed at most once. `validate` runs on a
        // single thread, so no synchronization is needed.
        let mut cache: HashMap<RepoPath, FileBlocks> = HashMap::new();
        let mut report = ValidationReport::default();
        for (file_path, block_with_context) in content_modified_blocks(&context) {
            // A block without the attribute places no obligation on anything, and must not be
            // recorded as checked either.
            let Some(affects) = block_with_context.block.attributes.get("affects") else {
                continue;
            };
            let violations = self.unsatisfied_references(
                &context,
                &modified_names,
                &mut cache,
                file_path,
                block_with_context,
                affects,
            )?;
            report.add_all(file_path, &block_with_context.block, violations);
        }
        Ok(report)
    }
}

impl<Fs: FileSystem + 'static> AffectsValidator<Fs> {
    /// One violation for every block that `affects` names but the same diff left unchanged.
    ///
    /// `affects` is the attribute's raw value; the caller has already established that
    /// `block_with_context` carries it and that the diff modified the block's content.
    fn unsatisfied_references(
        &self,
        context: &validators::ValidationContext,
        modified_names: &HashSet<(RepoPath, String)>,
        cache: &mut HashMap<RepoPath, FileBlocks>,
        file_path: &RepoPath,
        block_with_context: &BlockWithContext,
        affects: &str,
    ) -> anyhow::Result<Vec<Violation>> {
        let references = validators::parse_block_references(affects).with_context(|| {
            format!(
                "invalid affects reference on block {}:{} at line {}",
                file_path,
                block_with_context.block.name_display(),
                block_with_context
                    .block
                    .start_tag_position_range
                    .start()
                    .line,
            )
        })?;
        let mut violations = Vec::new();
        for (target_file, target_name) in references {
            // A reference like ":foo" is resolved relative to the file the block is in.
            let target_file = target_file.unwrap_or_else(|| file_path.clone());
            let was_modified = modified_names.contains(&(target_file.clone(), target_name.clone()))
                || target_modified_outside_scope(
                    context,
                    self.file_system.as_ref(),
                    cache,
                    &target_file,
                    &target_name,
                )?;
            if !was_modified {
                violations.push(create_violation(
                    file_path,
                    &block_with_context.block,
                    &target_file,
                    target_name.as_str(),
                )?);
            }
        }
        Ok(violations)
    }
}

/// Every block in the run whose content the diff changed, paired with the file it was found in.
fn content_modified_blocks(
    context: &validators::ValidationContext,
) -> impl Iterator<Item = (&RepoPath, &BlockWithContext)> {
    context.blocks.iter().flat_map(|(file_path, file_blocks)| {
        file_blocks
            .blocks_with_context
            .iter()
            .filter(|block_with_context| block_with_context.is_content_modified)
            .map(move |block_with_context| (file_path, block_with_context))
    })
}

/// The `(file, name)` key of every named block the diff modified, which is what a reference has to
/// match to be satisfied. Unnamed blocks are skipped because they can't be referenced.
fn modified_block_names(context: &validators::ValidationContext) -> HashSet<(RepoPath, String)> {
    content_modified_blocks(context)
        .filter_map(|(file_path, block_with_context)| {
            block_with_context
                .block
                .name()
                .map(|name| (file_path.clone(), name.to_string()))
        })
        .collect()
}

/// Whether the target *outside* the validation context was modified.
///
/// This can happen when the globs constrain the validation context: the diff may mention a block in
/// a file that does not match the given globs.
fn target_modified_outside_scope<Fs: FileSystem>(
    context: &validators::ValidationContext,
    file_system: &Fs,
    cache: &mut HashMap<RepoPath, FileBlocks>,
    target_file: &RepoPath,
    target_name: &str,
) -> anyhow::Result<bool> {
    if context.blocks.contains_key(target_file) {
        // Target file is in the validation context, so it must already be in scope.
        return Ok(false);
    }
    let Some(line_changes) = context.line_changes_for(target_file) else {
        // The target file is not in the diff.
        return Ok(false);
    };
    if !file_system.exists(target_file.as_path()) {
        return Ok(false);
    }
    let file_blocks = match cache.entry(target_file.clone()) {
        Entry::Occupied(entry) => entry.into_mut(),
        Entry::Vacant(entry) => {
            let Some(parsed) = parse_file(
                file_system,
                target_file.as_path(),
                line_changes,
                every_block,
                context.parsers(),
                context.extra_file_extensions(),
            )?
            else {
                return Ok(false);
            };
            entry.insert(parsed)
        }
    };
    Ok(file_blocks
        .blocks_with_context
        .iter()
        .any(|block| block.is_content_modified && block.block.name() == Some(target_name)))
}

/// Selects [`AffectsValidator`] for blocks that carry an `affects` attribute *and* were modified —
/// an unchanged block places no obligation on anything.
pub(crate) struct AffectsValidatorDetector();

impl AffectsValidatorDetector {
    /// Creates the detector. Registered in [`crate::validators::detector_factories`].
    pub fn new() -> Self {
        Self {}
    }
}

impl<Fs: FileSystem + 'static> validators::ValidatorDetector<Fs> for AffectsValidatorDetector {
    fn detect(
        &self,
        block_with_context: &BlockWithContext,
        file_system: &Arc<Fs>,
    ) -> anyhow::Result<Option<ValidatorType>> {
        if block_with_context.is_content_modified
            && block_with_context.block.attributes.contains_key("affects")
        {
            Ok(Some(ValidatorType::Sync(Box::new(AffectsValidator::new(
                Arc::clone(file_system),
            )))))
        } else {
            Ok(None)
        }
    }
}

fn create_violation(
    modified_block_file_path: &RepoPath,
    modified_block: &Block,
    affected_block_file_path: &RepoPath,
    affected_block_name: &str,
) -> anyhow::Result<Violation> {
    let message = format!(
        "Block {}:{} at line {} is modified, but {}:{} is not",
        modified_block_file_path.display(),
        modified_block.name_display(),
        modified_block.start_tag_position_range.start().line,
        affected_block_file_path.display(),
        affected_block_name
    );
    let details = serde_json::to_value(AffectsViolation {
        affected_block_file_path,
        affected_block_name,
    })
    .context("failed to serialize AffectsViolation block")?;
    Ok(Violation::new(
        ViolationRange::new(
            modified_block.start_tag_position_range.start().clone(),
            modified_block.start_tag_position_range.end().clone(),
        ),
        "affects".to_string(),
        message,
        modified_block.severity()?,
        Some(details),
    ))
}

#[cfg(test)]
mod validate_tests {
    use super::*;
    use crate::diff_parser::LineChange;
    use crate::fs::test_utils::FakeFileSystem;
    use crate::repo_path::RepoPath;
    use crate::test_utils::{
        checked_lines, merge_validation_contexts, validation_context,
        validation_context_with_changes,
    };
    use crate::validators::ValidatorSync;

    /// Builds a validator with a fake filesystem seeded with `files` (path, contents).
    fn validator(files: &[(&str, &str)]) -> AffectsValidator<FakeFileSystem> {
        let map = files
            .iter()
            .map(|(p, c)| (p.to_string(), c.to_string()))
            .collect();
        AffectsValidator::new(Arc::new(FakeFileSystem::new(map)))
    }

    /// The two-file setup where a block in `source.py` references a block in `target.py`.
    fn two_file_system() -> FakeFileSystem {
        FakeFileSystem::new(HashMap::from([
            (
                "source.py".to_string(),
                "# <block name=\"s\" affects=\"target.py:t\">\nvalue = 2\n# </block>".to_string(),
            ),
            (
                "target.py".to_string(),
                "# <block name=\"t\">\nvalue = 2\n# </block>".to_string(),
            ),
        ]))
    }

    /// Parses `line_changes` into a context that only `source.py` is in scope for, the way a run
    /// given a glob matching just that file would.
    fn context_scoped_to_source(
        file_system: &FakeFileSystem,
        line_changes: HashMap<RepoPath, Vec<LineChange>>,
    ) -> anyhow::Result<Arc<validators::ValidationContext>> {
        let parsers = crate::language_parsers::language_parsers()?;
        let parsed = crate::blocks::parse_blocks(
            &line_changes,
            crate::blocks::ScanMode::OnlyChanged,
            file_system,
            &crate::fs::test_utils::FakePathChecker::allow_only("source.py"),
            &parsers,
            &HashMap::new(),
        )?;
        assert!(
            !parsed
                .blocks
                .contains_key(&RepoPath::from_reference("target.py")?),
            "the glob must keep the target out of the validated set"
        );
        Ok(Arc::new(validators::ValidationContext::new(
            parsed.blocks,
            parsers,
            line_changes,
            HashMap::new(),
        )))
    }

    #[test]
    fn modified_block_with_unmodified_targets_returns_violations() -> anyhow::Result<()> {
        let validator = validator(&[]);
        let context = merge_validation_contexts(vec![
            validation_context(
                "file1.py",
                r#"# <block affects="file2.py:foo">
print("first")
# </block>

# <block affects="file3.py:bar">
print("second")
# </block>
"#,
            ),
            validation_context_with_changes(
                "file2.py",
                r#"# <block name="foo">
print("file2")
# </block>
"#,
                vec![LineChange {
                    line: 1, // Only the start tag is changed, not the content.
                    ranges: Some(vec![3..8, 10..15]),
                }],
            ),
            validation_context_with_changes(
                "file3.py",
                r#"# <block name="not-bar">
print("file3")
# </block>
"#,
                vec![LineChange {
                    line: 3, // Only the end tag is modified, not the content.
                    ranges: None,
                }],
            ),
        ]);

        let violations = validator.validate(context)?.violations;

        assert_eq!(violations.len(), 1);
        let file1_violations = violations
            .get(&RepoPath::from_reference("file1.py").unwrap())
            .unwrap();
        assert_eq!(file1_violations.len(), 2);
        assert_eq!(
            file1_violations[0].message,
            "Block file1.py:(unnamed) at line 1 is modified, but file2.py:foo is not"
        );
        assert_eq!(
            file1_violations[1].message,
            "Block file1.py:(unnamed) at line 5 is modified, but file3.py:bar is not"
        );

        Ok(())
    }

    #[test]
    fn modified_block_with_modified_targets_returns_no_violations() -> anyhow::Result<()> {
        let validator = validator(&[]);
        let context = merge_validation_contexts(vec![
            validation_context(
                "file1.py",
                r#"# <block affects="file2.py:foo">
print("first")
# </block>

# <block affects="file3.py:bar">
print("second")
# </block>
"#,
            ),
            validation_context(
                "file2.py",
                r#"# <block name="foo">
print("foo")
# </block>
"#,
            ),
            validation_context(
                "file3.py",
                r#"# <block name="bar">
print("bar")
# </block>
"#,
            ),
        ]);

        let violations = validator.validate(context)?.violations;

        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn modified_block_with_one_unmodified_target_returns_a_violation() -> anyhow::Result<()> {
        let validator = validator(&[]);
        let context = merge_validation_contexts(vec![
            validation_context(
                "file1.py",
                r#"# <block name="foo" affects=":bar, file2.py:buzz">
print("foo")
# </block>

# <block name="bar" affects=":foo">
print("bar")
# </block>
"#,
            ),
            validation_context_with_changes(
                "file2.py",
                r#"# <block name="buzz" affects="file1.py:bar">
print("not-buzz")
# </block>
print("hello")
"#,
                vec![LineChange {
                    line: 4, // Line outside the block is changed.
                    ranges: None,
                }],
            ),
        ]);

        let violations = validator.validate(context)?.violations;

        assert_eq!(violations.len(), 1);
        let file1_violations = violations
            .get(&RepoPath::from_reference("file1.py").unwrap())
            .unwrap();
        assert_eq!(file1_violations.len(), 1);
        assert_eq!(
            file1_violations[0].message,
            "Block file1.py:foo at line 1 is modified, but file2.py:buzz is not"
        );
        Ok(())
    }

    #[test]
    fn modified_block_with_multiple_modified_targets_returns_no_violations() -> anyhow::Result<()> {
        let validator = validator(&[]);
        let context = merge_validation_contexts(vec![
            validation_context(
                "file1.py",
                r#"# <block name="foo" affects=":bar, file2.py:buzz">
print("foo")
# </block>

# <block name="bar" affects=":foo">
print("bar")
# </block>
"#,
            ),
            validation_context(
                "file2.py",
                r#"# <block name="buzz" affects="file1.py:bar">
print("buzz")
# </block>
"#,
            ),
        ]);

        let violations = validator.validate(context)?.violations;

        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn modified_block_with_an_unmodified_target_in_the_same_file_returns_a_violation()
    -> anyhow::Result<()> {
        let validator = validator(&[]);
        let context = validation_context_with_changes(
            "file1.py",
            r#"# <block affects=":foo">
print("first")
# </block>

# <block name="foo">
print("second")
# </block>
"#,
            vec![LineChange {
                line: 2,
                ranges: None,
            }],
        );

        let violations = validator.validate(context)?.violations;

        assert_eq!(violations.len(), 1);
        let file1_violations = violations
            .get(&RepoPath::from_reference("file1.py").unwrap())
            .unwrap();
        assert_eq!(file1_violations.len(), 1);
        assert_eq!(
            file1_violations[0].message,
            "Block file1.py:(unnamed) at line 1 is modified, but file1.py:foo is not"
        );

        Ok(())
    }

    #[test]
    fn block_with_unmodified_content_returns_no_violations() -> anyhow::Result<()> {
        let validator = validator(&[]);
        let context = merge_validation_contexts(vec![
            validation_context_with_changes(
                "file1.py",
                r#"# <block affects="file2.py:foo">
pass
# </block>

# <block affects="file3.py:bar">
pass
# </block>
"#,
                vec![
                    LineChange {
                        line: 1,
                        ranges: Some(vec![0..10, 12..15]),
                    }, // First block start tag
                    LineChange {
                        line: 7,
                        ranges: None,
                    }, // Second block end tag
                ],
            ),
            validation_context_with_changes(
                "file2.py",
                r#"# <block name="foo">
pass
# </block>
"#,
                vec![LineChange {
                    line: 1,
                    ranges: Some(vec![0..4, 6..10]),
                }], // Only start tag modified
            ),
            validation_context_with_changes(
                "file3.py",
                r#"# <block name="bar">
pass
# </block>
"#,
                vec![LineChange {
                    line: 3,
                    ranges: None,
                }], // Only end tag modified
            ),
        ]);

        let violations = validator.validate(context)?.violations;

        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn unmodified_target_outside_the_globs_returns_a_violation() -> anyhow::Result<()> {
        // The counterpart of the test above: resolving a target the run did not read must report
        // the ones the diff never touched, rather than assuming anything out of scope is fine.
        let file_system = two_file_system();
        let line_changes = HashMap::from([(
            RepoPath::from_reference("source.py")?,
            vec![LineChange {
                line: 2,
                ranges: None,
            }],
        )]);
        let context = context_scoped_to_source(&file_system, line_changes)?;

        let violations = AffectsValidator::new(Arc::new(file_system))
            .validate(context)?
            .violations;

        assert_eq!(violations.len(), 1);
        Ok(())
    }

    #[test]
    fn modified_target_outside_the_globs_returns_no_violations() -> anyhow::Result<()> {
        let file_system = two_file_system();
        let line_changes = HashMap::from([
            (
                RepoPath::from_reference("source.py")?,
                vec![LineChange {
                    line: 2,
                    ranges: None,
                }],
            ),
            (
                RepoPath::from_reference("target.py")?,
                vec![LineChange {
                    line: 2,
                    ranges: None,
                }],
            ),
        ]);
        let context = context_scoped_to_source(&file_system, line_changes)?;

        let violations = AffectsValidator::new(Arc::new(file_system))
            .validate(context)?
            .violations;

        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn blocks_with_cyclic_references_partly_modified_returns_violations() -> anyhow::Result<()> {
        let validator = validator(&[]);
        let contents = r#"# <block name="foo" affects=":bar">
print("foo")
# </block>

# <block name="bar" affects=":foo">
pass
# </block>
"#;
        let line_changes = vec![
            LineChange {
                line: 2,
                ranges: None,
            }, // First block's content line
            LineChange {
                line: 4, // Not in any of the blocks.
                ranges: None,
            },
        ];
        let context = validation_context_with_changes("file1.py", contents, line_changes);

        let violations = validator.validate(context)?.violations;

        assert!(!violations.is_empty());
        Ok(())
    }

    #[test]
    fn blocks_with_cyclic_references_all_modified_returns_no_violations() -> anyhow::Result<()> {
        let validator = validator(&[]);
        let context = validation_context(
            "file1.py",
            r#"# <block name="foo" affects=":bar">
print("foo")
# </block>

# <block name="bar" affects=":foo">
print("bar")
# </block>
"#,
        );

        let violations = validator.validate(context)?.violations;

        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn reference_with_a_leading_current_directory_returns_no_violations() -> anyhow::Result<()> {
        // `./target.py` and `target.py` name the same file, so a change to both blocks satisfies
        // the reference regardless of which spelling the author used.
        let context = merge_validation_contexts(vec![
            validation_context(
                "source.py",
                "# <block name=\"s\" affects=\"./target.py:t\">\nvalue = 2\n# </block>",
            ),
            validation_context("target.py", "# <block name=\"t\">\nvalue = 2\n# </block>"),
        ]);
        assert!(validator(&[]).validate(context)?.violations.is_empty());
        Ok(())
    }

    #[test]
    fn blocks_without_an_affects_attribute_returns_no_violations() -> anyhow::Result<()> {
        let validator = validator(&[]);
        let context = validation_context(
            "file1.py",
            r#"# <block name="foo">
pass
# </block>
"#,
        );

        let violations = validator.validate(context)?.violations;

        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn modified_block_with_affects_records_one_check() -> anyhow::Result<()> {
        let context = validation_context(
            "example.py",
            r#"# <block name="source" affects=":target">
a = 1
# </block>
# <block name="target">
b = 2
# </block>"#,
        );

        let report = validator(&[]).validate(context)?;

        // Only the block with the `affects` attribute is checked. The target block is not.
        assert_eq!(checked_lines(&report), vec![1]);
        Ok(())
    }
}
