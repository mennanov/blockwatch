use crate::blocks::{Block, BlockWithContext};
use crate::validators;
use crate::validators::{
    ValidatorDetector, ValidatorSync, ValidatorType, Violation, ViolationRange,
};
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
    order_by: &'a str,
}

impl ValidatorSync for KeepSortedValidator {
    fn validate(
        &self,
        context: Arc<validators::ValidationContext>,
    ) -> anyhow::Result<HashMap<String, Vec<Violation>>> {
        let mut violations = HashMap::new();
        for (file_path, file_blocks) in &context.modified_blocks {
            for block_with_context in &file_blocks.blocks_with_context {
                if let Some(keep_sorted) = block_with_context.block.attributes.get("keep-sorted") {
                    let keep_sorted_normalized = keep_sorted.to_lowercase();
                    if keep_sorted_normalized != "asc" && keep_sorted_normalized != "desc" {
                        return Err(anyhow!(
                            "keep-sorted expected values are \"asc\" or \"desc\", got \"{}\" in {}:{} at line {}",
                            keep_sorted,
                            file_path,
                            block_with_context.block.name_display(),
                            block_with_context.block.starts_at_line
                        ));
                    }
                    let ord = if keep_sorted_normalized == "asc" {
                        Ordering::Greater
                    } else {
                        Ordering::Less
                    };
                    let mut prev_line: Option<&str> = None;
                    for (line_number, line) in block_with_context
                        .block
                        .content(&file_blocks.file_contents)
                        .lines()
                        .enumerate()
                    {
                        let trimmed_line = line.trim();
                        if trimmed_line.is_empty() {
                            continue;
                        }
                        if let Some(prev_line) = prev_line {
                            let cmp = prev_line.cmp(trimmed_line);
                            if cmp == ord {
                                let violation_line_number =
                                    block_with_context.block.starts_at_line + line_number;
                                let line_character_start =
                                    trimmed_line.as_ptr() as usize - line.as_ptr() as usize + 1; // Start position is 1-based.
                                let line_character_end =
                                    line_character_start + trimmed_line.len() - 1; // End position is 1-based and inclusive.
                                violations
                                    .entry(file_path.clone())
                                    .or_insert_with(Vec::new)
                                    .push(create_violation(
                                        file_path,
                                        Arc::clone(&block_with_context.block),
                                        keep_sorted_normalized.as_str(),
                                        violation_line_number,
                                        line_character_start,
                                        line_character_end,
                                    )?);
                                break;
                            }
                        }
                        prev_line = Some(trimmed_line);
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
    fn detect(
        &self,
        block_with_context: &BlockWithContext,
    ) -> anyhow::Result<Option<ValidatorType>> {
        if block_with_context
            .block
            .attributes
            .contains_key("keep-sorted")
        {
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
    block: Arc<Block>,
    keep_sorted_value: &str,
    violation_line_number: usize,
    violation_character_start: usize,
    violation_character_end: usize,
) -> anyhow::Result<Violation> {
    let message = format!(
        "Block {block_file_path}:{} defined at line {} has an out-of-order line {violation_line_number} ({keep_sorted_value})",
        block.name_display(),
        block.starts_at_line,
    );
    Ok(Violation::new(
        ViolationRange::new(
            violation_line_number,
            violation_character_start,
            violation_line_number,
            violation_character_end,
        ),
        "keep-sorted".to_string(),
        message,
        block,
        Some(
            serde_json::to_value(KeepSortedViolation {
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
    use crate::test_utils::block_with_context;
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
                blocks_with_context: vec![block_with_context(Block::new(
                    1,
                    2,
                    HashMap::from([("keep-sorted".to_string(), "asc".to_string())]),
                    0..0,
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
                blocks_with_context: vec![block_with_context(Block::new(
                    1,
                    2,
                    HashMap::from([("keep-sorted".to_string(), "invalid".to_string())]),
                    0..0,
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
        let file1_contents = "/*<block>*/block contents goes here: Hello//</block>";
        let context = Arc::new(validators::ValidationContext::new(HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_contents: file1_contents.to_string(),
                blocks_with_context: vec![block_with_context(Block::new(
                    1,
                    3,
                    HashMap::from([("keep-sorted".to_string(), "asc".to_string())]),
                    test_utils::substr_range(file1_contents, "<block>"),
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
        let file1_contents = "/*<block>*/block contents goes here: Hello//</block>";
        let context = Arc::new(validators::ValidationContext::new(HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_contents: file1_contents.to_string(),
                blocks_with_context: vec![block_with_context(Block::new(
                    1,
                    3,
                    HashMap::from([("keep-sorted".to_string(), "desc".to_string())]),
                    test_utils::substr_range(file1_contents, "<block>"),
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
        let file1_contents = "/*<block>*/block contents goes here: A\nB\nB\nC//</block>";
        let context = Arc::new(validators::ValidationContext::new(HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_contents: file1_contents.to_string(),
                blocks_with_context: vec![block_with_context(Block::new(
                    1,
                    6,
                    HashMap::from([("keep-sorted".to_string(), "asc".to_string())]),
                    test_utils::substr_range(file1_contents, "<block>"),
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
        let file1_contents = "/*<block>*/block contents goes here: C\nB\nB\nA\nA//</block>";
        let context = Arc::new(validators::ValidationContext::new(HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_contents: file1_contents.to_string(),
                blocks_with_context: vec![block_with_context(Block::new(
                    1,
                    7,
                    HashMap::from([("keep-sorted".to_string(), "desc".to_string())]),
                    test_utils::substr_range(file1_contents, "<block>"),
                    test_utils::substr_range(file1_contents, "C\nB\nB\nA\nA"),
                ))],
            },
        )])));

        let violations = validator.validate(context)?;

        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn empty_lines_and_spaces_are_ignored() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let file1_contents = "/*<block>*/block contents goes here: A\n\nB\n B\n C //</block>";
        let context = Arc::new(validators::ValidationContext::new(HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_contents: file1_contents.to_string(),
                blocks_with_context: vec![block_with_context(Block::new(
                    1,
                    6,
                    HashMap::from([("keep-sorted".to_string(), "asc".to_string())]),
                    test_utils::substr_range(file1_contents, "<block>"),
                    test_utils::substr_range(file1_contents, "A\n\nB\n B\n C "),
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
        let file1_contents = "/*<block>*/block contents goes here: A\nB\nC\nBB//</block>";
        let context = Arc::new(validators::ValidationContext::new(HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_contents: file1_contents.to_string(),
                blocks_with_context: vec![block_with_context(Block::new(
                    1,
                    6,
                    HashMap::from([("keep-sorted".to_string(), "asc".to_string())]),
                    test_utils::substr_range(file1_contents, "<block>"),
                    test_utils::substr_range(file1_contents, "A\nB\nC\nBB"),
                ))],
            },
        )])));

        let violations = validator.validate(context)?;

        assert_eq!(violations.len(), 1);
        let file1_violations = violations.get("file1").unwrap();
        assert_eq!(file1_violations.len(), 1);
        assert_eq!(
            file1_violations[0].message,
            "Block file1:(unnamed) defined at line 1 has an out-of-order line 4 (asc)"
        );
        assert_eq!(file1_violations[0].code, "keep-sorted");
        assert_eq!(file1_violations[0].range, ViolationRange::new(4, 1, 4, 2));
        assert_eq!(
            file1_violations[0].data,
            Some(json!({
                "order_by": "asc"
            }))
        );
        Ok(())
    }

    #[test]
    fn unsorted_desc_returns_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let file1_contents = "/*<block>*/block contents goes here: D\nC\nD\nC//</block>";
        let context = Arc::new(validators::ValidationContext::new(HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_contents: file1_contents.to_string(),
                blocks_with_context: vec![block_with_context(Block::new(
                    1,
                    6,
                    HashMap::from([("keep-sorted".to_string(), "desc".to_string())]),
                    test_utils::substr_range(file1_contents, "<block>"),
                    test_utils::substr_range(file1_contents, "D\nC\nD\nC"),
                ))],
            },
        )])));

        let violations = validator.validate(context)?;

        assert_eq!(violations.len(), 1);
        let file1_violations = violations.get("file1").unwrap();
        assert_eq!(file1_violations.len(), 1);
        assert_eq!(
            file1_violations[0].message,
            "Block file1:(unnamed) defined at line 1 has an out-of-order line 3 (desc)"
        );
        assert_eq!(file1_violations[0].code, "keep-sorted",);
        assert_eq!(file1_violations[0].range, ViolationRange::new(3, 1, 3, 1));
        assert_eq!(
            file1_violations[0].data,
            Some(json!({
                "order_by": "desc"
            }))
        );
        Ok(())
    }

    #[test]
    fn identical_lines_asc_sorted_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let file1_contents = "/*<block>*/block contents goes here: A\nA\nA//</block>";
        let context = Arc::new(validators::ValidationContext::new(HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_contents: file1_contents.to_string(),
                blocks_with_context: vec![block_with_context(Block::new(
                    1,
                    5,
                    HashMap::from([("keep-sorted".to_string(), "asc".to_string())]),
                    test_utils::substr_range(file1_contents, "<block>"),
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
        let file1_contents = "/*<block>*/block contents goes here: A\nA\nA//</block>";
        let context = Arc::new(validators::ValidationContext::new(HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_contents: file1_contents.to_string(),
                blocks_with_context: vec![block_with_context(Block::new(
                    1,
                    5,
                    HashMap::from([("keep-sorted".to_string(), "desc".to_string())]),
                    test_utils::substr_range(file1_contents, "<block>"),
                    test_utils::substr_range(file1_contents, "A\nA\nA"),
                ))],
            },
        )])));

        let violations = validator.validate(context)?;

        assert!(violations.is_empty());
        Ok(())
    }
}
