//! Creates the validators that the blocks of a run turn on, and runs them.

use super::{
    AsyncValidator, SyncValidator, TypedValidator, ValidationContext, ValidationLog, Validator,
};
use crate::fs::FileSystem;
use crate::settings::ValidatorSelection;
use std::sync::Arc;
use strum::IntoEnumIterator;

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
            Err(e) => return Err(anyhow::anyhow!("failed to run validation: {e:?}")),
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
                Err(e) => return Err(anyhow::anyhow!("failed to run validation: {e}")),
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
            .map_err(|e| anyhow::anyhow!("failed to join sync violations thread: {e:?}"))?;
        let mut log = sync_result?;

        let async_result = async_violations_handle
            .join()
            .map_err(|e| anyhow::anyhow!("failed to join async violations thread: {e:?}"))?;
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

/// Counts the blocks in `context` carrying a rule that cannot fire unless a diff is supplied; see
/// [`Validator::needs_diff`]. A validator that `selection` leaves out does not count.
pub(crate) fn diff_gated_block_count(
    context: &ValidationContext,
    selection: &ValidatorSelection,
) -> usize {
    let validators: Vec<Validator> = Validator::iter()
        .filter(|validator| validator.needs_diff() && selection.includes(*validator))
        .collect();
    context
        .blocks
        .values()
        .flat_map(|file_blocks| &file_blocks.blocks_with_context)
        .filter(|block_with_context| {
            validators
                .iter()
                .any(|validator| validator.is_turned_on_by(&block_with_context.block.attributes))
        })
        .count()
}

