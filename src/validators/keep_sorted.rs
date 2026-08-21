use crate::blocks::{Block, BlockWithContext};
use crate::fs::FileSystem;
use crate::validators::{
    ValidationReport, ValidatorDetector, ValidatorSync, ValidatorType, Violation, ViolationRange,
};
use crate::{Position, validators};
use anyhow::{Context, anyhow};
use serde::Serialize;
use std::cmp::Ordering;
use std::ops::RangeInclusive;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use strum_macros::EnumString;

#[derive(Default, EnumString)]
#[strum(ascii_case_insensitive)]
enum SortFormat {
    #[default]
    Lexicographic,
    Numeric,
}

impl SortFormat {
    fn cmp(&self, a: &str, b: &str) -> anyhow::Result<Ordering> {
        match self {
            Self::Lexicographic => Ok(a.cmp(b)),
            Self::Numeric => {
                let a_num: f64 = a
                    .parse()
                    .map_err(|_| anyhow!("\"{}\" is not a valid number", a))?;
                let b_num: f64 = b
                    .parse()
                    .map_err(|_| anyhow!("\"{}\" is not a valid number", b))?;
                Ok(a_num.total_cmp(&b_num))
            }
        }
    }
}

/// Enforces `keep-sorted`: the non-empty lines inside the block must be in ascending order.
///
/// The `keep-sorted` attribute's value optionally supplies a regex selecting the part of each line
/// to compare, so entries can be sorted by a key rather than by the whole line.
pub(crate) struct KeepSortedValidator {}

impl KeepSortedValidator {
    /// Creates the validator. It is stateless; all input arrives through the validation context.
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
    ) -> anyhow::Result<ValidationReport> {
        let mut report = ValidationReport::default();
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

                    let format_raw = block_with_context
                        .block
                        .attributes
                        .get("keep-sorted-format")
                        .map(|s| s.trim())
                        .unwrap_or_default();
                    let sort_format = if format_raw.is_empty() {
                        SortFormat::default()
                    } else {
                        SortFormat::from_str(format_raw).map_err(|_| {
                            anyhow!(
                                "keep-sorted-format has an unsupported value \"{}\" in {}:{} at line {}",
                                format_raw,
                                file_path.display(),
                                block_with_context.block.name_display(),
                                block_with_context
                                    .block
                                    .start_tag_position_range
                                    .start()
                                    .line
                            )
                        })?
                    };

                    let violating_ord = if keep_sorted_normalized == "asc" {
                        Ordering::Greater
                    } else {
                        Ordering::Less
                    };
                    let mut block_violations = Vec::new();
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
                                let cmp =
                                    sort_format.cmp(prev_val, curr_val).with_context(|| {
                                        format!(
                                            "in block {}:{} defined at line {}",
                                            file_path.display(),
                                            block_with_context.block.name_display(),
                                            block_with_context
                                                .block
                                                .start_tag_position_range
                                                .start()
                                                .line,
                                        )
                                    })?;
                                if cmp == violating_ord {
                                    let violation_start = block_with_context
                                        .block
                                        .content_position(line_number, *curr_range.start() - 1);
                                    let line_character_end = violation_start.character
                                        + (*curr_range.end() - *curr_range.start()); // End position is inclusive.
                                    block_violations.push(create_violation(
                                        file_path,
                                        &block_with_context.block,
                                        keep_sorted_normalized.as_str(),
                                        violation_start.line,
                                        violation_start.character,
                                        line_character_end,
                                    )?);
                                    break;
                                }
                            }
                            prev_value = Some((curr_val, curr_range));
                        }
                    }
                    report.add_all(file_path, &block_with_context.block, block_violations);
                }
            }
        }

        Ok(report)
    }
}

/// Selects [`KeepSortedValidator`] for blocks carrying a `keep-sorted` attribute.
pub(crate) struct KeepSortedValidatorDetector();

impl KeepSortedValidatorDetector {
    /// Creates the detector. Registered in [`crate::validators::detector_factories`].
    pub fn new() -> Self {
        Self {}
    }
}

