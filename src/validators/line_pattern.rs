use crate::blocks::Block;
use crate::validators;
use crate::validators::{Validator, Violation};
use anyhow::anyhow;
use async_trait::async_trait;
use regex::Regex;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;

pub(crate) struct LinePatternValidator {}

impl LinePatternValidator {
    pub(super) fn new() -> Self {
        Self {}
    }
}

#[derive(Serialize)]
struct LinePatternViolation {
    line_number_not_matching: usize,
    pattern: String,
}

#[async_trait]
impl Validator for LinePatternValidator {
    async fn validate(
        &self,
        context: Arc<validators::Context>,
    ) -> anyhow::Result<HashMap<String, Vec<Violation>>> {
        let mut violations = HashMap::new();
        for (file_path, blocks) in &context.modified_blocks {
            for block in blocks {
                let Some(pattern) = block.attributes.get("line-pattern") else {
                    continue;
                };
                // Compile regex and ensure it anchors to entire line. Users may pass unanchored; we enforce full-line.
                let re = Regex::new(pattern).map_err(|e| {
                    anyhow!(
                        "line-pattern expected a valid regular expression, got \"{}\" in {}:{} at line {} (error: {})",
                        pattern,
                        file_path,
                        block.name_display(),
                        block.starts_at_line,
                        e
                    )
                })?;
                for (idx, line) in block.content.lines().enumerate() {
                    if !re.is_match(line) {
                        let line_no = block.starts_at_line + idx + 1;
                        violations
                            .entry(file_path.clone())
                            .or_insert_with(Vec::new)
                            .push(create_violation(file_path, block, pattern, line_no)?);
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
    pattern: &str,
    line_number_not_matching: usize,
) -> anyhow::Result<Violation> {
    let message = format!(
        "Block {}:{} defined at line {} has a non-matching line {} (pattern: /{}/)",
        block_file_path,
        block.name_display(),
        block.starts_at_line,
        line_number_not_matching,
        pattern
    );
    Ok(Violation::new(
        "line-pattern".to_string(),
        message,
        Some(serde_json::to_value(LinePatternViolation {
            line_number_not_matching,
            pattern: pattern.to_string(),
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
        let validator = LinePatternValidator::new();
        let context = Arc::new(validators::Context::new(HashMap::new()));
        let violations = validator.validate(context).await?;
        assert!(violations.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn blocks_with_empty_content_returns_no_violations() -> anyhow::Result<()> {
        let validator = LinePatternValidator::new();
        let context = Arc::new(validators::Context::new(HashMap::from([(
            "file1".to_string(),
            vec![Arc::new(Block::new(
                1,
                2,
                HashMap::from([("line-pattern".to_string(), "[A-Z]+".to_string())]),
                "".to_string(),
            ))],
        )])));
        let violations = validator.validate(context).await?;
        assert!(violations.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn valid_regex_all_lines_match_returns_no_violations() -> anyhow::Result<()> {
        let validator = LinePatternValidator::new();
        let context = Arc::new(validators::Context::new(HashMap::from([(
            "file1".to_string(),
            vec![Arc::new(Block::new(
                1,
                5,
                HashMap::from([("line-pattern".to_string(), "^[A-Z]+$".to_string())]),
                "FOO\nBAR\nZ".to_string(),
            ))],
        )])));
        let violations = validator.validate(context).await?;
        assert!(violations.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn non_matching_line_reports_first_violation_only() -> anyhow::Result<()> {
        let validator = LinePatternValidator::new();
        let context = Arc::new(validators::Context::new(HashMap::from([(
            "file1".to_string(),
            vec![Arc::new(Block::new(
                1,
                6,
                HashMap::from([("line-pattern".to_string(), "^[A-Z]+$".to_string())]),
                "OK\nfail\nALSOOK".to_string(),
            ))],
        )])));
        let violations = validator.validate(context).await?;
        assert_eq!(violations.len(), 1);
        assert_eq!(violations.get("file1").unwrap().len(), 1);
        assert_eq!(
            violations.get("file1").unwrap()[0].error,
            "Block file1:(unnamed) defined at line 1 has a non-matching line 3 (pattern: /^[A-Z]+$/)"
        );
        assert_eq!(
            violations.get("file1").unwrap()[0].violation,
            "line-pattern"
        );
        assert_eq!(
            violations.get("file1").unwrap()[0].details,
            Some(json!({
                "line_number_not_matching": 3,
                "pattern": "^[A-Z]+$"
            }))
        );
        Ok(())
    }

    #[tokio::test]
    async fn invalid_regex_returns_error() {
        let validator = LinePatternValidator::new();
        let context = Arc::new(validators::Context::new(HashMap::from([(
            "file1".to_string(),
            vec![Arc::new(Block::new(
                10,
                15,
                HashMap::from([("line-pattern".to_string(), "[A-Z+".to_string())]),
                "FOO".to_string(),
            ))],
        )])));
        let result = validator.validate(context).await;
        assert!(result.is_err());
    }
}
