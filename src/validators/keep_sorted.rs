use crate::attributes::{SortFormat, SortOrder};
use crate::blocks::Block;
use crate::position::Position;
use crate::repo_path::RepoPath;
use crate::validators;
use crate::validators::{
    SyncValidator, ValidationReport, Validator, Violation, ViolationRange, line_value,
};
use anyhow::Context;
use serde::Serialize;
use std::cmp::Ordering;

/// Enforces `keep-sorted`: the non-empty lines inside the block must be in ascending order.
///
/// The `keep-sorted` attribute's value optionally supplies a regex selecting the part of each line
/// to compare, so entries can be sorted by a key rather than by the whole line.
pub(super) struct KeepSortedValidator;

#[derive(Serialize)]
struct KeepSortedViolation<'a> {
    order_by: &'a str,
}

impl SyncValidator for KeepSortedValidator {
    fn validate(
        &self,
        context: &validators::ValidationContext,
    ) -> anyhow::Result<ValidationReport> {
        let mut report = ValidationReport::default();
        for (file_path, file_blocks) in &context.blocks {
            for block_with_context in &file_blocks.blocks_with_context {
                let block = &block_with_context.block;
                let Some(order) = block.attributes.keep_sorted() else {
                    continue;
                };
                let (violation, trace) = check_block(
                    file_path,
                    block,
                    &file_blocks.file_content,
                    order,
                    context.traces_requested,
                )?;
                report.add_all(file_path, block, violation.into_iter().collect());
                if let Some(trace) = trace {
                    report.add_trace(file_path, block, &trace)?;
                }
            }
        }

        Ok(report)
    }
}

/// Checks that the non-empty lines of `block` stay ordered by `order`.
///
/// Stops at the first out-of-order line and returns its violation. Includes a trace of the lines
/// read before stopping when `traces_requested` is true.
fn check_block(
    file_path: &RepoPath,
    block: &Block,
    file_content: &str,
    order: SortOrder,
    traces_requested: bool,
) -> anyhow::Result<(Option<Violation>, Option<KeepSortedTrace>)> {
    let re = block.attributes.keep_sorted_pattern();
    let sort_format = block.attributes.keep_sorted_format().unwrap_or_default();
    let violating_ord = match order {
        SortOrder::Asc => Ordering::Greater,
        SortOrder::Desc => Ordering::Less,
    };
    let mut trace = traces_requested.then(|| KeepSortedTrace {
        format: sort_format,
        lines: Vec::new(),
        order,
    });
    let mut prev_value: Option<&str> = None;
    for (line_number, line) in block.content.text(file_content).lines().enumerate() {
        let trimmed = line.trim();
        let value = line_value(line, re);
        if let Some(trace) = &mut trace
            && !trimmed.is_empty()
        {
            trace.lines.push(KeepSortedLine {
                line: block.content.position(line_number, 0).line,
                text: trimmed.to_string(),
                value: value.as_ref().map(|(value, _)| value.to_string()),
            });
        }

        let Some((curr_val, curr_range)) = value else {
            continue;
        };
        if let Some(prev_val) = prev_value {
            let cmp = sort_format
                .compare(prev_val, curr_val)
                .with_context(|| format!("in block {}", block.label(file_path)))?;
            if cmp == violating_ord {
                let violation_start = block.content.position(line_number, curr_range.start);
                let line_character_end = violation_start.character + curr_range.len();
                let violation = create_violation(
                    file_path,
                    block,
                    &order.to_string(),
                    trimmed,
                    violation_start.line,
                    violation_start.character,
                    line_character_end,
                )?;
                return Ok((Some(violation), trace));
            }
        }
        prev_value = Some(curr_val);
    }

    Ok((None, trace))
}

/// What `keep-sorted` saw in one block.
#[derive(Serialize, Debug)]
struct KeepSortedTrace {
    format: SortFormat,
    /// The lines that aren't blank, up to the first one out of order, where the validator stops.
    lines: Vec<KeepSortedLine>,
    order: SortOrder,
}

/// One line that `keep-sorted` read.
#[derive(Serialize, Debug)]
struct KeepSortedLine {
    /// The line in the block's file.
    line: usize,
    /// The line, trimmed.
    text: String,
    /// What is compared: the `value` group of `keep-sorted-pattern`, the whole match, or the
    /// trimmed line. `None` when the pattern skips the line.
    value: Option<String>,
}

