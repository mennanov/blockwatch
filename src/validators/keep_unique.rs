use crate::blocks::Block;
use crate::validators;
use crate::validators::{Validator, Violation};
use async_trait::async_trait;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

pub(super) struct KeepUniqueValidator {}

impl KeepUniqueValidator {
    /// Creates a validator that ensures lines (or regex matches) within a block are unique.
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
        for (file_path, (file_content, blocks)) in &context.modified_blocks {
            for block in blocks {
                if !block.attributes.contains_key("keep-unique") {
                    continue;
                }
                let pattern = block
                    .attributes
                    .get("keep-unique")
                    .cloned()
                    .unwrap_or_default();
                let re = if pattern.is_empty() {
                    None
                } else {
                    Some(regex::Regex::new(&pattern))
                };
                let mut seen: HashSet<String> = HashSet::new();
                for (idx, line) in block.content(file_content).lines().enumerate() {
                    let key_opt = match &re {
                        None => Some(line.to_string()),
                        Some(Ok(re)) => {
                            if let Some(c) = re.captures(line) {
                                // If named group "value" exists use it, otherwise use whole match
                                if let Some(m) = c.name("value") {
                                    Some(m.as_str().to_string())
                                } else {
                                    c.get(0).map(|m| m.as_str().to_string())
                                }
                            } else {
                                None // skip line when no match
                            }
                        }
                        Some(Err(_)) => {
                            // Invalid regex: return an error for the validator
                            return Err(anyhow::anyhow!(
                                "Invalid keep-unique regex pattern for block {}:{} defined at line {}",
                                file_path,
                                block.name_display(),
                                block.starts_at_line
                            ));
                        }
                    };
                    if let Some(key) = key_opt
                        && !seen.insert(key)
                    {
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
    use crate::test_utils;
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
            (
                "".to_string(),
                vec![Arc::new(Block::new(
                    1,
                    2,
                    HashMap::from([("keep-unique".to_string(), "".to_string())]),
                    0..0,
                ))],
            ),
        )])));

        let violations = validator.validate(context).await?;

        assert!(violations.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn all_unique_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepUniqueValidator::new();
        let file1_contents = "block contents goes here: A\nB\nC";
        let context = Arc::new(validators::Context::new(HashMap::from([(
            "file1".to_string(),
            (
                file1_contents.to_string(),
                vec![Arc::new(Block::new(
                    1,
                    5,
                    HashMap::from([("keep-unique".to_string(), "".to_string())]),
                    test_utils::substr_range(file1_contents, "A\nB\nC"),
                ))],
            ),
        )])));

        let violations = validator.validate(context).await?;

        assert!(violations.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn duplicate_returns_violation_first_dup_line_reported() -> anyhow::Result<()> {
        let validator = KeepUniqueValidator::new();
        let file1_contents = "block contents goes here: A\nB\nC\nB\nC";
        let context = Arc::new(validators::Context::new(HashMap::from([(
            "file1".to_string(),
            (
                file1_contents.to_string(),
                vec![Arc::new(Block::new(
                    1,
                    6,
                    HashMap::from([("keep-unique".to_string(), "".to_string())]),
                    test_utils::substr_range(file1_contents, "A\nB\nC\nB\nC"),
                ))],
            ),
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

    #[tokio::test]
    async fn regex_with_named_group_detects_duplicates() -> anyhow::Result<()> {
        let validator = KeepUniqueValidator::new();
        let attrs = HashMap::from([("keep-unique".to_string(), "^ID:(?P<value>\\d+)".to_string())]);
        let file1_contents = "block contents goes here: ID:1 A\nID:2 B\nID:1 C";
        let context = Arc::new(validators::Context::new(HashMap::from([(
            "file1".to_string(),
            (
                file1_contents.to_string(),
                vec![Arc::new(Block::new(
                    1,
                    6,
                    attrs,
                    test_utils::substr_range(file1_contents, "ID:1 A\nID:2 B\nID:1 C"),
                ))],
            ),
        )])));
        let violations = validator.validate(context).await?;
        assert_eq!(violations.len(), 1);
        assert_eq!(violations.get("file1").unwrap().len(), 1);
        assert_eq!(
            violations.get("file1").unwrap()[0].details,
            Some(json!({"line_number_duplicated": 4}))
        );
        Ok(())
    }

    #[tokio::test]
    async fn regex_without_named_group_uses_full_match() -> anyhow::Result<()> {
        let validator = KeepUniqueValidator::new();
        let attrs = HashMap::from([("keep-unique".to_string(), "^ID:\\d+".to_string())]);
        let file1_contents = "block contents goes here: ID:1 A\nID:2 B\nID:1 C";
        let context = Arc::new(validators::Context::new(HashMap::from([(
            "file1".to_string(),
            (
                file1_contents.to_string(),
                vec![Arc::new(Block::new(
                    1,
                    6,
                    attrs,
                    test_utils::substr_range(file1_contents, "ID:1 A\nID:2 B\nID:1 C"),
                ))],
            ),
        )])));
        let violations = validator.validate(context).await?;
        assert_eq!(violations.len(), 1);
        assert_eq!(violations.get("file1").unwrap().len(), 1);
        assert_eq!(
            violations.get("file1").unwrap()[0].details,
            Some(json!({"line_number_duplicated": 4}))
        );
        Ok(())
    }

    #[tokio::test]
    async fn regex_non_matching_lines_are_skipped() -> anyhow::Result<()> {
        let validator = KeepUniqueValidator::new();
        let attrs = HashMap::from([("keep-unique".to_string(), "^ID:(?P<value>\\d+)".to_string())]);
        let file1_contents = "block contents goes here: ID:1\nX:2\nID:2";
        let context = Arc::new(validators::Context::new(HashMap::from([(
            "file1".to_string(),
            (
                file1_contents.to_string(),
                vec![Arc::new(Block::new(
                    1,
                    4,
                    attrs,
                    test_utils::substr_range(file1_contents, "ID:1\nX:2\nID:2"),
                ))],
            ),
        )])));
        let violations = validator.validate(context).await?;
        assert!(violations.is_empty());
        Ok(())
    }
}