impl<Fs: FileSystem> ValidatorDetector<Fs> for KeepSortedValidatorDetector {
    fn detect(
        &self,
        block_with_context: &BlockWithContext,
        _file_system: &Arc<Fs>,
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
    use crate::repo_path::RepoPath;
    use crate::test_utils::{checked_lines, validation_context, violation_count};
    use serde_json::json;

    #[test]
    fn block_out_of_ascending_order_returns_a_violation() -> anyhow::Result<()> {
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

        let violations = validator.validate(context)?.violations;

        assert_eq!(violations.len(), 1);
        let file_violations = violations
            .get(&RepoPath::from_reference("example.py")?)
            .unwrap();
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
    fn block_out_of_descending_order_returns_a_violation() -> anyhow::Result<()> {
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

        let violations = validator.validate(context)?.violations;

        assert_eq!(violations.len(), 1);
        let file_violations = violations
            .get(&RepoPath::from_reference("example.py")?)
            .unwrap();
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
    fn block_with_a_multiline_start_tag_returns_a_violation_on_the_out_of_order_line()
    -> anyhow::Result<()> {
        let context = validation_context(
            "example.rs",
            "/* <block\nkeep-sorted=\"asc\"> */\nB\nA\n/* </block> */",
        );

        let violations = KeepSortedValidator::new().validate(context)?.violations;

        let file_violations = violations
            .get(&RepoPath::from_reference("example.rs")?)
            .unwrap();
        // The start tag spans lines 1-2, so the content starts on line 2 and `A` sits on line 4.
        // Anchoring to the start tag instead would name line 3, where `B` is.
        assert_eq!(
            file_violations[0].range,
            ViolationRange::new(Position::new(4, 1), Position::new(4, 1))
        );
        Ok(())
    }

    #[test]
    fn block_sorted_ascending_returns_no_violations() -> anyhow::Result<()> {
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
        let violations = validator.validate(context)?.violations;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn block_sorted_descending_returns_no_violations() -> anyhow::Result<()> {
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
        let violations = validator.validate(context)?.violations;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn empty_keep_sorted_value_returns_an_ascending_order_violation() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted>
        B
        A
        # </block>"#,
        );
        let violations = validator.validate(context)?.violations;
        assert_eq!(violations.len(), 1);
        let file_violations = violations
            .get(&RepoPath::from_reference("example.py")?)
            .unwrap();
        assert_eq!(
            file_violations[0].message,
            "Block example.py:(unnamed) defined at line 1 has an out-of-order line 3 (asc)"
        );
        Ok(())
    }

    #[test]
    fn invalid_keep_sorted_value_returns_an_error() -> anyhow::Result<()> {
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
    fn block_with_identical_lines_sorted_ascending_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="asc">
        A
        A
        A
        # </block>"#,
        );
        let violations = validator.validate(context)?.violations;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn block_with_identical_lines_sorted_descending_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="desc">
        A
        A
        A
        # </block>"#,
        );
        let violations = validator.validate(context)?.violations;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn block_with_a_single_line_sorted_ascending_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="asc">
        Hello
        # </block>"#,
        );
        let violations = validator.validate(context)?.violations;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn block_with_a_single_line_sorted_descending_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="desc">
        Hello
        # </block>"#,
        );
        let violations = validator.validate(context)?.violations;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn block_with_blank_and_whitespace_only_lines_returns_no_violations() -> anyhow::Result<()> {
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
        let violations = validator.validate(context)?.violations;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn pattern_with_a_named_group_returns_a_violation_ranged_on_the_group() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="asc" keep-sorted-pattern="id: (?P<value>\d+)">
        B_id_2 = "id: 2"
        A_id_3 = "id: 3"
        C_id_1 = "id: 1"
        # </block>"#,
        );

        let violations = validator.validate(context)?.violations;

