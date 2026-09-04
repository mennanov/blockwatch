use crate::blocks::{Block, BlockWithContext, FileBlocks, every_block, parse_file};
use crate::fs::FileSystem;
use crate::repo_path::RepoPath;
use crate::validators;
use crate::validators::{
    BlockReference, ValidationReport, ValidatorType, Violation, ViolationRange,
};
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
    /// Present only when a block referenced by name (as opposed to a whole file).
    #[serde(skip_serializing_if = "Option::is_none")]
    affected_block_name: Option<&'a str>,
}

impl<Fs: FileSystem + 'static> validators::ValidatorSync for AffectsValidator<Fs> {
    fn validate(
        &self,
        context: Arc<validators::ValidationContext>,
    ) -> anyhow::Result<ValidationReport> {
        let mut targets = TargetIndex::new(&context, self.file_system.as_ref());
        let mut report = ValidationReport::default();
        // Every block carrying `affects` is examined, not only the ones the diff touched: reference
        // integrity has to catch a dangling target even when the referencing block is unchanged.
        for (file_path, file_blocks) in &context.blocks {
            for block_with_context in &file_blocks.blocks_with_context {
                let Some(affects) = block_with_context.block.attributes.get("affects") else {
                    continue;
                };
                let violations =
                    reference_violations(&mut targets, file_path, block_with_context, affects)?;
                report.add_all(file_path, &block_with_context.block, violations);
            }
        }
        Ok(report)
    }
}

/// Answers what a run needs to know about the targets its references name: whether a target exists,
/// and whether the diff changed it.
///
/// One instance serves a whole run, so a file read to resolve one reference is parsed once and its
/// named blocks are reused by every later reference landing on it.
struct TargetIndex<'a, Fs: FileSystem> {
    context: &'a validators::ValidationContext,
    /// Reads target files that are not already parsed into the validation context.
    file_system: &'a Fs,
    /// Named blocks, with their modification bit, of the files the run already parsed.
    in_scope: HashMap<&'a RepoPath, HashMap<String, bool>>,
    /// The same, for the files read from the filesystem so far.
    read: HashMap<RepoPath, HashMap<String, bool>>,
}

impl<'a, Fs: FileSystem> TargetIndex<'a, Fs> {
    fn new(context: &'a validators::ValidationContext, file_system: &'a Fs) -> Self {
        Self {
            context,
            file_system,
            in_scope: context
                .blocks
                .iter()
                .map(|(file_path, file_blocks)| (file_path, name_index(file_blocks)))
                .collect(),
            read: HashMap::new(),
        }
    }

