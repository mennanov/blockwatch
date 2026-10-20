use crate::blocks::{Block, BlockWithContext, FileBlocks};
use crate::fs::FileSystem;
use crate::repo_path::RepoPath;
use crate::symbol_path::SymbolPath;
use crate::target_reference::{ResolvedTarget, TargetKind, TargetReference};
use crate::validators;
use crate::validators::{
    TargetFiles, TargetResult, ValidationReport, Validator, ValidatorTrace, ValidatorType,
    Violation,
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
struct AffectsValidator<Fs: FileSystem> {
    /// Reads target files that are not already parsed into the validation context.
    file_system: Arc<Fs>,
}

impl<Fs: FileSystem + 'static> AffectsValidator<Fs> {
    /// Creates the validator over the filesystem it will read excluded target files from.
    fn new(file_system: Arc<Fs>) -> Self {
        Self { file_system }
    }
}

#[derive(Serialize)]
struct AffectsViolation<'a> {
    affected_block_file_path: &'a RepoPath,
    /// A block name, or a symbol path with its leading `#`. Absent for a whole-file reference.
    #[serde(skip_serializing_if = "Option::is_none")]
    affected_block_name: Option<&'a str>,
}

impl<Fs: FileSystem + 'static> validators::SyncValidator for AffectsValidator<Fs> {
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
                let mut trace = context.traces_requested.then(|| AffectsTrace {
                    block_changed: block_with_context.is_content_modified,
                    targets: Vec::new(),
                });
                let violations = reference_violations(
                    &mut targets,
                    file_path,
                    block_with_context,
                    affects,
                    trace.as_mut().map(|trace| &mut trace.targets),
                )?;
                report.add_all(file_path, &block_with_context.block, violations);
                if let Some(trace) = trace {
                    let trace = ValidatorTrace::Affects(trace);
                    report.add_trace(file_path, &block_with_context.block, trace);
                }
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
    /// The text and the symbols of the files that symbol references point into.
    files: TargetFiles<'a, Fs>,
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
            files: TargetFiles::new(context, file_system),
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
        let target_file = &self.real_path(target_file);
        if let Some(index) = self.in_scope.get(target_file)
            && let Some(&modified) = index.get(target_name)
        {
            return Ok(Some(modified));
        }
        let index = match self.read.entry(target_file.clone()) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let parsed = self
                    .context
                    .parse_file(self.file_system, target_file)?
                    .ok_or_else(|| {
                        anyhow!("affects target file format is unsupported: {target_file}")
                    })?;
                entry.insert(name_index(&parsed))
            }
        };
        Ok(index.get(target_name).copied())
    }

    /// Whether the diff touches the definition of the symbol at `path` in `target_file`, in any
    /// of the places it is written.
    ///
    /// Resolves to a reason instead when no symbol, or more than one, has `path`. A file that
    /// cannot be read, has no grammar or no symbols, or does not parse is an `Err`.
    fn modifies_symbol(
        &mut self,
        target_file: &RepoPath,
        path: &SymbolPath,
    ) -> anyhow::Result<TargetResult<bool>> {
        let target_file = &self.real_path(target_file);
        // A file the diff never mentions has no line changes, so nothing in it counts as modified.
        let line_changes = self.context.line_changes().for_file(target_file);
        let (content, resolution) = self.files.resolve_symbol(target_file, path)?;
        Ok(resolution.map(|symbol| {
            symbol
                .position_ranges(content)
                .iter()
                .any(|range| line_changes.touches(range))
        }))
    }

    /// Whether the diff mentions `target_file`.
    fn file_modified(&self, target_file: &RepoPath) -> bool {
        self.context
            .line_changes()
            .mentions(&self.real_path(target_file))
    }

    fn file_exists(&self, target_file: &RepoPath) -> bool {
        self.file_system.exists(target_file.as_path())
    }

    /// The path of `target_file`, with a symlink replaced by the file it points at. Returns
    /// `target_file` itself when it does not resolve, such as when the file is missing.
    fn real_path(&self, target_file: &RepoPath) -> RepoPath {
        // The walk skips a symlink and the diff drops it, so a linked file's blocks and changes are
        // only found under the path of the file it points at.
        self.file_system
            .repo_path(target_file.as_path())
            .unwrap_or_else(|| target_file.clone())
    }
}