        assert_eq!(violations.len(), 1);
        let file_violations = violations
            .get(&RepoPath::from_reference("example.py")?)
            .unwrap();
        assert_eq!(file_violations.len(), 1);
        assert_eq!(file_violations[0].code, "keep-sorted");
        assert_eq!(
            file_violations[0].range,
            ViolationRange::new(Position::new(4, 23), Position::new(4, 23))
        );
        Ok(())
    }

    #[test]
    fn pattern_without_a_named_group_compares_the_whole_match_and_skips_non_matching_lines()
    -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="asc" keep-sorted-pattern="x=\d+">
        x=2
        # ignored
        x=1
        # </block>"#,
        );

        let violations = validator.validate(context)?.violations;

        assert_eq!(violations.len(), 1);
        let file_violations = violations
            .get(&RepoPath::from_reference("example.py")?)
            .unwrap();
        assert_eq!(file_violations.len(), 1);
        assert_eq!(
            file_violations[0].range,
            ViolationRange::new(Position::new(4, 9), Position::new(4, 11))
        );
        Ok(())
    }

    #[test]
    fn invalid_keep_sorted_pattern_returns_an_error() -> anyhow::Result<()> {
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

    #[test]
    fn numeric_format_out_of_ascending_order_returns_a_violation() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="asc" keep-sorted-format="numeric">
        2
        20
        10
        # </block>"#,
        );
        let violations = validator.validate(context)?.violations;
        assert_eq!(violations.len(), 1);
        let file_violations = violations
            .get(&RepoPath::from_reference("example.py")?)
            .unwrap();
        assert_eq!(file_violations.len(), 1);
        assert_eq!(
            file_violations[0].message,
            "Block example.py:(unnamed) defined at line 1 has an out-of-order line 4 (asc)"
        );
        assert_eq!(file_violations[0].code, "keep-sorted");
        assert_eq!(
            file_violations[0].data,
            Some(json!({
                "order_by": "asc"
            }))
        );
        Ok(())
    }

    #[test]
    fn numeric_format_out_of_descending_order_returns_a_violation() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="desc" keep-sorted-format="numeric">
        20
        2
        10
        # </block>"#,
        );
        let violations = validator.validate(context)?.violations;
        assert_eq!(violations.len(), 1);
        let file_violations = violations
            .get(&RepoPath::from_reference("example.py")?)
            .unwrap();
        assert_eq!(file_violations.len(), 1);
        assert_eq!(
            file_violations[0].message,
            "Block example.py:(unnamed) defined at line 1 has an out-of-order line 4 (desc)"
        );
        Ok(())
    }

    #[test]
    fn numeric_format_sorted_ascending_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="asc" keep-sorted-format="numeric">
        2
        10
        20
        # </block>"#,
        );
        let violations = validator.validate(context)?.violations;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn numeric_format_sorted_descending_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="desc" keep-sorted-format="numeric">
        20
        10
        2
        # </block>"#,
        );
        let violations = validator.validate(context)?.violations;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn numeric_format_with_a_pattern_out_of_order_returns_a_violation() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="asc" keep-sorted-format="numeric" keep-sorted-pattern="id: (?P<value>\d+)">
        B_id_2 = "id: 2"
        C_id_10 = "id: 10"
        A_id_3 = "id: 3"
        # </block>"#,
        );
        let violations = validator.validate(context)?.violations;
        assert_eq!(violations.len(), 1);
        let file_violations = violations
            .get(&RepoPath::from_reference("example.py")?)
            .unwrap();
        assert_eq!(file_violations.len(), 1);
        assert_eq!(
            file_violations[0].message,
            "Block example.py:(unnamed) defined at line 1 has an out-of-order line 4 (asc)"
        );
        Ok(())
    }

    #[test]
    fn numeric_format_with_a_pattern_sorted_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="asc" keep-sorted-format="numeric" keep-sorted-pattern="id: (?P<value>\d+)">
        B_id_2 = "id: 2"
        A_id_3 = "id: 3"
        C_id_10 = "id: 10"
        # </block>"#,
        );
        let violations = validator.validate(context)?.violations;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn numeric_format_with_floats_sorted_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="asc" keep-sorted-format="numeric">
        1.5
        2.3
        10.1
        # </block>"#,
        );
        let violations = validator.validate(context)?.violations;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn numeric_format_with_negative_numbers_sorted_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="asc" keep-sorted-format="numeric">
        -5
        0
        3
        # </block>"#,
        );
        let violations = validator.validate(context)?.violations;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn numeric_format_with_equal_values_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="asc" keep-sorted-format="numeric">
        5
        5
        10
        # </block>"#,
        );
        let violations = validator.validate(context)?.violations;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn numeric_format_with_a_non_numeric_value_returns_an_error() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="asc" keep-sorted-format="numeric">
        2
        abc
        10
        # </block>"#,
        );
        let result = validator.validate(context);
        assert!(result.is_err());
        let err = format!("{:#}", result.unwrap_err());
        assert!(
            err.contains("is not a valid number"),
            "unexpected error: {err}"
        );
        Ok(())
    }

    #[test]
    fn unknown_keep_sorted_format_value_returns_an_error() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="asc" keep-sorted-format="alphabetical">
        a
        b
        # </block>"#,
        );
        let result = validator.validate(context);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("keep-sorted-format has an unsupported value")
        );
        Ok(())
    }

    #[test]
    fn block_with_empty_content_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="asc">
        # </block>"#,
        );
        let violations = validator.validate(context)?.violations;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn block_without_a_keep_sorted_attribute_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator::new();
        let context = validation_context("example.py", "#<block>\n#</block>");
        let violations = validator.validate(context)?.violations;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn blocks_with_and_without_keep_sorted_records_a_check_for_the_examined_ones_only()
    -> anyhow::Result<()> {
        let context = validation_context(
            "example.py",
            r#"# <block name="sorted" keep-sorted="asc">
'apple',
'banana',
# </block>
# <block name="unsorted" keep-sorted="asc">
'banana',
'apple',
# </block>
# <block name="unrelated">
'anything',
# </block>"#,
        );

        let report = KeepSortedValidator::new().validate(context)?;

        // The block without a keep-sorted attribute is not checked, so it records nothing.
        assert_eq!(checked_lines(&report), vec![1, 5]);
        assert_eq!(violation_count(&report), 1);
        Ok(())
    }
}