/// Instantiates exactly the validators the blocks in `context` call for.
///
/// A validator is created at most once, no matter how many blocks use it, and scanning stops as
/// soon as every candidate has been detected. Returning sync and async validators separately lets
/// the caller skip starting a Tokio runtime when no async validator is present.
///
/// Only the validators that `selection` includes are considered.
///
/// `create` instantiates a detected validator, as [`Validator::create`] does. It is a parameter so
/// that a test can use validators that don't call a model or read the environment.
pub(crate) fn detect_validators<Fs: FileSystem + 'static>(
    context: &ValidationContext,
    create: impl Fn(Validator, &Arc<Fs>) -> anyhow::Result<TypedValidator>,
    selection: &ValidatorSelection,
    file_system: &Arc<Fs>,
) -> anyhow::Result<(SyncValidators, AsyncValidators)> {
    let mut undetected: Vec<Validator> = Validator::iter()
        .filter(|validator| selection.includes(*validator))
        .collect();
    let mut sync_validators = Vec::new();
    let mut async_validators = Vec::new();
    'outer: for file_blocks in context.blocks.values() {
        for block in &file_blocks.blocks_with_context {
            let mut still_undetected = Vec::new();
            for validator in undetected {
                if !validator.is_turned_on_by(&block.block.attributes) {
                    still_undetected.push(validator);
                    continue;
                }
                match create(validator, file_system)? {
                    TypedValidator::Sync(sync_validator) => {
                        sync_validators.push((validator, sync_validator));
                    }
                    TypedValidator::Async(async_validator) => {
                        async_validators.push((validator, async_validator));
                    }
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

#[cfg(test)]
mod tests {
    use crate::attributes::Attributes;
    use crate::blocks::{Block, Content, Declaration};
    use crate::fs::FileSystem;
    use crate::fs::test_utils::FakeFileSystem;
    use crate::position::Position;
    use crate::repo_path::RepoPath;
    use crate::settings::ValidatorSelection;
    use crate::validators;
    use crate::validators::test_utils::{merge_validation_contexts, validation_context};
    use crate::validators::{
        AsyncValidator, SyncValidator, TypedValidator, ValidationContext, ValidationReport,
        Validator, Violation, ViolationRange, detect_validators,
    };
    use async_trait::async_trait;
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;

    fn empty_testing_block() -> Block {
        Block {
            attributes: Attributes::default(),
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
                            vec![Violation::new(
                                empty_testing_violation_range(),
                                file_name,
                                &self.testing_block,
                                Validator::CheckAi,
                                "check-ai error message".to_string(),
                                None,
                                None,
                            )],
                        )
                    })
                    .collect(),
                ..ValidationReport::default()
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
                            vec![Violation::new(
                                empty_testing_violation_range(),
                                file_name,
                                &self.testing_block,
                                Validator::KeepSorted,
                                "keep-sorted error message".to_string(),
                                None,
                                None,
                            )],
                        )
                    })
                    .collect(),
                ..ValidationReport::default()
            })
        }
    }

    /// Creates the fakes above in place of `keep-sorted` and `check-ai`.
    fn create_fakes<Fs: FileSystem>(
        validator: Validator,
        _file_system: &Arc<Fs>,
    ) -> anyhow::Result<TypedValidator> {
        Ok(match validator {
            Validator::KeepSorted => TypedValidator::Sync(Box::new(FakeSyncValidator {
                testing_block: Arc::new(empty_testing_block()),
            })),
            Validator::CheckAi => TypedValidator::Async(Box::new(FakeAsyncValidator {
                testing_block: Arc::new(empty_testing_block()),
            })),
            Validator::Affects
            | Validator::CheckLua
            | Validator::KeepUnique
            | Validator::LineCount
            | Validator::LinePattern
            | Validator::SameAs => unreachable!("tests only use keep-sorted and check-ai blocks"),
        })
    }

    #[test]
    fn detect_and_run_with_sync_and_async_validators_returns_correct_violations()
    -> anyhow::Result<()> {
        let context = merge_validation_contexts(vec![
            validation_context(
                "example1.py",
                r#"# <block keep-sorted="asc" check-ai="condition B">
    # </block>"#,
            ),
            validation_context(
                "example2.py",
                r#"# <block keep-sorted="desc" check-ai="condition D">
    # </block>"#,
            ),
        ]);

        let (sync_validators, async_validators) = detect_validators(
            &context,
            create_fakes,
            &ValidatorSelection::All,
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
                r#"# <block keep-sorted="asc">
    # </block>"#,
            ),
            validation_context(
                "example2.py",
                r#"# <block keep-sorted="desc">
    # </block>"#,
            ),
        ]);

        let (sync_validators, async_validators) = detect_validators(
            &context,
            create_fakes,
            &ValidatorSelection::All,
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
            create_fakes,
            &ValidatorSelection::All,
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

    /// Runs the fake validators that `selection` picks on one block that turns on both
    /// `keep-sorted` and `check-ai`. Returns the code of each violation.
    fn violation_codes(selection: ValidatorSelection) -> anyhow::Result<Vec<Validator>> {
        let context = validation_context(
            "example1.py",
            r#"# <block keep-sorted="asc" check-ai="condition B">
    # </block>"#,
        );
        let (sync_validators, async_validators) = detect_validators(
            &context,
            create_fakes,
            &selection,
            &Arc::new(FakeFileSystem::new(HashMap::new())),
        )?;
        let violations = validators::run(context, sync_validators, async_validators)?.violations;
        Ok(violations
            .into_values()
            .flatten()
            .map(|violation| violation.code)
            .collect())
    }

    #[test]
    fn detect_and_run_with_disabled_async_validators_returns_violations_for_sync_validators_only()
    -> anyhow::Result<()> {
        let selection = ValidatorSelection::Skip(HashSet::from([Validator::CheckAi]));
        assert_eq!(violation_codes(selection)?, [Validator::KeepSorted]);
        Ok(())
    }

    #[test]
    fn detect_and_run_with_enabled_async_validators_returns_violations_for_async_validators_only()
    -> anyhow::Result<()> {
        let selection = ValidatorSelection::Only(HashSet::from([Validator::CheckAi]));
        assert_eq!(violation_codes(selection)?, [Validator::CheckAi]);
        Ok(())
    }

    #[test]
    fn detect_and_run_with_disabled_sync_validators_returns_violations_for_async_validators_only()
    -> anyhow::Result<()> {
        let selection = ValidatorSelection::Skip(HashSet::from([Validator::KeepSorted]));
        assert_eq!(violation_codes(selection)?, [Validator::CheckAi]);
        Ok(())
    }

    #[test]
    fn detect_and_run_with_enabled_sync_validators_returns_violations_for_sync_validators_only()
    -> anyhow::Result<()> {
        let selection = ValidatorSelection::Only(HashSet::from([Validator::KeepSorted]));
        assert_eq!(violation_codes(selection)?, [Validator::KeepSorted]);
        Ok(())
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
            Validator::create,
            &ValidatorSelection::All,
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
            validators::diff_gated_block_count(&diff_gated_context(), &ValidatorSelection::All),
            1
        );
    }

    #[test]
    fn diff_gated_block_count_with_the_validator_disabled_returns_zero() {
        assert_eq!(
            validators::diff_gated_block_count(
                &diff_gated_context(),
                &ValidatorSelection::Skip(HashSet::from([Validator::Affects]))
            ),
            0
        );
    }

    #[test]
    fn diff_gated_block_count_with_other_validators_enabled_returns_zero() {
        assert_eq!(
            validators::diff_gated_block_count(
                &diff_gated_context(),
                &ValidatorSelection::Only(HashSet::from([Validator::KeepSorted]))
            ),
            0
        );
    }
}
