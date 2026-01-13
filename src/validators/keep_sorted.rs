use crate::blocks::{Block, BlockWithContext};
use crate::validators::{
    ValidatorDetector, ValidatorSync, ValidatorType, Violation, ViolationRange,
};
use crate::{Position, validators};
use anyhow::{Context, anyhow};
use serde::Serialize;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(crate) struct KeepSortedValidator {}

impl KeepSortedValidator {
    pub(super) fn new() -> Self {
        Self {}
    }

    fn trimmed_line_value(line: &str) -> Option<(&str, RangeInclusive<usize>)> {
        let trimmed_line = line.trim();
        if trimmed_line.is_empty() {
            None
        } else {
            let start = trimmed_line.as_ptr() as usize - line.as_ptr() as usize + 1;
            let end = start + trimmed_line.len() - 1;
            Some((trimmed_line, start..=end))
        }
    }

    fn regex_value<'a>(
        line: &'a str,
        regex: &regex::Regex,
    ) -> Option<(&'a str, RangeInclusive<usize>)> {
        if let Some(caps) = regex.captures(line) {
            if let Some(m) = caps.name("value") {
                let range = m.range();
                Some((m.as_str(), range.start + 1..=range.end))
            } else if let Some(m) = caps.get(0) {
                let range = m.range();
                Some((m.as_str(), range.start + 1..=range.end))
            } else {
                None
            }
        } else {
            None
        }
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
    ) -> anyhow::Result<HashMap<PathBuf, Vec<Violation>>> {
        let mut violations = HashMap::new();
        for (file_path, file_blocks) in &context.blocks {
            for block_with_context in &file_blocks.blocks_with_context {
                if let Some(keep_sorted) = block_with_context.block.attributes.get("keep-sorted") {
                    let keep_sorted_cleaned = keep_sorted.trim();
                    let keep_sorted_normalized = if keep_sorted_cleaned.is_empty() {
                        "asc".to_string()
                    } else {
                        keep_sorted.to_lowercase()
                    };
                    if keep_sorted_normalized != "asc" && keep_sorted_normalized != "desc" {
                        return Err(anyhow!(
                            "keep-sorted expected values are \"asc\" or \"desc\", got \"{}\" in {}:{} at line {}",
                            keep_sorted,
                            file_path.display(),
                            block_with_context.block.name_display(),
                            block_with_context
                                .block
                                .start_tag_position_range
                                .start()
                                .line
                        ));
                    }
                    // Optional regex pattern similar to keep-unique: if provided, we compare extracted matches.
                    let pattern = block_with_context
                        .block
                        .attributes
                        .get("keep-sorted-pattern")
                        .cloned()
                        .unwrap_or_default();
                    let re = if pattern.is_empty() {
                        None
                    } else {
                        Some(regex::Regex::new(&pattern))
                    };

                    let violating_ord = if keep_sorted_normalized == "asc" {
                        Ordering::Greater
                    } else {
                        Ordering::Less
                    };
                    // Keep previous value and its range for violation location purposes
                    let mut prev_value: Option<(&str, RangeInclusive<usize>)> = None;
                    for (line_number, line) in block_with_context
                        .block
                        .content(&file_blocks.file_content)
                        .lines()
                        .enumerate()
                    {
                        // Determine current comparable value and its character range within the line
                        let value = match &re {
                            None => Self::trimmed_line_value(line),
                            Some(Ok(regex)) => Self::regex_value(line, regex),
                            Some(Err(e)) => {
                                return Err(anyhow!(
                                    "Invalid keep-sorted-pattern expression in block {}:{} defined at line {}: {}",
                                    file_path.display(),
                                    block_with_context.block.name_display(),
                                    block_with_context
                                        .block
                                        .start_tag_position_range
                                        .start()
                                        .line,
                                    e
                                ));
                            }
                        };

                        if let Some((curr_val, curr_range)) = value {
                            if let Some((prev_val, _prev_range)) = &prev_value {
                                let cmp = (*prev_val).cmp(curr_val);
                                if cmp == violating_ord {
                                    let violation_line_number = block_with_context
                                        .block
                                        .start_tag_position_range
                                        .start()
                                        .line
                                        + line_number;
                                    let line_character_start = *curr_range.start();
                                    let line_character_end = *curr_range.end();
                                    violations
                                        .entry(file_path.clone())
                                        .or_insert_with(Vec::new)
                                        .push(create_violation(
                                            file_path,
                                            &block_with_context.block,
                                            keep_sorted_normalized.as_str(),
                                            violation_line_number,
                                            line_character_start,
                                            line_character_end,
                                        )?);
                                    break;
                                }
                            }
                            prev_value = Some((curr_val, curr_range));
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
    block_file_path: &Path,
    block: &Block,
    keep_sorted_value: &str,
    violation_line_number: usize,
    violation_character_start: usize,
    violation_character_end: usize,
) -> anyhow::Result<Violation> {
    let message = format!(
        "Block {}:{} defined at line {} has an out-of-order line {violation_line_number} ({keep_sorted_value})",
        block_file_path.display(),
        block.name_display(),
        block.start_tag_position_range.start().line,
    );
    Ok(Violation::new(
        ViolationRange::new(
            Position::new(violation_line_number, violation_character_start),
            Position::new(violation_line_number, violation_character_end),
        ),
        "keep-sorted".to_string(),
        message,
        block.severity()?,
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
    use crate::test_utils::validation_context;
    use serde_json::json;

    #[test]
    fn empty_blocks_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = validation_context("example.py", "#<block>\n#</block>");
        let violations = validator.validate(context)?;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn blocks_with_empty_content_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="asc">
        # </block>"#,
        );
        let violations = validator.validate(context)?;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn invalid_keep_sorted_value_returns_error() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="invalid">
        # </block>"#,
        );
        let result = validator.validate(context);
        assert!(result.is_err());
        Ok(())
    }

    #[test]
    fn empty_keep_sorted_value_defaults_to_asc() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted>
        B
        A
        # </block>"#,
        );
        let violations = validator.validate(context)?;
        assert_eq!(violations.len(), 1);
        let file_violations = violations.get(&PathBuf::from("example.py")).unwrap();
        assert_eq!(
            file_violations[0].message,
            "Block example.py:(unnamed) defined at line 1 has an out-of-order line 3 (asc)"
        );
        Ok(())
    }

    #[test]
    fn single_line_asc_sorted_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="asc">
        Hello
        # </block>"#,
        );
        let violations = validator.validate(context)?;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn single_line_desc_sorted_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="desc">
        Hello
        # </block>"#,
        );
        let violations = validator.validate(context)?;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn multiple_lines_asc_sorted_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="asc">
        A
        B
        B
        C
        # </block>"#,
        );
        let violations = validator.validate(context)?;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn multiple_lines_desc_sorted_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="desc">
        C
        B
        B
        A
        A
        # </block>"#,
        );
        let violations = validator.validate(context)?;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn empty_lines_and_spaces_are_ignored() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="asc">
        A
        
        
         B 
    B 
        C
        # </block>"#,
        );
        let violations = validator.validate(context)?;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn unsorted_asc_returns_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="asc">
        A
        B
        C
        BB
        # </block>"#,
        );

        let violations = validator.validate(context)?;

        assert_eq!(violations.len(), 1);
        let file_violations = violations.get(&PathBuf::from("example.py")).unwrap();
        assert_eq!(file_violations.len(), 1);
        assert_eq!(
            file_violations[0].message,
            "Block example.py:(unnamed) defined at line 1 has an out-of-order line 5 (asc)"
        );
        assert_eq!(file_violations[0].code, "keep-sorted");
        assert_eq!(
            file_violations[0].range,
            ViolationRange::new(Position::new(5, 9), Position::new(5, 10))
        );
        assert_eq!(
            file_violations[0].data,
            Some(json!({
                "order_by": "asc"
            }))
        );
        Ok(())
    }

    #[test]
    fn unsorted_desc_returns_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="desc">
        D
        C
        D
        C
        # </block>"#,
        );

        let violations = validator.validate(context)?;

        assert_eq!(violations.len(), 1);
        let file_violations = violations.get(&PathBuf::from("example.py")).unwrap();
        assert_eq!(file_violations.len(), 1);
        assert_eq!(
            file_violations[0].message,
            "Block example.py:(unnamed) defined at line 1 has an out-of-order line 4 (desc)"
        );
        assert_eq!(file_violations[0].code, "keep-sorted");
        assert_eq!(
            file_violations[0].range,
            ViolationRange::new(Position::new(4, 9), Position::new(4, 9))
        );
        assert_eq!(
            file_violations[0].data,
            Some(json!({
                "order_by": "desc"
            }))
        );
        Ok(())
    }

    #[test]
    fn identical_lines_asc_sorted_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="asc">
        A
        A
        A
        # </block>"#,
        );
        let violations = validator.validate(context)?;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn identical_lines_desc_sorted_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="desc">
        A
        A
        A
        # </block>"#,
        );
        let violations = validator.validate(context)?;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn regex_with_named_group_detects_out_of_order_and_reports_group_range() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="asc" keep-sorted-pattern="id: (?P<value>\d+)">
        B_id_2 = "id: 2"
        A_id_3 = "id: 3"
        C_id_1 = "id: 1"
        # </block>"#,
        );

        let violations = validator.validate(context)?;

        assert_eq!(violations.len(), 1);
        let file_violations = violations.get(&PathBuf::from("example.py")).unwrap();
        assert_eq!(file_violations.len(), 1);
        assert_eq!(file_violations[0].code, "keep-sorted");
        assert_eq!(
            file_violations[0].range,
            ViolationRange::new(Position::new(4, 23), Position::new(4, 23))
        );
        Ok(())
    }

    #[test]
    fn regex_without_named_group_uses_full_match_and_skips_nonmatching() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="asc" keep-sorted-pattern="x=\d+">
        x=2
        # ignored
        x=1
        # </block>"#,
        );

        let violations = validator.validate(context)?;

        assert_eq!(violations.len(), 1);
        let file_violations = violations.get(&PathBuf::from("example.py")).unwrap();
        assert_eq!(file_violations.len(), 1);
        assert_eq!(
            file_violations[0].range,
            ViolationRange::new(Position::new(4, 9), Position::new(4, 11))
        );
        Ok(())
    }

    #[test]
    fn invalid_pattern_returns_error() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="asc" keep-sorted-pattern="(unclosed">
        item1 = 1
        item2 = 2
        # </block>"#,
        );

        let result = validator.validate(context);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Invalid keep-sorted-pattern expression")
        );

        Ok(())
    }
}
