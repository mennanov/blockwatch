use crate::blocks::Block;
use crate::validators;
use crate::validators::{ValidatorDetector, ValidatorSync, ValidatorType, Violation};
use anyhow::{Context, anyhow};
use serde::Serialize;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::Arc;

pub(crate) struct KeepSortedValidator {}

impl KeepSortedValidator {
    pub(super) fn new() -> Self {
        Self {}
    }
}

#[derive(Serialize)]
struct KeepSortedViolation<'a> {
    line_number_out_of_order: usize,
    order_by: &'a str,
}

impl ValidatorSync for KeepSortedValidator {
    fn validate(
        &self,
        context: Arc<validators::ValidationContext>,
    ) -> anyhow::Result<HashMap<String, Vec<Violation>>> {
        let mut violations = HashMap::new();
        for (file_path, file_blocks) in &context.modified_blocks {
            for block in &file_blocks.blocks {
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
                        for (line_number, line) in block
                            .content(&file_blocks.file_contents)
                            .lines()
                            .enumerate()
                        {
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

pub(crate) struct KeepSortedValidatorDetector();

impl KeepSortedValidatorDetector {
    pub fn new() -> Self {
        Self {}
    }
}

impl ValidatorDetector for KeepSortedValidatorDetector {
    fn detect(&self, block: &Block) -> anyhow::Result<Option<ValidatorType>> {
        if block.attributes.contains_key("keep-sorted") {
            Ok(Some(ValidatorType::Sync(Box::new(
                KeepSortedValidator::new(),
            ))))
        } else {
            Ok(None)
        }
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
    use crate::blocks::FileBlocks;
    use crate::test_utils;
    use serde_json::json;
    use std::collections::HashMap;

    #[test]
    fn empty_blocks_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = Arc::new(validators::ValidationContext::new(HashMap::new()));

        let violations = validator.validate(context)?;

        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn blocks_with_empty_content_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = Arc::new(validators::ValidationContext::new(HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_contents: "".to_string(),
                blocks: vec![Arc::new(Block::new(
                    1,
                    2,
                    HashMap::from([("keep-sorted".to_string(), "asc".to_string())]),
                    0..0,
                ))],
            },
        )])));

        let violations = validator.validate(context)?;

        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn invalid_keep_sorted_value_returns_error() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = Arc::new(validators::ValidationContext::new(HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_contents: "".to_string(),
                blocks: vec![Arc::new(Block::new(
                    1,
                    2,
                    HashMap::from([("keep-sorted".to_string(), "invalid".to_string())]),
                    0..0,
                ))],
            },
        )])));

        let result = validator.validate(context);

        assert!(result.is_err());
        Ok(())
    }

    #[test]
    fn single_line_asc_sorted_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let file1_contents = "block contents goes here: Hello";
        let context = Arc::new(validators::ValidationContext::new(HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_contents: file1_contents.to_string(),
                blocks: vec![Arc::new(Block::new(
                    1,
                    3,
                    HashMap::from([("keep-sorted".to_string(), "asc".to_string())]),
                    test_utils::substr_range(file1_contents, "Hello"),
                ))],
            },
        )])));

        let violations = validator.validate(context)?;

        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn single_line_desc_sorted_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let file1_contents = "block contents goes here: Hello";
        let context = Arc::new(validators::ValidationContext::new(HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_contents: file1_contents.to_string(),
                blocks: vec![Arc::new(Block::new(
                    1,
                    3,
                    HashMap::from([("keep-sorted".to_string(), "desc".to_string())]),
                    test_utils::substr_range(file1_contents, "Hello"),
                ))],
            },
        )])));

        let violations = validator.validate(context)?;

        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn multiple_lines_asc_sorted_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let file1_contents = "block contents goes here: A\nB\nB\nC";
        let context = Arc::new(validators::ValidationContext::new(HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_contents: file1_contents.to_string(),
                blocks: vec![Arc::new(Block::new(
                    1,
                    6,
                    HashMap::from([("keep-sorted".to_string(), "asc".to_string())]),
                    test_utils::substr_range(file1_contents, "A\nB\nB\nC"),
                ))],
            },
        )])));

        let violations = validator.validate(context)?;

        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn multiple_lines_desc_sorted_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let file1_contents = "block contents goes here: C\nB\nB\nA\nA";
        let context = Arc::new(validators::ValidationContext::new(HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_contents: file1_contents.to_string(),
                blocks: vec![Arc::new(Block::new(
                    1,
                    7,
                    HashMap::from([("keep-sorted".to_string(), "desc".to_string())]),
                    test_utils::substr_range(file1_contents, "C\nB\nB\nA\nA"),
                ))],
            },
        )])));

        let violations = validator.validate(context)?;

        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn unsorted_asc_returns_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let file1_contents = "block contents goes here: A\nB\nC\nB";
        let context = Arc::new(validators::ValidationContext::new(HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_contents: file1_contents.to_string(),
                blocks: vec![Arc::new(Block::new(
                    1,
                    6,
                    HashMap::from([("keep-sorted".to_string(), "asc".to_string())]),
                    test_utils::substr_range(file1_contents, "A\nB\nC\nB"),
                ))],
            },
        )])));

        let violations = validator.validate(context)?;

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

    #[test]
    fn unsorted_desc_returns_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let file1_contents = "block contents goes here: D\nC\nD\nC";
        let context = Arc::new(validators::ValidationContext::new(HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_contents: file1_contents.to_string(),
                blocks: vec![Arc::new(Block::new(
                    1,
                    6,
                    HashMap::from([("keep-sorted".to_string(), "desc".to_string())]),
                    test_utils::substr_range(file1_contents, "D\nC\nD\nC"),
                ))],
            },
        )])));

        let violations = validator.validate(context)?;

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

    #[test]
    fn identical_lines_asc_sorted_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let file1_contents = "block contents goes here: A\nA\nA";
        let context = Arc::new(validators::ValidationContext::new(HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_contents: file1_contents.to_string(),
                blocks: vec![Arc::new(Block::new(
                    1,
                    5,
                    HashMap::from([("keep-sorted".to_string(), "asc".to_string())]),
                    test_utils::substr_range(file1_contents, "A\nA\nA"),
                ))],
            },
        )])));

        let violations = validator.validate(context)?;

        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn identical_lines_desc_sorted_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let file1_contents = "block contents goes here: A\nA\nA";
        let context = Arc::new(validators::ValidationContext::new(HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_contents: file1_contents.to_string(),
                blocks: vec![Arc::new(Block::new(
                    1,
                    5,
                    HashMap::from([("keep-sorted".to_string(), "desc".to_string())]),
                    test_utils::substr_range(file1_contents, "A\nA\nA"),
                ))],
            },
        )])));

        let violations = validator.validate(context)?;

        assert!(violations.is_empty());
        Ok(())
    }
}