fn create_violation(
    block_file_path: &RepoPath,
    block: &Block,
    keep_sorted_value: &str,
    violation_line: &str,
    violation_line_number: usize,
    violation_character_start: usize,
    violation_character_end: usize,
) -> anyhow::Result<Violation> {
    let message = format!(
        "Block {} has an out-of-order line {violation_line_number} ({keep_sorted_value})",
        block.label(block_file_path),
    );
    Ok(Violation::new(
        ViolationRange::new(
            Position::new(violation_line_number, violation_character_start),
            Position::new(violation_line_number, violation_character_end),
        ),
        block_file_path,
        block,
        Validator::KeepSorted,
        message,
        // Identify the violation by the offending line rather than its number, which shifts
        // whenever anything above it is added or removed.
        Some(violation_line),
        Some(serde_json::to_value(KeepSortedViolation {
            order_by: keep_sorted_value,
        })?),
    ))
}

#[cfg(test)]
mod validate_tests {
    use super::*;
    use crate::repo_path::RepoPath;
    use crate::validators::test_utils::validation_context;
    use crate::validators::test_utils::{checked_lines, violation_count};
    use serde_json::json;

    #[test]
    fn out_of_order_line_holding_non_ascii_returns_a_range_measured_in_characters()
    -> anyhow::Result<()> {
        let validator = KeepSortedValidator;
        let context = validation_context(
            "example.py",
            "# <block keep-sorted=\"asc\">\nzebra\ncafé\n# </block>",
        );

        let violations = validator.validate(&context)?.violations;

        let file_violations = violations
            .get(&RepoPath::from_reference("example.py")?)
            .unwrap();
        // `café` is four characters long even though it takes five bytes.
        assert_eq!(
            file_violations[0].range,
            ViolationRange::new(Position::new(3, 1), Position::new(3, 5))
        );
        Ok(())
    }

    #[test]
    fn out_of_order_line_with_non_ascii_before_the_sort_key_returns_a_character_based_range()
    -> anyhow::Result<()> {
        let validator = KeepSortedValidator;
        // The key is matched past a multi-byte character, so its offset within the line differs
        // in bytes and in characters.
        let context = validation_context(
            "example.py",
            "# <block keep-sorted keep-sorted-pattern=\"café (?<value>\\w+)$\">\ncafé zebra\ncafé apple\n# </block>",
        );

        let violations = validator.validate(&context)?.violations;

        let file_violations = violations
            .get(&RepoPath::from_reference("example.py")?)
            .unwrap();
        // `apple` starts at the 6th character of the line and ends at the 10th.
        assert_eq!(
            file_violations[0].range,
            ViolationRange::new(Position::new(3, 6), Position::new(3, 11))
        );
        Ok(())
    }

    #[test]
    fn pattern_matches_within_the_trimmed_line() -> anyhow::Result<()> {
        let validator = KeepSortedValidator;
        // `^` and `$` anchor to the trimmed line, so entries keep matching once they are nested
        // inside a list or a block of code.
        let context = validation_context(
            "example.py",
            "# <block keep-sorted keep-sorted-pattern=\"^ID:(?P<value>\\d+)$\">\n  ID:2\n  ID:1\n# </block>",
        );

        let violations = validator.validate(&context)?.violations;

        let file_violations = violations
            .get(&RepoPath::from_reference("example.py")?)
            .unwrap();
        // The range still points into the original line, indentation included.
        assert_eq!(
            file_violations[0].range,
            ViolationRange::new(Position::new(3, 6), Position::new(3, 7))
        );
        Ok(())
    }