/// One violation per reference listed in the `affects` attribute. Adds what the validator saw
/// in each target to `trace`, when given.
///
/// 2 violation kinds possible:
/// 1. The referenced target no longer exists.
/// 2. The referenced target is not modified while `block_with_context` is.
///
/// `affects` is the attribute's raw value from the block in `block_with_context`.
///
/// # Errors
/// Returns an error when a target file is missing or can't be parsed.
fn reference_violations<Fs: FileSystem>(
    targets: &mut TargetIndex<'_, Fs>,
    file_path: &RepoPath,
    block_with_context: &BlockWithContext,
    affects: &str,
    mut trace: Option<&mut Vec<AffectsTarget>>,
) -> anyhow::Result<Vec<Violation>> {
    let block = &block_with_context.block;
    let mut violations = Vec::new();
    for reference in parse_references(file_path, block, affects)? {
        let target = reference.resolve(file_path);
        let changed = match &target.kind {
            TargetKind::Block(name) => targets
                .block_modified(&target.file, name)?
                .ok_or_else(|| "does not exist".to_string()),
            TargetKind::WholeFile => {
                // A whole-file target is never parsed, so its existence is the only integrity
                // signal there is; a missing one aborts the run, as a missing file holding a named
                // target does.
                if !targets.file_exists(&target.file) {
                    return Err(anyhow!(
                        "affects target file does not exist: {}",
                        target.file
                    ));
                }
                Ok(targets.file_modified(&target.file))
            }
            TargetKind::Symbol(path) => targets.modifies_symbol(&target.file, path)?,
        };
        if let Some(trace) = trace.as_deref_mut() {
            trace.push(AffectsTarget {
                changed: changed.as_ref().ok().copied(),
                reason: changed.as_ref().err().cloned(),
                target: target.to_string(),
            });
        }
        let violation = match changed {
            // An unchanged block places no obligation on the targets it references.
            Ok(changed) => (block_with_context.is_content_modified && !changed)
                .then(|| create_violation(file_path, block, &target))
                .transpose()?,
            Err(reason) => Some(unresolved_violation(file_path, block, &target, &reason)?),
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
) -> anyhow::Result<Vec<TargetReference>> {
    TargetReference::parse_list(affects).with_context(|| {
        format!(
            "invalid affects reference on block {}",
            block.label(file_path),
        )
    })
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

/// What `affects` saw in one block.
#[derive(Serialize, Debug)]
pub(crate) struct AffectsTrace {
    /// Whether the diff changed the block's content. Only then must its targets change too.
    block_changed: bool,
    /// One entry for each reference in `affects`, in the order the attribute lists them.
    targets: Vec<AffectsTarget>,
}

/// What `affects` saw in one target. Exactly one of `changed` and `reason` is set.
///
/// serde writes the keys in the order the fields are declared, and the output keeps its keys
/// sorted. So declare the fields in alphabetical order.
#[derive(Serialize, Debug)]
struct AffectsTarget {
    /// Whether the diff changed the target. `None` when the target doesn't resolve.
    #[serde(skip_serializing_if = "Option::is_none")]
    changed: Option<bool>,
    /// Why the target doesn't resolve, as the violation shows it. `None` when it resolves.
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    /// The target, as the violation shows it.
    target: String,
}

/// Selects [`AffectsValidator`] for every block that carries an `affects` attribute.
///
/// It does not gate on whether the block was modified: the validator also checks that each
/// referenced target still resolves, and a dangling reference has to be reported even when the
/// referencing block itself is untouched.
pub(super) fn detect<Fs: FileSystem + 'static>(
    block_with_context: &BlockWithContext,
    file_system: &Arc<Fs>,
) -> Option<ValidatorType> {
    block_with_context
        .block
        .attributes
        .contains_key("affects")
        .then(|| ValidatorType::Sync(Box::new(AffectsValidator::new(Arc::clone(file_system)))))
}

fn create_violation(
    modified_block_file_path: &RepoPath,
    modified_block: &Block,
    target: &ResolvedTarget,
) -> anyhow::Result<Violation> {
    let message = format!(
        "Block {} is modified, but {target} is not",
        modified_block.label(modified_block_file_path),
    );
    affects_violation(modified_block_file_path, modified_block, target, message)
}

/// The violation of a reference whose target doesn't resolve, for `reason`. Reported instead of
/// `create_violation`'s "is modified, but X is not", which would misdescribe a missing target as an
/// unchanged one.
fn unresolved_violation(
    file_path: &RepoPath,
    block: &Block,
    target: &ResolvedTarget,
    reason: &str,
) -> anyhow::Result<Violation> {
    let message = match &target.kind {
        TargetKind::Block(_) => format!(
            "Block {} references {target}, which {reason}",
            block.label(file_path)
        ),
        TargetKind::Symbol(_) | TargetKind::WholeFile => format!(
            "Block {} references {target}: {reason}",
            block.label(file_path)
        ),
    };
    affects_violation(file_path, block, target, message)
}

/// Builds an `affects` violation anchored on the referencing block's start tag, carrying the
/// referenced target as machine-readable details. Shared by both violation kinds so they serialize
/// the same shape and differ only in their human-readable `message`.
fn affects_violation(
    file_path: &RepoPath,
    block: &Block,
    target: &ResolvedTarget,
    message: String,
) -> anyhow::Result<Violation> {
    let name = target.name();
    let details = serde_json::to_value(AffectsViolation {
        affected_block_file_path: &target.file,
        affected_block_name: name.as_deref(),
    })?;
    Ok(Violation::new(
        block.start_tag_position_range.clone().into(),
        file_path,
        block,
        Validator::Affects,
        message,
        // A block may reference several targets, so the target identifies this violation.
        Some(&target.to_string()),
        Some(details),
    ))
}

#[cfg(test)]
mod validate_tests {
    use super::*;
    use crate::diff_parser::{LineChange, LineChangeKind};
    use crate::fs::test_utils::FakeFileSystem;
    use crate::repo_path::RepoPath;
    use crate::test_utils::{
        diff_changes, validation_context, validation_context_with_changes, virtual_block,
    };
    use crate::validators::SyncValidator;
    use crate::validators::test_utils::{
        checked_lines, merge_validation_contexts, violation_count, with_virtual_blocks,
    };

    /// Builds a validator with a fake filesystem seeded with `files` (path, contents).
    pub(super) fn validator(files: &[(&str, &str)]) -> AffectsValidator<FakeFileSystem> {
        validator_with_symlinks(files, &[])
    }

    /// Like [`validator`], with `symlinks` (link, the path it points at) in the filesystem too.
    fn validator_with_symlinks(
        files: &[(&str, &str)],
        symlinks: &[(&str, &str)],
    ) -> AffectsValidator<FakeFileSystem> {
        let map = files
            .iter()
            .map(|(p, c)| (p.to_string(), c.to_string()))
            .collect();
        AffectsValidator::new(Arc::new(FakeFileSystem::new(map).with_symlinks(symlinks)))
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
        let line_changes = diff_changes(line_changes);
        let parsers = crate::language_parsers::language_parsers()?;
        let parsed = crate::scan::Scan::new(
            file_system,
            &crate::fs::PathFilter::allow_only("source.py"),
            &parsers,
            &[],
        )
        .parse_blocks(&line_changes, crate::scan::ScanMode::OnlyChanged)?;
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
            Vec::new(),
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
                    kind: LineChangeKind::Modified(vec![3..8, 10..15]),
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
                    kind: LineChangeKind::Modified(vec![3..8, 10..15]),
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

    /// A file with a block named `r`, which `CLAUDE.md` is a symlink to in the symlink tests.
    const AGENTS_MD: &str = "<!-- <block name=\"r\"> -->\nrules\n<!-- </block> -->\n";

    /// A source file whose only block points at the block `r` through the symlink `CLAUDE.md`.
    const SOURCE_THROUGH_A_SYMLINK: &str =
        "# <block affects=\"CLAUDE.md:r\">\nvalue = 1\n# </block>\n";

    #[test]
    fn modified_block_with_a_modified_target_through_a_symlink_returns_no_violations()
    -> anyhow::Result<()> {
        let context = merge_validation_contexts(vec![
            validation_context("source.py", SOURCE_THROUGH_A_SYMLINK),
            validation_context("AGENTS.md", AGENTS_MD),
        ]);

        let report =
            validator_with_symlinks(&[("AGENTS.md", AGENTS_MD)], &[("CLAUDE.md", "AGENTS.md")])
                .validate(context)?;

        assert_eq!(violation_count(&report), 0);
        Ok(())
    }

    #[test]
    fn modified_block_with_an_unmodified_target_through_a_symlink_returns_a_violation()
    -> anyhow::Result<()> {
        let context = validation_context("source.py", SOURCE_THROUGH_A_SYMLINK);

        let violations =
            validator_with_symlinks(&[("AGENTS.md", AGENTS_MD)], &[("CLAUDE.md", "AGENTS.md")])
                .validate(context)?
                .violations;

        let source_violations = &violations[&RepoPath::from_reference("source.py")?];
        assert_eq!(source_violations.len(), 1);
        assert_eq!(
            source_violations[0].message,
            "Block source.py:(unnamed) at line 1 is modified, but CLAUDE.md:r is not"
        );
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
                    kind: LineChangeKind::Added,
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
                kind: LineChangeKind::Added,
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
                        kind: LineChangeKind::Modified(vec![0..10, 12..15]),
                    }, // First block start tag
                    LineChange {
                        line: 7,
                        kind: LineChangeKind::Added,
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
                    kind: LineChangeKind::Modified(vec![0..4, 6..10]),
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
                    kind: LineChangeKind::Added,
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
                kind: LineChangeKind::Added,
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
                    kind: LineChangeKind::Added,
                }],
            ),
            (
                RepoPath::from_reference("target.py")?,
                vec![LineChange {
                    line: 2,
                    kind: LineChangeKind::Added,
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
    fn modified_virtual_block_outside_the_globs_returns_no_violations() -> anyhow::Result<()> {
        let file_system = FakeFileSystem::new(HashMap::from([
            (
                "source.py".to_string(),
                r#"# <block affects="package.json:version">
value = 2
# </block>
"#
                .to_string(),
            ),
            (
                "package.json".to_string(),
                r#"{
  "version": "1.2.3"
}
"#
                .to_string(),
            ),
        ]));
        let added = |line| LineChange {
            line,
            kind: LineChangeKind::Added,
        };
        let line_changes = HashMap::from([
            (RepoPath::from_reference("source.py")?, vec![added(2)]),
            (RepoPath::from_reference("package.json")?, vec![added(2)]),
        ]);
        // The validator reads `package.json` itself. The block it refers to is declared in the
        // config file.
        let context = with_virtual_blocks(
            context_scoped_to_source(&file_system, line_changes)?,
            vec![virtual_block(
                "package.json#/version",
                &[("name", "version")],
            )?],
        );

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
                kind: LineChangeKind::Added,
            }, // First block's content line
            LineChange {
                line: 4, // Not in any of the blocks.
                kind: LineChangeKind::Added,
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
                kind: LineChangeKind::Modified(vec![3..8, 10..15]),
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
    pub(super) fn context_with_changed_files(
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
                        kind: LineChangeKind::Added,
                    })
                    .collect(),
            );
        }
        let line_changes = diff_changes(line_changes);
        let parsers = crate::language_parsers::language_parsers()?;
        let parsed = crate::scan::Scan::new(
            &file_system,
            &crate::fs::PathFilter::allow_all(),
            &parsers,
            &[],
        )
        .parse_blocks(&line_changes, crate::scan::ScanMode::All)?;
        Ok(Arc::new(validators::ValidationContext::new(
            parsed.blocks,
            parsers,
            line_changes,
            Vec::new(),
        )))
    }

    /// A source file whose only block points at a whole file.
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
            "Block source.py:s at line 1 is modified, but config.json is not"
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
    fn modified_block_with_a_modified_whole_file_target_through_a_symlink_returns_no_violations()
    -> anyhow::Result<()> {
        let files = [
            (
                "source.py",
                "# <block name=\"s\" affects=\"settings.json\">\nvalue = 2\n# </block>",
            ),
            ("config.json", "{\"value\": 2}\n"),
        ];
        let context = context_with_changed_files(&files, &["source.py", "config.json"])?;

        let report = validator_with_symlinks(&files, &[("settings.json", "config.json")])
            .validate(context)?;

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

    /// The target file of the symbol reference tests.
    const PACKAGE_JSON: &str = r#"{
  "version": "1.0",
  "description": "x",
  "dependencies": {
    "inngest": "4.18.1"
  }
}"#;

    #[test]
    fn modified_block_with_a_modified_symbol_target_returns_no_violations() -> anyhow::Result<()> {
        let context = merge_validation_contexts(vec![
            validation_context(
                "source.py",
                r#"# <block affects="package.json#/version">
version = "1.1"
# </block>"#,
            ),
            validation_context_with_changes(
                "package.json",
                PACKAGE_JSON,
                vec![LineChange {
                    line: 2,
                    kind: LineChangeKind::Added,
                }],
            ),
        ]);

        let report = validator(&[("package.json", PACKAGE_JSON)]).validate(context)?;

        assert_eq!(violation_count(&report), 0);
        Ok(())
    }

    #[test]
    fn modified_block_with_a_modified_symbol_target_through_a_symlink_returns_no_violations()
    -> anyhow::Result<()> {
        let context = merge_validation_contexts(vec![
            validation_context(
                "source.py",
                r#"# <block affects="settings.json#/version">
version = "1.1"
# </block>"#,
            ),
            validation_context_with_changes(
                "package.json",
                PACKAGE_JSON,
                vec![LineChange {
                    line: 2,
                    kind: LineChangeKind::Added,
                }],
            ),
        ]);

        let report = validator_with_symlinks(
            &[("package.json", PACKAGE_JSON)],
            &[("settings.json", "package.json")],
        )
        .validate(context)?;

        assert_eq!(violation_count(&report), 0);
        Ok(())
    }

    #[test]
    fn modified_block_with_a_symbol_target_unmodified_returns_a_violation() -> anyhow::Result<()> {
        // The diff touches `/description` only, which leaves `/version` as it was.
        let context = merge_validation_contexts(vec![
            validation_context(
                "source.py",
                r#"# <block affects="package.json#/version">
version = "1.1"
# </block>"#,
            ),
            validation_context_with_changes(
                "package.json",
                PACKAGE_JSON,
                vec![LineChange {
                    line: 3,
                    kind: LineChangeKind::Added,
                }],
            ),
        ]);

        let violations = validator(&[("package.json", PACKAGE_JSON)])
            .validate(context)?
            .violations;

        let source_violations = &violations[&RepoPath::from_reference("source.py")?];
        assert_eq!(source_violations.len(), 1);
        assert_eq!(
            source_violations[0].message,
            "Block source.py:(unnamed) at line 1 is modified, but package.json#/version is not"
        );
        assert_eq!(
            source_violations[0].data,
            Some(serde_json::json!({
                "affected_block_file_path": "package.json",
                "affected_block_name": "#/version",
            }))
        );
        Ok(())
    }

    #[test]
    fn modified_block_with_a_symbol_target_changed_below_its_first_line_returns_no_violations()
    -> anyhow::Result<()> {
        // A definition can span several lines, as the object `/dependencies` does here. A change on
        // any of them changes the symbol, not only a change on the line the definition starts on.
        let context = merge_validation_contexts(vec![
            validation_context(
                "source.py",
                r#"# <block affects="package.json#/dependencies">
inngest = "4.18.2"
# </block>"#,
            ),
            validation_context_with_changes(
                "package.json",
                PACKAGE_JSON,
                vec![LineChange {
                    line: 5,
                    kind: LineChangeKind::Added,
                }],
            ),
        ]);

        let report = validator(&[("package.json", PACKAGE_JSON)]).validate(context)?;

        assert_eq!(violation_count(&report), 0);
        Ok(())
    }

    #[test]
    fn modified_block_with_a_symbol_target_after_a_deleted_line_returns_a_violation()
    -> anyhow::Result<()> {
        // The key starts at column 1, exactly where the deletion leaves its gap. The deleted line
        // sat before the key and was never part of its definition, so the key did not change.
        let target = r#"{
"version": "1.0"
}"#;
        let context = merge_validation_contexts(vec![
            validation_context(
                "source.py",
                r#"# <block affects="package.json#/version">
version = "1.1"
# </block>"#,
            ),
            validation_context_with_changes(
                "package.json",
                target,
                vec![LineChange {
                    line: 2,
                    kind: LineChangeKind::Deleted,
                }],
            ),
        ]);

        let report = validator(&[("package.json", target)]).validate(context)?;

        assert_eq!(violation_count(&report), 1);
        Ok(())
    }

    #[test]
    fn unmodified_block_with_an_unmodified_symbol_target_returns_no_violations()
    -> anyhow::Result<()> {
        let files = [
            (
                "source.py",
                r#"# <block affects="package.json#/version">
version = "1.0"
# </block>"#,
            ),
            ("package.json", PACKAGE_JSON),
        ];
        let context = context_with_changed_files(&files, &[])?;

        let report = validator(&files).validate(context)?;

        assert_eq!(violation_count(&report), 0);
        Ok(())
    }

    #[test]
    fn unmodified_block_with_a_missing_symbol_target_returns_a_violation() -> anyhow::Result<()> {
        // Reference integrity is checked whether or not the referencing block changed.
        let files = [
            (
                "source.py",
                r#"# <block affects="package.json#/versoin">
version = "1.0"
# </block>"#,
            ),
            ("package.json", PACKAGE_JSON),
        ];
        let context = context_with_changed_files(&files, &[])?;

        let violations = validator(&files).validate(context)?.violations;

        let source_violations = &violations[&RepoPath::from_reference("source.py")?];
        assert_eq!(source_violations.len(), 1);
        assert_eq!(
            source_violations[0].message,
            "Block source.py:(unnamed) at line 1 references package.json#/versoin: \
             symbol not found; did you mean: /version"
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

#[cfg(test)]
mod trace_tests {
    use super::validate_tests::{context_with_changed_files, validator};
    use crate::diff_parser::{LineChange, LineChangeKind};
    use crate::test_utils::{validation_context, validation_context_with_changes};
    use crate::validators::SyncValidator;
    use crate::validators::test_utils::{trace_json, with_traces};
    use serde_json::json;

    #[test]
    fn changed_block_records_which_of_its_targets_changed() -> anyhow::Result<()> {
        let source = r#"# <block affects=":changed, :unchanged">
red
# </block>
# <block name="changed">
red
# </block>
# <block name="unchanged">
red
# </block>
"#;
        let context = with_traces(validation_context_with_changes(
            "a.py",
            source,
            vec![
                LineChange {
                    line: 2,
                    kind: LineChangeKind::Added,
                },
                LineChange {
                    line: 5,
                    kind: LineChangeKind::Added,
                },
            ],
        ));

        let report = validator(&[("a.py", source)]).validate(context)?;

        assert_eq!(
            trace_json(&report),
            json!({
                "block_changed": true,
                "targets": [
                    { "changed": true, "target": "a.py:changed" },
                    { "changed": false, "target": "a.py:unchanged" }
                ]
            })
        );
        Ok(())
    }

    #[test]
    fn symbol_and_whole_file_targets_record_whether_the_diff_changed_them() -> anyhow::Result<()> {
        let files = [
            (
                "a.py",
                "# <block affects=\"colors.txt, package.json#/version\">\nred\n# </block>\n",
            ),
            ("colors.txt", "red\n"),
            ("package.json", "{\"version\": \"1.0\"}\n"),
        ];
        let context = with_traces(context_with_changed_files(
            &files,
            &["a.py", "package.json"],
        )?);

        let report = validator(&files).validate(context)?;

        assert_eq!(
            trace_json(&report)["targets"],
            json!([
                { "changed": false, "target": "colors.txt" },
                { "changed": true, "target": "package.json#/version" }
            ])
        );
        Ok(())
    }

    #[test]
    fn targets_that_do_not_resolve_record_why() -> anyhow::Result<()> {
        let source = "# <block affects=\":missing, package.json#/versoin\">\nred\n# </block>\n";
        let files = [
            ("a.py", source),
            ("package.json", "{\"version\": \"1.0\"}\n"),
        ];
        let context = with_traces(context_with_changed_files(&files, &[])?);

        let report = validator(&files).validate(context)?;

        assert_eq!(
            trace_json(&report),
            json!({
                "block_changed": false,
                "targets": [
                    { "reason": "does not exist", "target": "a.py:missing" },
                    {
                        "reason": "symbol not found; did you mean: /version",
                        "target": "package.json#/versoin"
                    }
                ]
            })
        );
        Ok(())
    }

    #[test]
    fn context_without_traces_requested_records_no_trace() -> anyhow::Result<()> {
        let context = validation_context(
            "a.py",
            "# <block affects=\":b\">\nred\n# </block>\n# <block name=\"b\">\nred\n# </block>\n",
        );

        let report = validator(&[]).validate(context)?;

        assert!(report.traces.is_empty());
        Ok(())
    }
}