    /// Whether the block named `target_name` in the `target_file` is modified.
    ///
    /// Returns `Some(was_modified)` when the block exists; `None` when the file parses but holds no
    /// block with that name - a dangling reference.
    ///
    /// A missing or unsupported target file is an `Err`.
    fn block_modified(
        &mut self,
        target_file: &RepoPath,
        target_name: &str,
    ) -> anyhow::Result<Option<bool>> {
        if let Some(index) = self.in_scope.get(target_file)
            && let Some(&modified) = index.get(target_name)
        {
            return Ok(Some(modified));
        }
        // Parsing with the target's own line changes recovers both its blocks and their modified
        // state; a file the diff never referenced simply has no line changes, so nothing in it
        // counts as modified.
        let line_changes = self.context.line_changes_for(target_file).unwrap_or(&[]);
        let index = match self.read.entry(target_file.clone()) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let parsed = parse_file(
                    self.file_system,
                    target_file.as_path(),
                    line_changes,
                    every_block,
                    self.context.parsers(),
                    self.context.extra_file_extensions(),
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

    /// Whether the diff mentions `target_file`.
    fn file_modified(&self, target_file: &RepoPath) -> bool {
        self.context.line_changes_for(target_file).is_some()
    }

    fn file_exists(&self, target_file: &RepoPath) -> bool {
        self.file_system.exists(target_file.as_path())
    }
}

/// One violation per reference listed in the `affects` attribute.
///
/// 2 violation kinds possible:
/// 1. The referenced target no longer exists.
/// 2. The referenced target is not modified while `block_with_context` is.
///
/// `affects` is the attribute's raw value from the block in `block_with_context`.
fn reference_violations<Fs: FileSystem>(
    targets: &mut TargetIndex<'_, Fs>,
    file_path: &RepoPath,
    block_with_context: &BlockWithContext,
    affects: &str,
) -> anyhow::Result<Vec<Violation>> {
    let mut violations = Vec::new();
    for reference in parse_references(file_path, &block_with_context.block, affects)? {
        let violation = match reference {
            BlockReference::Block { file, name } => block_reference_violation(
                targets,
                file_path,
                block_with_context,
                // A reference like ":foo" is resolved relative to the file the block is in.
                &file.unwrap_or_else(|| file_path.clone()),
                &name,
            )?,
            BlockReference::File(target_file) => {
                file_reference_violation(targets, file_path, block_with_context, &target_file)?
            }
        };
        violations.extend(violation);
    }
    Ok(violations)
}

/// Parses the attribute's raw value, naming the block that wrote it so an author can find the
/// offending reference without searching for it.
fn parse_references(
    file_path: &RepoPath,
    block: &Block,
    affects: &str,
) -> anyhow::Result<Vec<BlockReference>> {
    validators::parse_block_references(affects).with_context(|| {
        format!(
            "invalid affects reference on block {}:{} at line {}",
            file_path,
            block.name_display(),
            block.start_tag_position_range.start().line,
        )
    })
}

/// The violation a reference to a named block produces, if any.
fn block_reference_violation<Fs: FileSystem>(
    targets: &mut TargetIndex<'_, Fs>,
    file_path: &RepoPath,
    block_with_context: &BlockWithContext,
    target_file: &RepoPath,
    target_name: &str,
) -> anyhow::Result<Option<Violation>> {
    let Some(target_modified) = targets.block_modified(target_file, target_name)? else {
        return Ok(Some(dangling_reference_violation(
            file_path,
            &block_with_context.block,
            target_file,
            Some(target_name),
        )?));
    };
    // The "affects" only applies when this block's content changed: an unchanged block places no
    // obligation on the blocks it references.
    if block_with_context.is_content_modified && !target_modified {
        return Ok(Some(create_violation(
            file_path,
            &block_with_context.block,
            target_file,
            Some(target_name),
        )?));
    }
    Ok(None)
}

/// The violation a whole-file reference produces, if any.
fn file_reference_violation<Fs: FileSystem>(
    targets: &TargetIndex<'_, Fs>,
    file_path: &RepoPath,
    block_with_context: &BlockWithContext,
    target_file: &RepoPath,
) -> anyhow::Result<Option<Violation>> {
    // A whole-file target is never parsed, so its existence is the only integrity signal there is;
    // a missing one aborts the run, as a missing file holding a named target does.
    if !targets.file_exists(target_file) {
        return Err(anyhow!(
            "affects target file does not exist: {}",
            target_file.display()
        ));
    }
    if block_with_context.is_content_modified && !targets.file_modified(target_file) {
        return Ok(Some(create_violation(
            file_path,
            &block_with_context.block,
            target_file,
            None,
        )?));
    }
    Ok(None)
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
    affected_block_name: Option<&str>,
) -> anyhow::Result<Violation> {
    let message = format!(
        "Block {}:{} at line {} is modified, but {} is not",
        modified_block_file_path.display(),
        modified_block.name_display(),
        modified_block.start_tag_position_range.start().line,
        target_display(affected_block_file_path, affected_block_name),
    );
    affects_violation(
        modified_block_file_path,
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
    target_name: Option<&str>,
) -> anyhow::Result<Violation> {
    let message = format!(
        "Block {}:{} at line {} references {}, which does not exist",
        referencing_block_file_path.display(),
        referencing_block.name_display(),
        referencing_block.start_tag_position_range.start().line,
        target_display(target_file_path, target_name),
    );
    affects_violation(
        referencing_block_file_path,
        referencing_block,
        target_file_path,
        target_name,
        message,
    )
}

/// Target's display string.
///
/// Returns either a `file:name` for a named block or `file <path>` for a whole-file reference.
fn target_display(file_path: &RepoPath, name: Option<&str>) -> String {
    match name {
        Some(name) => format!("{}:{}", file_path.display(), name),
        None => format!("file {}", file_path.display()),
    }
}

/// Builds an `affects` violation anchored on the referencing block's start tag, carrying the
/// referenced target as machine-readable details. Shared by both violation kinds so they serialize
/// the same shape and differ only in their human-readable `message`.
fn affects_violation(
    file_path: &RepoPath,
    block: &Block,
    affected_block_file_path: &RepoPath,
    affected_block_name: Option<&str>,
    message: String,
) -> anyhow::Result<Violation> {
    let details = serde_json::to_value(AffectsViolation {
        affected_block_file_path,
        affected_block_name,
    })
    .context("failed to serialize AffectsViolation block")?;
    Violation::new(
        ViolationRange::new(
            block.start_tag_position_range.start().clone(),
            block.start_tag_position_range.end().clone(),
        ),
        file_path,
        block,
        "affects".to_string(),
        message,
        // A block may reference several targets, so the target identifies this violation.
        Some(&target_display(
            affected_block_file_path,
            affected_block_name,
        )),
        Some(details),
    )
}

#[cfg(test)]
mod validate_tests {
    use super::*;
    use crate::diff_parser::LineChange;
    use crate::fs::test_utils::FakeFileSystem;
    use crate::repo_path::RepoPath;
    use crate::test_utils::{
        checked_lines, merge_validation_contexts, validation_context,
        validation_context_with_changes, violation_count,
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

    /// Create a [`validators::ValidationContext`] with `files` and the diff from `changed_files`.
    ///
    /// A changed file need not hold blocks or even be parseable by tree-sitter: that is what a
    /// whole-file reference is for.
    fn context_with_changed_files(
        files: &[(&str, &str)],
        changed_files: &[&str],
    ) -> anyhow::Result<Arc<validators::ValidationContext>> {
        let file_system = FakeFileSystem::new(
            files
                .iter()
                .map(|(path, contents)| (path.to_string(), contents.to_string()))
                .collect(),
        );
        let mut line_changes = HashMap::new();
        for changed_file in changed_files {
            let contents = files
                .iter()
                .find(|(path, _)| path == changed_file)
                .map(|(_, contents)| *contents)
                .unwrap_or_default();
            line_changes.insert(
                RepoPath::from_reference(changed_file)?,
                contents
                    .lines()
                    .enumerate()
                    .map(|(index, _)| LineChange {
                        line: index + 1,
                        ranges: None,
                    })
                    .collect(),
            );
        }
        let parsers = crate::language_parsers::language_parsers()?;
        let parsed = crate::blocks::parse_blocks(
            &line_changes,
            crate::blocks::ScanMode::All,
            &file_system,
            &crate::fs::test_utils::FakePathChecker::allow_all(),
            &parsers,
            &HashMap::new(),
        )?;
        Ok(Arc::new(validators::ValidationContext::new(
            parsed.blocks,
            parsers,
            line_changes,
            HashMap::new(),
        )))
    }

    /// A source file whose only block points at a whole file that no grammar can parse.
    const WHOLE_FILE_REFERENCE_FILES: [(&str, &str); 2] = [
        (
            "source.py",
            "# <block name=\"s\" affects=\"config.json\">\nvalue = 2\n# </block>",
        ),
        ("config.json", "{\"value\": 2}\n"),
    ];

    #[test]
    fn modified_block_with_an_unmodified_whole_file_target_returns_a_violation()
    -> anyhow::Result<()> {
        let context = context_with_changed_files(&WHOLE_FILE_REFERENCE_FILES, &["source.py"])?;

        let violations = validator(&WHOLE_FILE_REFERENCE_FILES)
            .validate(context)?
            .violations;

        let source_violations = violations
            .get(&RepoPath::from_reference("source.py")?)
            .unwrap();
        assert_eq!(source_violations.len(), 1);
        assert_eq!(
            source_violations[0].message,
            "Block source.py:s at line 1 is modified, but file config.json is not"
        );
        assert_eq!(
            source_violations[0].data,
            Some(serde_json::json!({"affected_block_file_path": "config.json"}))
        );
        Ok(())
    }

    #[test]
    fn modified_block_with_a_modified_whole_file_target_returns_no_violations() -> anyhow::Result<()>
    {
        let context =
            context_with_changed_files(&WHOLE_FILE_REFERENCE_FILES, &["source.py", "config.json"])?;

        let report = validator(&WHOLE_FILE_REFERENCE_FILES).validate(context)?;

        assert_eq!(violation_count(&report), 0);
        Ok(())
    }

    #[test]
    fn unmodified_block_with_an_unmodified_whole_file_target_returns_no_violations()
    -> anyhow::Result<()> {
        // An unchanged block places no obligation on what it references.
        let context = context_with_changed_files(&WHOLE_FILE_REFERENCE_FILES, &[])?;

        let report = validator(&WHOLE_FILE_REFERENCE_FILES).validate(context)?;

        assert_eq!(violation_count(&report), 0);
        Ok(())
    }

    #[test]
    fn whole_file_reference_to_a_missing_file_returns_an_error() -> anyhow::Result<()> {
        // Reference integrity is checked whether or not the referencing block changed, so the run
        // aborts even here, where nothing was modified.
        let files = [(
            "source.py",
            "# <block name=\"s\" affects=\"gone.json\">\nvalue = 1\n# </block>",
        )];
        let context = context_with_changed_files(&files, &[])?;

        let error = validator(&files).validate(context).unwrap_err();

        assert_eq!(
            error.to_string(),
            "affects target file does not exist: gone.json"
        );
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
