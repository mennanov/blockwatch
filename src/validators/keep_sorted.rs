use crate::blocks::Block;
use crate::validators;
use crate::validators::{Validator, Violation};
use anyhow::{Context, anyhow};
use async_trait::async_trait;
use serde::Serialize;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::Arc;

pub(crate) struct KeepSortedValidator {}

impl KeepSortedValidator {
    pub(crate) fn new() -> Self {
        Self {}
    }
}

#[derive(Serialize)]
struct KeepSortedViolation<'a> {
    line_number_out_of_order: usize,
    order_by: &'a str,
}

#[async_trait]
impl Validator for KeepSortedValidator {
    async fn validate(
        &self,
        context: Arc<validators::Context>,
    ) -> anyhow::Result<HashMap<String, Vec<Violation>>> {
        let mut violations = HashMap::new();
        for (file_path, blocks) in &context.modified_blocks {
            for block in blocks {
                match block.attributes.get("keep-sorted") {
                    None => continue,
                    Some(keep_sorted) => {
                        let keep_sorted_normalized = keep_sorted.to_lowercase();
                        if keep_sorted_normalized != "asc" && keep_sorted_normalized != "desc" {
                            return Err(anyhow!(
                                "keep-sorted expected values are \"asc\" or \"desc\", got \"{}\" in {}:{} at line {}",
                                keep_sorted,
                                file_path,
                                block.name_display(),
                                block.starts_at_line
                            ));
                        }
                        let ord = if keep_sorted_normalized == "asc" {
                            Ordering::Greater
                        } else {
                            Ordering::Less
                        };
                        let mut prev_line: Option<&str> = None;
                        for (line_number, line) in block.content.lines().enumerate() {
                            if let Some(prev_line) = prev_line {
                                let cmp = prev_line.cmp(line);
                                if cmp == ord {
                                    violations
                                        .entry(file_path.clone())
                                        .or_insert_with(Vec::new)
                                        .push(create_violation(
                                            file_path,
                                            block,
                                            keep_sorted_normalized.as_str(),
                                            block.starts_at_line + line_number + 1,
                                        )?);
                                    break;
                                }
                            }
                            prev_line = Some(line);
                        }
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
    keep_sorted_value: &str,
    line_number_out_of_order: usize,
) -> anyhow::Result<Violation> {
    let message = format!(
        "Block {}:{} defined at line {} has an out-of-order line {} ({})",
        block_file_path,
        block.name_display(),
        block.starts_at_line,
        line_number_out_of_order,
        keep_sorted_value
    );
    Ok(Violation::new(
        "keep-sorted".to_string(),
        message,
        Some(
            serde_json::to_value(KeepSortedViolation {
                line_number_out_of_order,
                order_by: keep_sorted_value,
            })
            .context("failed to serialize AffectsViolation block")?,
        ),
    ))
}

#[cfg(test)]
mod validate_tests {
    use super::*;
    use crate::validators::Validator;
    use serde_json::json;
    use std::collections::HashMap;

    #[tokio::test]
    async fn empty_blocks_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = Arc::new(validators::Context::new(HashMap::new()));

        let violations = validator.validate(context).await?;

        assert!(violations.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn blocks_with_empty_content_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = Arc::new(validators::Context::new(HashMap::from([(
            "file1".to_string(),
            vec![Block::new(
                1,
                2,
                HashMap::from([("keep-sorted".to_string(), "asc".to_string())]),
                "".to_string(),
            )],
        )])));

        let violations = validator.validate(context).await?;

        assert!(violations.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn invalid_keep_sorted_value_returns_error() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = Arc::new(validators::Context::new(HashMap::from([(
            "file1".to_string(),
            vec![Block::new(
                1,
                2,
                HashMap::from([("keep-sorted".to_string(), "invalid".to_string())]),
                "".to_string(),
            )],
        )])));

        let result = validator.validate(context).await;

        assert!(result.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn single_line_asc_sorted_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = Arc::new(validators::Context::new(HashMap::from([(
            "file1".to_string(),
            vec![Block::new(
                1,
                3,
                HashMap::from([("keep-sorted".to_string(), "asc".to_string())]),
                "Hello".to_string(),
            )],
        )])));

        let violations = validator.validate(context).await?;

        assert!(violations.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn single_line_desc_sorted_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = Arc::new(validators::Context::new(HashMap::from([(
            "file1".to_string(),
            vec![Block::new(
                1,
                3,
                HashMap::from([("keep-sorted".to_string(), "desc".to_string())]),
                "Hello".to_string(),
            )],
        )])));

        let violations = validator.validate(context).await?;

        assert!(violations.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn multiple_lines_asc_sorted_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = Arc::new(validators::Context::new(HashMap::from([(
            "file1".to_string(),
            vec![Block::new(
                1,
                6,
                HashMap::from([("keep-sorted".to_string(), "asc".to_string())]),
                "A\nB\nB\nC".to_string(),
            )],
        )])));

        let violations = validator.validate(context).await?;

        assert!(violations.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn multiple_lines_desc_sorted_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = Arc::new(validators::Context::new(HashMap::from([(
            "file1".to_string(),
            vec![Block::new(
                1,
                7,
                HashMap::from([("keep-sorted".to_string(), "desc".to_string())]),
                "C\nB\nB\nA\nA".to_string(),
            )],
        )])));

        let violations = validator.validate(context).await?;

        assert!(violations.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn unsorted_asc_returns_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = Arc::new(validators::Context::new(HashMap::from([(
            "file1".to_string(),
            vec![Block::new(
                1,
                6,
                HashMap::from([("keep-sorted".to_string(), "asc".to_string())]),
                "A\nB\nC\nB".to_string(),
            )],
        )])));

        let violations = validator.validate(context).await?;

        assert_eq!(violations.len(), 1);
        assert_eq!(violations.get("file1").unwrap().len(), 1);
        assert_eq!(
            violations.get("file1").unwrap()[0].error,
            "Block file1:(unnamed) defined at line 1 has an out-of-order line 5 (asc)"
        );
        assert_eq!(violations.get("file1").unwrap()[0].violation, "keep-sorted",);
        assert_eq!(
            violations.get("file1").unwrap()[0].details,
            Some(json!({
                "line_number_out_of_order": 5,
                "order_by": "asc"
            }))
        );
        Ok(())
    }

    #[tokio::test]
    async fn unsorted_desc_returns_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = Arc::new(validators::Context::new(HashMap::from([(
            "file1".to_string(),
            vec![Block::new(
                1,
                6,
                HashMap::from([("keep-sorted".to_string(), "desc".to_string())]),
                "D\nC\nD\nC".to_string(),
            )],
        )])));

        let violations = validator.validate(context).await?;

        assert_eq!(violations.len(), 1);
        assert_eq!(violations.get("file1").unwrap().len(), 1);
        assert_eq!(
            violations.get("file1").unwrap()[0].error,
            "Block file1:(unnamed) defined at line 1 has an out-of-order line 4 (desc)"
        );
        assert_eq!(violations.get("file1").unwrap()[0].violation, "keep-sorted",);
        assert_eq!(
            violations.get("file1").unwrap()[0].details,
            Some(json!({
                "line_number_out_of_order": 4,
                "order_by": "desc"
            }))
        );
        Ok(())
    }

    #[tokio::test]
    async fn identical_lines_asc_sorted_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = Arc::new(validators::Context::new(HashMap::from([(
            "file1".to_string(),
            vec![Block::new(
                1,
                5,
                HashMap::from([("keep-sorted".to_string(), "asc".to_string())]),
                "A\nA\nA".to_string(),
            )],
        )])));

        let violations = validator.validate(context).await?;

        assert!(violations.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn identical_lines_desc_sorted_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = Arc::new(validators::Context::new(HashMap::from([(
            "file1".to_string(),
            vec![Block::new(
                1,
                5,
                HashMap::from([("keep-sorted".to_string(), "desc".to_string())]),
                "A\nA\nA".to_string(),
            )],
        )])));

        let violations = validator.validate(context).await?;

        assert!(violations.is_empty());
        Ok(())
    }
}
