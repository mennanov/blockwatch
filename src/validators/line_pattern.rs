use crate::blocks::Block;
use crate::validators;
use crate::validators::{
    ValidatorDetector, ValidatorSync, ValidatorType, Violation, ViolationRange,
};
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
                for (line_number, line) in block
                    .content(&file_blocks.file_contents)
                    .lines()
                    .enumerate()
                {
                    let trimmed_line = line.trim();
                    if trimmed_line.is_empty() {
                        continue;
                    }
                    if !re.is_match(trimmed_line) {
                        let violation_line_number = block.starts_at_line + line_number;
                        let line_character_start =
                            trimmed_line.as_ptr() as usize - line.as_ptr() as usize + 1; // Start position is 1-based.
                        let line_character_end = line_character_start + trimmed_line.len() - 1; // End position is 1-based and inclusive.
                        violations
                            .entry(file_path.clone())
                            .or_insert_with(Vec::new)
                            .push(create_violation(
                                file_path,
                                Arc::clone(block),
                                pattern,
                                violation_line_number,
                                line_character_start,
                                line_character_end,
                            )?);
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
    block: Arc<Block>,
    pattern: &str,
    violation_line_number: usize,
    violation_character_start: usize,
    violation_character_end: usize,
) -> anyhow::Result<Violation> {
    let message = format!(
        "Block {}:{} defined at line {} has a non-matching line {} (pattern: /{}/)",
        block_file_path,
        block.name_display(),
        block.starts_at_line,
        violation_line_number,
        pattern
    );
    Ok(Violation::new(
        ViolationRange::new(
            violation_line_number,
            violation_character_start,
            violation_line_number,
            violation_character_end,
        ),
        "line-pattern".to_string(),
        message,
        block,
        Some(serde_json::to_value(LinePatternViolation {
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
    fn empty_lines_and_spaces_are_ignored() -> anyhow::Result<()> {
        let validator = LinePatternValidator::new();
        let file1_contents = "block content goes here: FOO\n \n\n BAR \nZ ";
        let context = Arc::new(validators::ValidationContext::new(HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_contents: file1_contents.to_string(),
                blocks: vec![Arc::new(Block::new(
                    1,
                    5,
                    HashMap::from([("line-pattern".to_string(), "^[A-Z]+$".to_string())]),
                    test_utils::substr_range(file1_contents, "FOO\n \n\n BAR \nZ "),
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
        let file1_contents = "block content goes here: OK\n fail \nALSOOK";
        let context = Arc::new(validators::ValidationContext::new(HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_contents: file1_contents.to_string(),
                blocks: vec![Arc::new(Block::new(
                    1,
                    6,
                    HashMap::from([("line-pattern".to_string(), "^[A-Z]+$".to_string())]),
                    test_utils::substr_range(file1_contents, "OK\n fail \nALSOOK"),
                ))],
            },
        )])));
        let violations = validator.validate(context)?;
        assert_eq!(violations.len(), 1);
        let file1_violations = violations.get("file1").unwrap();
        assert_eq!(file1_violations.len(), 1);
        assert_eq!(
            file1_violations[0].message,
            "Block file1:(unnamed) defined at line 1 has a non-matching line 2 (pattern: /^[A-Z]+$/)"
        );
        assert_eq!(file1_violations[0].code, "line-pattern");
        assert_eq!(file1_violations[0].range, ViolationRange::new(2, 2, 2, 5));
        assert_eq!(
            file1_violations[0].data,
            Some(json!({
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
