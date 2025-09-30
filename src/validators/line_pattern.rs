use crate::blocks::Block;
use crate::validators;
use crate::validators::{ValidatorDetector, ValidatorSync, ValidatorType, Violation};
use anyhow::anyhow;
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

impl ValidatorSync for LinePatternValidator {
    fn validate(
        &self,
        context: Arc<validators::ValidationContext>,
    ) -> anyhow::Result<HashMap<String, Vec<Violation>>> {
        let mut violations = HashMap::new();
        for (file_path, file_blocks) in &context.modified_blocks {
            for block in &file_blocks.blocks {
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
                for (idx, line) in block
                    .content(&file_blocks.file_contents)
                    .lines()
                    .enumerate()
                {
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

pub(crate) struct LinePatternValidatorDetector();

impl LinePatternValidatorDetector {
    pub fn new() -> Self {
        Self {}
    }
}

impl ValidatorDetector for LinePatternValidatorDetector {
    fn detect(&self, block: &Block) -> anyhow::Result<Option<ValidatorType>> {
        if block.attributes.contains_key("line-pattern") {
            Ok(Some(ValidatorType::Sync(Box::new(
                LinePatternValidator::new(),
            ))))
        } else {
            Ok(None)
        }
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
    use crate::blocks::{Block, FileBlocks};
    use crate::test_utils;
    use serde_json::json;

    #[test]
    fn empty_blocks_returns_no_violations() -> anyhow::Result<()> {
        let validator = LinePatternValidator::new();
        let context = Arc::new(validators::ValidationContext::new(HashMap::new()));
        let violations = validator.validate(context)?;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn blocks_with_empty_content_returns_no_violations() -> anyhow::Result<()> {
        let validator = LinePatternValidator::new();
        let context = Arc::new(validators::ValidationContext::new(HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_contents: "".to_string(),
                blocks: vec![Arc::new(Block::new(
                    1,
                    2,
                    HashMap::from([("line-pattern".to_string(), "[A-Z]+".to_string())]),
                    0..0,
                ))],
            },
        )])));
        let violations = validator.validate(context)?;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn valid_regex_all_lines_match_returns_no_violations() -> anyhow::Result<()> {
        let validator = LinePatternValidator::new();
        let file1_contents = "block content goes here: FOO\nBAR\nZ";
        let context = Arc::new(validators::ValidationContext::new(HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_contents: file1_contents.to_string(),
                blocks: vec![Arc::new(Block::new(
                    1,
                    5,
                    HashMap::from([("line-pattern".to_string(), "^[A-Z]+$".to_string())]),
                    test_utils::substr_range(file1_contents, "FOO\nBAR\nZ"),
                ))],
            },
        )])));
        let violations = validator.validate(context)?;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn non_matching_line_reports_first_violation_only() -> anyhow::Result<()> {
        let validator = LinePatternValidator::new();
        let file1_contents = "block content goes here: OK\nfail\nALSOOK";
        let context = Arc::new(validators::ValidationContext::new(HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_contents: file1_contents.to_string(),
                blocks: vec![Arc::new(Block::new(
                    1,
                    6,
                    HashMap::from([("line-pattern".to_string(), "^[A-Z]+$".to_string())]),
                    test_utils::substr_range(file1_contents, "OK\nfail\nALSOOK"),
                ))],
            },
        )])));
        let violations = validator.validate(context)?;
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

    #[test]
    fn invalid_regex_returns_error() {
        let validator = LinePatternValidator::new();
        let context = Arc::new(validators::ValidationContext::new(HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_contents: "".to_string(),
                blocks: vec![Arc::new(Block::new(
                    10,
                    15,
                    HashMap::from([("line-pattern".to_string(), "[A-Z+".to_string())]),
                    0..0,
                ))],
            },
        )])));
        let result = validator.validate(context);
        assert!(result.is_err());
    }
}
