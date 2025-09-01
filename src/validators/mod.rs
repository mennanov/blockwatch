mod affects;
mod keep_sorted;
mod keep_unique;
mod line_count;
mod line_pattern;

use crate::blocks::Block;
use crate::validators::affects::AffectsValidator;
use crate::validators::keep_sorted::KeepSortedValidator;
use crate::validators::keep_unique::KeepUniqueValidator;
use crate::validators::line_count::LineCountValidator;
use crate::validators::line_pattern::LinePatternValidator;
use async_trait::async_trait;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;

/// Validates the given `Context` and returns a list of the violations grouped by filename.
#[async_trait]
pub(crate) trait Validator: Send + Sync {
    async fn validate(
        &self,
        context: Arc<Context>,
    ) -> anyhow::Result<HashMap<String, Vec<Violation>>>;
}

#[derive(Serialize, Debug)]
pub(crate) struct Violation {
    violation: String,
    error: String,
    details: Option<serde_json::Value>,
}

impl Violation {
    pub(crate) fn new(
        name: String,
        error_message: String,
        metadata: Option<serde_json::Value>,
    ) -> Self {
        Self {
            violation: name,
            error: error_message,
            details: metadata,
        }
    }
}

pub(crate) struct Context {
    // Modified blocks grouped by filename.
    modified_blocks: HashMap<String, Vec<Block>>,
}

impl Context {
    pub(crate) fn new(modified_blocks: HashMap<String, Vec<Block>>) -> Self {
        Self { modified_blocks }
    }
}

/// Runs all validators concurrently and returns violations grouped by file paths.
pub(crate) async fn run(context: Context) -> anyhow::Result<HashMap<String, Vec<Violation>>> {
    let validators: Vec<Box<dyn Validator>> = vec![
        // <block affects="README.md:validators-list">
        Box::new(AffectsValidator::new()),
        Box::new(KeepSortedValidator::new()),
        Box::new(KeepUniqueValidator::new()),
        Box::new(LinePatternValidator::new()),
        Box::new(LineCountValidator::new()),
        // </block>
    ];
    let context = Arc::new(context);
    let mut violations = HashMap::new();
    let mut tasks = tokio::task::JoinSet::new();
    for validator in validators {
        let context = Arc::clone(&context);
        tasks.spawn(async move { validator.validate(context).await });
    }

    while let Some(result) = tasks.join_next().await {
        match result {
            Ok(Ok(file_violations)) => {
                for (file_path, file_violations) in file_violations {
                    violations
                        .entry(file_path)
                        .or_insert_with(Vec::new)
                        .extend(file_violations);
                }
            }
            Ok(Err(e)) => return Err(e),
            Err(e) => return Err(anyhow::anyhow!("Failed to run validation: {}", e)),
        }
    }

    Ok(violations)
}

#[cfg(test)]
mod run_tests {
    use crate::blocks::Block;
    use crate::validators;
    use crate::validators::run;
    use std::collections::HashMap;

    #[tokio::test]
    async fn merges_different_violations_correctly() -> anyhow::Result<()> {
        let context = validators::Context::new(HashMap::from([
            (
                "file1".to_string(),
                vec![Block::new(
                    1,
                    6,
                    HashMap::from([
                        ("affects".to_string(), "file2:foo".to_string()),
                        ("keep-sorted".to_string(), "desc".to_string()),
                    ]),
                    "D\nC\nD\nC".to_string(),
                )],
            ),
            (
                "file2".to_string(),
                vec![Block::new(
                    1,
                    6,
                    HashMap::from([("keep-sorted".to_string(), "asc".to_string())]),
                    "A\nB\nA".to_string(),
                )],
            ),
        ]));

        let violations = run(context).await?;

        assert_eq!(violations.len(), 2);
        assert_eq!(violations["file1"].len(), 2);
        let mut file1_violations = violations["file1"]
            .iter()
            .map(|v| v.violation.as_str())
            .collect::<Vec<_>>();
        file1_violations.sort();
        assert_eq!(file1_violations, vec!["affects", "keep-sorted"]);
        assert_eq!(violations["file2"].len(), 1);
        assert_eq!(violations["file2"][0].violation, "keep-sorted");
        Ok(())
    }
}
