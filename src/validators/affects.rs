use crate::blocks::{Block, BlockWithContext, FileBlocks, every_block, parse_file};
use crate::fs::FileSystem;
use crate::repo_path::RepoPath;
use crate::validators;
use crate::validators::{ValidationReport, ValidatorType, Violation, ViolationRange};
use anyhow::{Context, anyhow};
use serde::Serialize;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::Arc;

/// Enforces `affects="file:name"`: when a block's content changes, every block it declares it
/// affects must have changed in the same diff. It also checks reference integrity.
///
/// E.g., catches a constant edited without its documentation, or an enum extended without its
/// switch statement.
pub(crate) struct AffectsValidator<Fs: FileSystem> {
    /// Reads target files that are not already parsed into the validation context.
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
        // Named blocks per file along with their modification bit, indexed by file path.
        let in_scope_index: HashMap<&RepoPath, HashMap<String, bool>> = context
            .blocks
            .iter()
            .map(|(file_path, file_blocks)| (file_path, name_index(file_blocks)))
            .collect();
        // Named blocks per file read from the filesystem, indexed by file path.
        let mut cache: HashMap<RepoPath, HashMap<String, bool>> = HashMap::new();
        let mut report = ValidationReport::default();
        // Every block carrying `affects` is examined, not only the ones the diff touched: reference
        // integrity has to catch a dangling target even when the referencing block is unchanged.
        for (file_path, file_blocks) in &context.blocks {
            for block_with_context in &file_blocks.blocks_with_context {
                let Some(affects) = block_with_context.block.attributes.get("affects") else {
                    continue;
                };
                let violations = self.reference_violations(
                    &context,
                    &in_scope_index,
                    &mut cache,
                    file_path,
                    block_with_context,
                    affects,
                )?;
                report.add_all(file_path, &block_with_context.block, violations);
            }
        }
        Ok(report)
    }
}

impl<Fs: FileSystem + 'static> AffectsValidator<Fs> {
    /// One violation per block reference.
    ///
    /// 2 violation kinds possible:
    /// 1. Invalid block reference
    /// 2. The referenced block is not modified while `block_with_context` is.
    ///
    /// `affects` is the attribute's raw value; the caller has already established that
    /// `block_with_context` carries it.
    fn reference_violations(
        &self,
        context: &validators::ValidationContext,
        in_scope_index: &HashMap<&RepoPath, HashMap<String, bool>>,
        cache: &mut HashMap<RepoPath, HashMap<String, bool>>,
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
            match resolve_target(
                context,
                self.file_system.as_ref(),
                in_scope_index,
                cache,
                &target_file,
                &target_name,
            )? {
                None => violations.push(dangling_reference_violation(
                    file_path,
                    &block_with_context.block,
                    &target_file,
                    &target_name,
                )?),
                // The "affects" only applies when this block's content changed: an
                // unchanged block places no obligation on the blocks it references.
                Some(target_modified) => {
                    if block_with_context.is_content_modified && !target_modified {
                        violations.push(create_violation(
                            file_path,
                            &block_with_context.block,
                            &target_file,
                            target_name.as_str(),
                        )?);
                    }
                }
            }
        }
        Ok(violations)
    }
}

/// Resolves a referenced target block, mirroring `same-as`'s three-step lookup:
/// 1. blocks already parsed into the validation context (no I/O), via `in_scope_index`,
/// 2. the index of files read earlier during this run, via `cache`,
/// 3. reading and parsing the file through `file_system`, indexing and caching the result.
///
/// Returns `Some(was_modified)` when the block exists, where `was_modified` reports whether the diff
/// changed its content; `None` when the file parses but holds no block with that name — a dangling
/// reference. A missing or unsupported target file is an `Err`, aborting the run, the same
/// abort-on-missing-file behavior `same-as` has.
///
/// A target file present in scope but missing the named block falls through to a disk read: under
/// `--only-changed` the in-scope blocks are just the ones the diff touched, so the named block may
/// live in the same file yet outside that subset.
fn resolve_target<Fs: FileSystem>(
    context: &validators::ValidationContext,
    file_system: &Fs,
    in_scope_index: &HashMap<&RepoPath, HashMap<String, bool>>,
    cache: &mut HashMap<RepoPath, HashMap<String, bool>>,
    target_file: &RepoPath,
    target_name: &str,
) -> anyhow::Result<Option<bool>> {
    if let Some(index) = in_scope_index.get(target_file)
        && let Some(&modified) = index.get(target_name)
    {
        return Ok(Some(modified));
    }
    // Parsing with the target's own line changes recovers both its blocks and their modified state;
    // a file the diff never referenced simply has no line changes, so nothing in it counts as modified.
    let line_changes = context.line_changes_for(target_file).unwrap_or(&[]);
    let index = match cache.entry(target_file.clone()) {
        Entry::Occupied(entry) => entry.into_mut(),
        Entry::Vacant(entry) => {
            let parsed = parse_file(
                file_system,
                target_file.as_path(),
                line_changes,
                every_block,
                context.parsers(),
                context.extra_file_extensions(),
            )?
            .ok_or_else(|| {
                anyhow!(
                    "affects target file format is unsupported: {}",
                    target_file.display()
                )
            })?;
            entry.insert(name_index(&parsed))
        }
    };
    Ok(index.get(target_name).copied())
}

/// Builds a map of the named blocks in `file_blocks` together with their modification bit.
fn name_index(file_blocks: &FileBlocks) -> HashMap<String, bool> {
    let mut index = HashMap::new();
    for block in &file_blocks.blocks_with_context {
        if let Some(name) = block.block.name() {
            index
                .entry(name.to_string())
                .or_insert(block.is_content_modified);
        }
    }
    index
}