    #[test]
    fn pattern_matching_empty_text_skips_blank_lines() -> anyhow::Result<()> {
        let validator = KeepSortedValidator;
        let context = validation_context(
            "example.py",
            "# <block keep-sorted keep-sorted-pattern=\"(?P<value>.*)\">\nA\n\nB\n\n# </block>",
        );

        let violations = validator.validate(&context)?.violations;

        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn numeric_format_pattern_matching_empty_text_skips_blank_lines() -> anyhow::Result<()> {
        let validator = KeepSortedValidator;
        // A blank line holds no number, so it must be skipped rather than reported as a line whose
        // value cannot be parsed.
        let context = validation_context(
            "example.py",
            "# <block keep-sorted keep-sorted-format=\"numeric\" keep-sorted-pattern=\"(?P<value>\\d*)\">\n1\n\n2\n# </block>",
        );

        let violations = validator.validate(&context)?.violations;

        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn block_out_of_ascending_order_returns_a_violation() -> anyhow::Result<()> {
        let validator = KeepSortedValidator;
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="asc">
        A
        B
        C
        BB
        # </block>"#,
        );

        let violations = validator.validate(&context)?.violations;

        assert_eq!(violations.len(), 1);
        let file_violations = violations
            .get(&RepoPath::from_reference("example.py")?)
            .unwrap();
        assert_eq!(file_violations.len(), 1);
        assert_eq!(
            file_violations[0].message,
            "Block example.py:(unnamed) at line 1 has an out-of-order line 5 (asc)"
        );
        assert_eq!(file_violations[0].code, Validator::KeepSorted);
        assert_eq!(
            file_violations[0].range,
            ViolationRange::new(Position::new(5, 9), Position::new(5, 11))
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
        let validator = KeepSortedValidator;
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="desc">
        D
        C
        D
        C
        # </block>"#,
        );

        let violations = validator.validate(&context)?.violations;

        assert_eq!(violations.len(), 1);
        let file_violations = violations
            .get(&RepoPath::from_reference("example.py")?)
            .unwrap();
        assert_eq!(file_violations.len(), 1);
        assert_eq!(
            file_violations[0].message,
            "Block example.py:(unnamed) at line 1 has an out-of-order line 4 (desc)"
        );
        assert_eq!(file_violations[0].code, Validator::KeepSorted);
        assert_eq!(
            file_violations[0].range,
            ViolationRange::new(Position::new(4, 9), Position::new(4, 10))
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

        let violations = KeepSortedValidator.validate(&context)?.violations;

        let file_violations = violations
            .get(&RepoPath::from_reference("example.rs")?)
            .unwrap();
        // The start tag spans lines 1-2, so the content starts on line 2 and `A` sits on line 4.
        // Anchoring to the start tag instead would name line 3, where `B` is.
        assert_eq!(
            file_violations[0].range,
            ViolationRange::new(Position::new(4, 1), Position::new(4, 2))
        );
        Ok(())
    }

    #[test]
    fn block_sorted_ascending_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator;
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="asc">
        A
        B
        B
        C
        # </block>"#,
        );
        let violations = validator.validate(&context)?.violations;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn block_sorted_descending_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator;
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
        let violations = validator.validate(&context)?.violations;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn empty_keep_sorted_value_returns_an_ascending_order_violation() -> anyhow::Result<()> {
        let validator = KeepSortedValidator;
        let context = validation_context(
            "example.py",
            r"# <block keep-sorted>
        B
        A
        # </block>",
        );
        let violations = validator.validate(&context)?.violations;
        assert_eq!(violations.len(), 1);
        let file_violations = violations
            .get(&RepoPath::from_reference("example.py")?)
            .unwrap();
        assert_eq!(
            file_violations[0].message,
            "Block example.py:(unnamed) at line 1 has an out-of-order line 3 (asc)"
        );
        Ok(())
    }

    #[test]
    fn order_and_format_in_capitals_are_read_like_lowercase() -> anyhow::Result<()> {
        let context = validation_context(
            "example.py",
            "# <block keep-sorted=\"DESC\" keep-sorted-format=\"Numeric\">\n2\n10\n# </block>",
        );

        let report = KeepSortedValidator.validate(&context)?;

        assert_eq!(violation_count(&report), 1);
        Ok(())
    }

    #[test]
    fn block_with_identical_lines_sorted_ascending_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator;
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="asc">
        A
        A
        A
        # </block>"#,
        );
        let violations = validator.validate(&context)?.violations;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn block_with_identical_lines_sorted_descending_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator;
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="desc">
        A
        A
        A
        # </block>"#,
        );
        let violations = validator.validate(&context)?.violations;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn block_with_a_single_line_sorted_ascending_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator;
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="asc">
        Hello
        # </block>"#,
        );
        let violations = validator.validate(&context)?.violations;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn block_with_a_single_line_sorted_descending_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator;
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="desc">
        Hello
        # </block>"#,
        );
        let violations = validator.validate(&context)?.violations;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn block_with_blank_and_whitespace_only_lines_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator;
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="asc">
        A
        
        
         B 
    B 
        C
        # </block>"#,
        );
        let violations = validator.validate(&context)?.violations;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn pattern_with_a_named_group_returns_a_violation_ranged_on_the_group() -> anyhow::Result<()> {
        let validator = KeepSortedValidator;
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="asc" keep-sorted-pattern="id: (?P<value>\d+)">
        B_id_2 = "id: 2"
        A_id_3 = "id: 3"
        C_id_1 = "id: 1"
        # </block>"#,
        );

        let violations = validator.validate(&context)?.violations;

        assert_eq!(violations.len(), 1);
        let file_violations = violations
            .get(&RepoPath::from_reference("example.py")?)
            .unwrap();
        assert_eq!(file_violations.len(), 1);
        assert_eq!(file_violations[0].code, Validator::KeepSorted);
        assert_eq!(
            file_violations[0].range,
            ViolationRange::new(Position::new(4, 23), Position::new(4, 24))
        );
        Ok(())
    }

    #[test]
    fn pattern_without_a_named_group_compares_the_whole_match_and_skips_non_matching_lines()
    -> anyhow::Result<()> {
        let validator = KeepSortedValidator;
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="asc" keep-sorted-pattern="x=\d+">
        x=2
        # ignored
        x=1
        # </block>"#,
        );

        let violations = validator.validate(&context)?.violations;

        assert_eq!(violations.len(), 1);
        let file_violations = violations
            .get(&RepoPath::from_reference("example.py")?)
            .unwrap();
        assert_eq!(file_violations.len(), 1);
        assert_eq!(
            file_violations[0].range,
            ViolationRange::new(Position::new(4, 9), Position::new(4, 12))
        );
        Ok(())
    }

    #[test]
    fn numeric_format_out_of_ascending_order_returns_a_violation() -> anyhow::Result<()> {
        let validator = KeepSortedValidator;
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="asc" keep-sorted-format="numeric">
        2
        20
        10
        # </block>"#,
        );
        let violations = validator.validate(&context)?.violations;
        assert_eq!(violations.len(), 1);
        let file_violations = violations
            .get(&RepoPath::from_reference("example.py")?)
            .unwrap();
        assert_eq!(file_violations.len(), 1);
        assert_eq!(
            file_violations[0].message,
            "Block example.py:(unnamed) at line 1 has an out-of-order line 4 (asc)"
        );
        assert_eq!(file_violations[0].code, Validator::KeepSorted);
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
        let validator = KeepSortedValidator;
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="desc" keep-sorted-format="numeric">
        20
        2
        10
        # </block>"#,
        );
        let violations = validator.validate(&context)?.violations;
        assert_eq!(violations.len(), 1);
        let file_violations = violations
            .get(&RepoPath::from_reference("example.py")?)
            .unwrap();
        assert_eq!(file_violations.len(), 1);
        assert_eq!(
            file_violations[0].message,
            "Block example.py:(unnamed) at line 1 has an out-of-order line 4 (desc)"
        );
        Ok(())
    }

    #[test]
    fn numeric_format_sorted_ascending_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator;
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="asc" keep-sorted-format="numeric">
        2
        10
        20
        # </block>"#,
        );
        let violations = validator.validate(&context)?.violations;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn numeric_format_sorted_descending_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator;
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="desc" keep-sorted-format="numeric">
        20
        10
        2
        # </block>"#,
        );
        let violations = validator.validate(&context)?.violations;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn numeric_format_with_a_pattern_out_of_order_returns_a_violation() -> anyhow::Result<()> {
        let validator = KeepSortedValidator;
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="asc" keep-sorted-format="numeric" keep-sorted-pattern="id: (?P<value>\d+)">
        B_id_2 = "id: 2"
        C_id_10 = "id: 10"
        A_id_3 = "id: 3"
        # </block>"#,
        );
        let violations = validator.validate(&context)?.violations;
        assert_eq!(violations.len(), 1);
        let file_violations = violations
            .get(&RepoPath::from_reference("example.py")?)
            .unwrap();
        assert_eq!(file_violations.len(), 1);
        assert_eq!(
            file_violations[0].message,
            "Block example.py:(unnamed) at line 1 has an out-of-order line 4 (asc)"
        );
        Ok(())
    }

    #[test]
    fn numeric_format_with_a_pattern_sorted_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator;
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="asc" keep-sorted-format="numeric" keep-sorted-pattern="id: (?P<value>\d+)">
        B_id_2 = "id: 2"
        A_id_3 = "id: 3"
        C_id_10 = "id: 10"
        # </block>"#,
        );
        let violations = validator.validate(&context)?.violations;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn numeric_format_with_floats_sorted_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator;
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="asc" keep-sorted-format="numeric">
        1.5
        2.3
        10.1
        # </block>"#,
        );
        let violations = validator.validate(&context)?.violations;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn numeric_format_with_negative_numbers_sorted_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator;
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="asc" keep-sorted-format="numeric">
        -5
        0
        3
        # </block>"#,
        );
        let violations = validator.validate(&context)?.violations;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn numeric_format_with_equal_values_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator;
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="asc" keep-sorted-format="numeric">
        5
        5
        10
        # </block>"#,
        );
        let violations = validator.validate(&context)?.violations;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn numeric_format_with_a_non_numeric_value_returns_an_error() {
        let validator = KeepSortedValidator;
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="asc" keep-sorted-format="numeric">
        2
        abc
        10
        # </block>"#,
        );
        let result = validator.validate(&context);
        assert!(result.is_err());
        let err = format!("{:#}", result.unwrap_err());
        assert!(
            err.contains("is not a valid number"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn numeric_format_with_integers_beyond_float_precision_out_of_order_returns_a_violation()
    -> anyhow::Result<()> {
        let validator = KeepSortedValidator;
        // Both values round to the same floating point number, so comparing them as floats hides
        // the swap.
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="asc" keep-sorted-format="numeric">
        9007199254740993
        9007199254740992
        # </block>"#,
        );
        let violations = validator.validate(&context)?.violations;
        let file_violations = violations
            .get(&RepoPath::from_reference("example.py")?)
            .unwrap();
        assert_eq!(file_violations.len(), 1);
        assert_eq!(
            file_violations[0].message,
            "Block example.py:(unnamed) at line 1 has an out-of-order line 3 (asc)"
        );
        Ok(())
    }

    #[test]
    fn numeric_format_with_values_beyond_float_range_out_of_order_returns_a_violation()
    -> anyhow::Result<()> {
        let validator = KeepSortedValidator;
        // Values this large overflow a floating point number to infinity, which makes every one of
        // them compare equal to the others.
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="asc" keep-sorted-format="numeric">
        1e500
        1e400
        # </block>"#,
        );
        let violations = validator.validate(&context)?.violations;
        let file_violations = violations
            .get(&RepoPath::from_reference("example.py")?)
            .unwrap();
        assert_eq!(file_violations.len(), 1);
        assert_eq!(
            file_violations[0].message,
            "Block example.py:(unnamed) at line 1 has an out-of-order line 3 (asc)"
        );
        Ok(())
    }

    #[test]
    fn numeric_format_treats_negative_zero_as_equal_to_zero() -> anyhow::Result<()> {
        let validator = KeepSortedValidator;
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="asc" keep-sorted-format="numeric">
        0
        -0
        1
        # </block>"#,
        );
        let violations = validator.validate(&context)?.violations;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn numeric_format_with_digit_separators_sorted_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator;
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="asc" keep-sorted-format="numeric">
        999
        1_000
        1_000_000
        # </block>"#,
        );
        let violations = validator.validate(&context)?.violations;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn numeric_format_with_digit_separators_out_of_order_returns_a_violation() -> anyhow::Result<()>
    {
        let validator = KeepSortedValidator;
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="asc" keep-sorted-format="numeric">
        1_000_000
        1_000
        # </block>"#,
        );
        let violations = validator.validate(&context)?.violations;
        let file_violations = violations
            .get(&RepoPath::from_reference("example.py")?)
            .unwrap();
        assert_eq!(file_violations.len(), 1);
        assert_eq!(
            file_violations[0].message,
            "Block example.py:(unnamed) at line 1 has an out-of-order line 3 (asc)"
        );
        Ok(())
    }

    #[test]
    fn numeric_format_with_a_misplaced_digit_separator_returns_an_error() {
        let validator = KeepSortedValidator;
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="asc" keep-sorted-format="numeric">
        1
        1_000_
        # </block>"#,
        );
        let result = validator.validate(&context);
        assert!(result.is_err());
        let err = format!("{:#}", result.unwrap_err());
        assert!(
            err.contains("is not a valid number"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn numeric_format_with_an_infinite_value_returns_an_error() {
        let validator = KeepSortedValidator;
        // Infinity and NaN are float-specific spellings, not numbers a source file can hold.
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="asc" keep-sorted-format="numeric">
        1
        inf
        # </block>"#,
        );
        let result = validator.validate(&context);
        assert!(result.is_err());
        let err = format!("{:#}", result.unwrap_err());
        assert!(
            err.contains("is not a valid number"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn keep_sorted_and_format_with_surrounding_spaces_are_trimmed() -> anyhow::Result<()> {
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted=" desc " keep-sorted-format=" numeric ">
        10
        2
        # </block>"#,
        );
        let violations = KeepSortedValidator.validate(&context)?.violations;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn block_with_empty_content_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator;
        let context = validation_context(
            "example.py",
            r#"# <block keep-sorted="asc">
        # </block>"#,
        );
        let violations = validator.validate(&context)?.violations;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn block_without_a_keep_sorted_attribute_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepSortedValidator;
        let context = validation_context("example.py", "#<block>\n#</block>");
        let violations = validator.validate(&context)?.violations;
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

        let report = KeepSortedValidator.validate(&context)?;

        // The block without a keep-sorted attribute is not checked, so it records nothing.
        assert_eq!(checked_lines(&report), vec![1, 5]);
        assert_eq!(violation_count(&report), 1);
        Ok(())
    }
}

#[cfg(test)]
mod trace_tests {
    use super::*;
    use crate::validators::test_utils::validation_context;
    use crate::validators::test_utils::{trace_json, with_traces};
    use serde_json::json;

    #[test]
    fn block_with_a_pattern_records_the_value_of_each_line_up_to_the_first_out_of_order()
    -> anyhow::Result<()> {
        // `id: 1` is out of order, so the validator stops there and never reads `id: 3`. The blank
        // line is left out.
        let context = with_traces(validation_context(
            "a.py",
            r#"# <block keep-sorted="asc" keep-sorted-pattern="id: (?P<value>\d+)">
id: 2 red

skip me
id: 1 blue
id: 3 green
# </block>
"#,
        ));

        let report = KeepSortedValidator.validate(&context)?;

        assert_eq!(
            trace_json(&report),
            json!({
                "format": "lexicographic",
                "lines": [
                    { "line": 2, "text": "id: 2 red", "value": "2" },
                    { "line": 4, "text": "skip me", "value": null },
                    { "line": 5, "text": "id: 1 blue", "value": "1" }
                ],
                "order": "asc"
            })
        );
        Ok(())
    }

    #[test]
    fn block_without_a_pattern_records_each_trimmed_line_as_its_value() -> anyhow::Result<()> {
        let context = with_traces(validation_context(
            "a.py",
            "# <block keep-sorted=\"desc\" keep-sorted-format=\"numeric\">\n10\n  9\n1\n# </block>\n",
        ));

        let report = KeepSortedValidator.validate(&context)?;

        assert_eq!(
            trace_json(&report),
            json!({
                "format": "numeric",
                "lines": [
                    { "line": 2, "text": "10", "value": "10" },
                    { "line": 3, "text": "9", "value": "9" },
                    { "line": 4, "text": "1", "value": "1" }
                ],
                "order": "desc"
            })
        );
        Ok(())
    }

    #[test]
    fn context_without_traces_requested_records_no_trace() -> anyhow::Result<()> {
        let context = validation_context("a.py", "# <block keep-sorted>\nb\na\n# </block>\n");

        let report = KeepSortedValidator.validate(&context)?;

        assert_eq!(report.traces, []);
        Ok(())
    }
}
