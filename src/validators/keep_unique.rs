use crate::blocks::Block;
use crate::validators;
use crate::validators::{Validator, Violation};
use async_trait::async_trait;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

pub(crate) struct KeepUniqueValidator {}

impl KeepUniqueValidator {
    pub(crate) fn new() -> Self {
        Self {}
    }
}

#[derive(Serialize)]
struct KeepUniqueViolation {
    line_number_duplicated: usize,
}

#[async_trait]
impl Validator for KeepUniqueValidator {
    async fn validate(
        &self,
        context: Arc<validators::Context>,
    ) -> anyhow::Result<HashMap<String, Vec<Violation>>> {
        let mut violations = HashMap::new();
        for (file_path, blocks) in &context.modified_blocks {
            for block in blocks {
                if !block.attributes.contains_key("keep-unique") {
                    continue;
                }
                let mut seen: HashSet<&str> = HashSet::new();
                for (idx, line) in block.content.lines().enumerate() {
                    if !seen.insert(line) {
                        let line_no = block.starts_at_line + idx + 1;
                        violations
                            .entry(file_path.clone())
                            .or_insert_with(Vec::new)
                            .push(create_violation(file_path, block, line_no)?);
                        break;
                    }
                }
            }
        }
        Ok(violations)
    }
}

fn create_violation(
    block_file_path: &str,
    block: &Block,
    line_number_duplicated: usize,
) -> anyhow::Result<Violation> {
    let message = format!(
        "Block {}:{} defined at line {} has a duplicated line {}",
        block_file_path,
        block.name_display(),
        block.starts_at_line,
        line_number_duplicated,
    );
    Ok(Violation::new(
        "keep-unique".to_string(),
        message,
        Some(serde_json::to_value(KeepUniqueViolation {
            line_number_duplicated,
        })?),
    ))
}

#[cfg(test)]
mod validate_tests {
    use super::*;
    use crate::blocks::Block;
    use crate::validators::Validator;
    use serde_json::json;

    #[tokio::test]
    async fn empty_blocks_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepUniqueValidator::new();
        let context = Arc::new(validators::Context::new(HashMap::new()));

        let violations = validator.validate(context).await?;

        assert!(violations.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn blocks_with_empty_content_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepUniqueValidator::new();
        let context = Arc::new(validators::Context::new(HashMap::from([(
            "file1".to_string(),
            vec![Block::new(
                1,
                2,
                HashMap::from([("keep-unique".to_string(), "".to_string())]),
                "".to_string(),
            )],
        )])));

        let violations = validator.validate(context).await?;

        assert!(violations.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn all_unique_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepUniqueValidator::new();
        let context = Arc::new(validators::Context::new(HashMap::from([(
            "file1".to_string(),
            vec![Block::new(
                1,
                5,
                HashMap::from([("keep-unique".to_string(), "".to_string())]),
                "A\nB\nC".to_string(),
            )],
        )])));

        let violations = validator.validate(context).await?;

        assert!(violations.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn duplicate_returns_violation_first_dup_line_reported() -> anyhow::Result<()> {
        let validator = KeepUniqueValidator::new();
        let context = Arc::new(validators::Context::new(HashMap::from([(
            "file1".to_string(),
            vec![Block::new(
                1,
                6,
                HashMap::from([("keep-unique".to_string(), "".to_string())]),
                "A\nB\nC\nB\nC".to_string(),
            )],
        )])));

        let violations = validator.validate(context).await?;

        assert_eq!(violations.len(), 1);
        assert_eq!(violations.get("file1").unwrap().len(), 1);
        assert_eq!(
            violations.get("file1").unwrap()[0].error,
            "Block file1:(unnamed) defined at line 1 has a duplicated line 5"
        );
        assert_eq!(violations.get("file1").unwrap()[0].violation, "keep-unique");
        assert_eq!(
            violations.get("file1").unwrap()[0].details,
            Some(json!({
                "line_number_duplicated": 5,
            }))
        );
        Ok(())
    }
}