/// Selects [`AffectsValidator`] for every block that carries an `affects` attribute.
///
/// It does not gate on whether the block was modified: the validator also checks that each
/// referenced target still resolves, and a dangling reference has to be reported even when the
/// referencing block itself is untouched.
pub(crate) struct AffectsValidatorDetector();

impl AffectsValidatorDetector {
    /// Creates the detector. Registered in [`validators::detector_factories`].
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
        if block_with_context.block.attributes.contains_key("affects") {
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
    affects_violation(
        modified_block,
        affected_block_file_path,
        affected_block_name,
        message,
    )
}

/// A reference whose target block no longer exists (renamed or deleted). Reported instead of
/// `create_violation`'s "is modified, but X is not", which would misdescribe a missing target as an
/// unchanged one.
fn dangling_reference_violation(
    referencing_block_file_path: &RepoPath,
    referencing_block: &Block,
    target_file_path: &RepoPath,
    target_name: &str,
) -> anyhow::Result<Violation> {
    let message = format!(
        "Block {}:{} at line {} references {}:{}, which does not exist",
        referencing_block_file_path.display(),
        referencing_block.name_display(),
        referencing_block.start_tag_position_range.start().line,
        target_file_path.display(),
        target_name,
    );
    affects_violation(referencing_block, target_file_path, target_name, message)
}

/// Builds an `affects` violation anchored on the referencing block's start tag, carrying the
/// referenced target as machine-readable details. Shared by both violation kinds so they serialize
/// the same shape and differ only in their human-readable `message`.
fn affects_violation(
    block: &Block,
    affected_block_file_path: &RepoPath,
    affected_block_name: &str,
    message: String,
) -> anyhow::Result<Violation> {
    let details = serde_json::to_value(AffectsViolation {
        affected_block_file_path,
        affected_block_name,
    })
    .context("failed to serialize AffectsViolation block")?;
    Ok(Violation::new(
        ViolationRange::new(
            block.start_tag_position_range.start().clone(),
            block.start_tag_position_range.end().clone(),
        ),
        "affects".to_string(),
        message,
        block.severity()?,
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
        // Both targets exist but only their start tags changed, so they stay in scope with
        // unmodified content and each modified source that points at them is a violation.
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
                r#"# <block name="bar">
print("file3")
# </block>
"#,
                vec![LineChange {
                    line: 1, // Only the start tag is changed, not the content.
                    ranges: Some(vec![3..8, 10..15]),
                }],
            ),
        ]);

        let violations = validator.validate(context)?.violations;

        assert_eq!(violations.len(), 1);
        let file1_violations = violations
            .get(&RepoPath::from_reference("file1.py")?)
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
        let file2 = r#"# <block name="buzz" affects="file1.py:bar">
print("not-buzz")
# </block>
print("hello")
"#;
        let validator = validator(&[("file2.py", file2)]);
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
                file2,
                vec![LineChange {
                    line: 4, // Line outside the block is changed.
                    ranges: None,
                }],
            ),
        ]);

        let violations = validator.validate(context)?.violations;

        assert_eq!(violations.len(), 1);
        let file1_violations = violations
            .get(&RepoPath::from_reference("file1.py")?)
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
        let source = r#"# <block affects=":foo">
print("first")
# </block>

# <block name="foo">
print("second")
# </block>
"#;
        let validator = validator(&[("file1.py", source)]);
        let context = validation_context_with_changes(
            "file1.py",
            source,
            vec![LineChange {
                line: 2,
                ranges: None,
            }],
        );

        let violations = validator.validate(context)?.violations;

        assert_eq!(violations.len(), 1);
        let file1_violations = violations
            .get(&RepoPath::from_reference("file1.py")?)
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
        let contents = r#"# <block name="foo" affects=":bar">
print("foo")
# </block>

# <block name="bar" affects=":foo">
pass
# </block>
"#;
        let validator = validator(&[("file1.py", contents)]);
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
    fn modified_block_referencing_a_nonexistent_target_reports_a_dangling_reference()
    -> anyhow::Result<()> {
        // A renamed or deleted target block is a dangling reference.
        let source = "# <block name=\"s\" affects=\":nope\">\nvalue = 1\n# </block>";
        let context = validation_context("file.py", source);
        let violations = validator(&[("file.py", source)])
            .validate(context)?
            .violations;
        let file = violations
            .get(&RepoPath::from_reference("file.py")?)
            .unwrap();
        assert_eq!(file.len(), 1);
        assert!(
            file[0].message.contains("does not exist"),
            "expected a dangling-reference message, got: {}",
            file[0].message
        );
        Ok(())
    }

    #[test]
    fn unmodified_block_referencing_a_deleted_target_returns_a_violation() -> anyhow::Result<()> {
        let source = "# <block name=\"s\" affects=\":nope\">\nvalue = 1\n# </block>";
        let context = validation_context_with_changes(
            "file.py",
            source,
            // Touch only the start tag, so the block stays in scope but its content is unmodified.
            vec![LineChange {
                line: 1,
                ranges: Some(vec![3..8, 10..15]),
            }],
        );
        let violations = validator(&[("file.py", source)])
            .validate(context)?
            .violations;
        let file = violations
            .get(&RepoPath::from_reference("file.py")?)
            .unwrap();
        assert_eq!(file.len(), 1);
        assert!(
            file[0].message.contains("does not exist"),
            "expected a dangling-reference message, got: {}",
            file[0].message
        );
        Ok(())
    }

    #[test]
    fn reference_to_a_missing_target_file_fails_the_run() -> anyhow::Result<()> {
        let context = validation_context(
            "source.py",
            "# <block name=\"s\" affects=\"gone.py:t\">\nvalue = 1\n# </block>",
        );
        assert!(validator(&[]).validate(context).is_err());
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
