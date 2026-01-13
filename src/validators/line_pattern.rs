use crate::blocks::{Block, BlockWithContext};
use crate::validators::{
    ValidatorDetector, ValidatorSync, ValidatorType, Violation, ViolationRange,
};
use crate::{Position, validators};
use anyhow::anyhow;
use regex::Regex;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
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
    ) -> anyhow::Result<HashMap<PathBuf, Vec<Violation>>> {
        let mut violations = HashMap::new();
        for (file_path, file_blocks) in &context.blocks {
            for block_with_context in &file_blocks.blocks_with_context {
                let Some(pattern) = block_with_context.block.attributes.get("line-pattern") else {
                    continue;
                };
                // Compile regex and ensure it anchors to entire line. Users may pass unanchored; we enforce full-line.
                let re = Regex::new(pattern).map_err(|e| {
                    anyhow!(
                        "line-pattern expected a valid regular expression, got \"{}\" in {}:{} at line {} (error: {})",
                        pattern,
                        file_path.display(),
                        block_with_context.block.name_display(),
                        block_with_context.block.start_tag_position_range.start().line,
                        e
                    )
                })?;
                for (line_number, line) in block_with_context
                    .block
                    .content(&file_blocks.file_content)
                    .lines()
                    .enumerate()
                {
                    let trimmed_line = line.trim();
                    if trimmed_line.is_empty() {
                        continue;
                    }
                    if !re.is_match(trimmed_line) {
                        let violation_line_number = block_with_context
                            .block
                            .start_tag_position_range
                            .start()
                            .line
                            + line_number;
                        let line_character_start =
                            trimmed_line.as_ptr() as usize - line.as_ptr() as usize + 1; // Start position is 1-based.
                        let line_character_end = line_character_start + trimmed_line.len() - 1; // End position is 1-based and inclusive.
                        violations
                            .entry(file_path.clone())
                            .or_insert_with(Vec::new)
                            .push(create_violation(
                                file_path,
                                &block_with_context.block,
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
    fn detect(
        &self,
        block_with_context: &BlockWithContext,
    ) -> anyhow::Result<Option<ValidatorType>> {
        if block_with_context
            .block
            .attributes
            .contains_key("line-pattern")
        {
            Ok(Some(ValidatorType::Sync(Box::new(
                LinePatternValidator::new(),
            ))))
        } else {
            Ok(None)
        }
    }
}

fn create_violation(
    block_file_path: &Path,
    block: &Block,
    pattern: &str,
    violation_line_number: usize,
    violation_character_start: usize,
    violation_character_end: usize,
) -> anyhow::Result<Violation> {
    let message = format!(
        "Block {}:{} defined at line {} has a non-matching line {} (pattern: /{}/)",
        block_file_path.display(),
        block.name_display(),
        block.start_tag_position_range.start().line,
        violation_line_number,
        pattern
    );
    Ok(Violation::new(
        ViolationRange::new(
            Position::new(violation_line_number, violation_character_start),
            Position::new(violation_line_number, violation_character_end),
        ),
        "line-pattern".to_string(),
        message,
        block.severity()?,
        Some(serde_json::to_value(LinePatternViolation {
            pattern: pattern.to_string(),
        })?),
    ))
}

#[cfg(test)]
mod validate_tests {
    use super::*;
    use crate::test_utils::validation_context;
    use serde_json::json;

    #[test]
    fn empty_blocks_returns_no_violations() -> anyhow::Result<()> {
        let validator = LinePatternValidator::new();
        let context = validation_context("example.py", "#<block>\n#</block>");
        let violations = validator.validate(context)?;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn blocks_with_empty_content_returns_no_violations() -> anyhow::Result<()> {
        let validator = LinePatternValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block line-pattern="[A-Z]+">
        # </block>"#,
        );
        let violations = validator.validate(context)?;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn valid_regex_all_lines_match_returns_no_violations() -> anyhow::Result<()> {
        let validator = LinePatternValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block line-pattern="^[A-Z]+$">
        FOO
        BAR
        Z
        # </block>"#,
        );
        let violations = validator.validate(context)?;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn empty_lines_and_spaces_are_ignored() -> anyhow::Result<()> {
        let validator = LinePatternValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block line-pattern="^[A-Z]+$">
        FOO
         
        
         BAR 
        Z 
        # </block>"#,
        );

        let violations = validator.validate(context)?;

        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn non_matching_line_reports_first_violation_only() -> anyhow::Result<()> {
        let validator = LinePatternValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block line-pattern="^[A-Z]+$">
        OK
        fail
        NOT OK
        # </block>"#,
        );

        let violations = validator.validate(context)?;

        assert_eq!(violations.len(), 1);
        let file_violations = violations.get(&PathBuf::from("example.py")).unwrap();
        assert_eq!(file_violations.len(), 1);
        assert_eq!(
            file_violations[0].message,
            "Block example.py:(unnamed) defined at line 1 has a non-matching line 3 (pattern: /^[A-Z]+$/)"
        );
        assert_eq!(file_violations[0].code, "line-pattern");
        assert_eq!(
            file_violations[0].range,
            ViolationRange::new(Position::new(3, 9), Position::new(3, 12))
        );
        assert_eq!(
            file_violations[0].data,
            Some(json!({
                "pattern": "^[A-Z]+$"
            }))
        );
        Ok(())
    }

    #[test]
    fn invalid_regex_returns_error() {
        let validator = LinePatternValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block line-pattern="[A-Z+">
        # </block>"#,
        );

        let result = validator.validate(context);

        assert!(result.is_err());
    }
}
