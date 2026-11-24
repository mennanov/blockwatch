use crate::attributes::Constraint;
use crate::blocks::Block;
use crate::repo_path::RepoPath;
use crate::validators;
use crate::validators::{SyncValidator, ValidationReport, Validator, Violation, line_value};
use regex::Regex;
use serde::Serialize;

/// Enforces `line-count="<N"` and friends: the number of non-empty lines in the block must satisfy
/// the given comparison. With `line-count-pattern`, only the lines that give a value count.
///
/// Useful for keeping a section from silently growing past the size it was designed for — a code
/// sample that must stay readable, or a list with a hard limit.
pub(super) struct LineCountValidator;

#[derive(Serialize)]
struct LineCountViolation {
    actual: usize,
    op: String,
    expected: usize,
}

impl SyncValidator for LineCountValidator {
    fn validate(
        &self,
        context: &validators::ValidationContext,
    ) -> anyhow::Result<ValidationReport> {
        let mut report = ValidationReport::default();
        for (file_path, file_blocks) in &context.blocks {
            for block_with_context in &file_blocks.blocks_with_context {
                let block = &block_with_context.block;
                let Some(constraint) = block.attributes.line_count() else {
                    continue;
                };
                let pattern = block.attributes.line_count_pattern();
                let mut lines = context.traces_requested.then(Vec::new);
                let actual = count_lines(block, &file_blocks.file_content, pattern, lines.as_mut());
                let violation = (!constraint.accepts(actual))
                    .then(|| create_violation(file_path, block, constraint, actual, pattern))
                    .transpose()?;
                report.add_all(file_path, block, violation.into_iter().collect());
                if let Some(lines) = lines {
                    let trace = LineCountTrace {
                        actual,
                        expected: constraint.expected(),
                        lines,
                        op: constraint.op_str(),
                    };
                    report.add_trace(file_path, block, &trace)?;
                }
            }
        }
        Ok(report)
    }
}

/// Returns how many lines of `block` count: the lines that `pattern` gives a value for, or every
/// line that isn't blank when there is no `pattern`. Adds each line that isn't blank to `trace`,
/// when given.
fn count_lines(
    block: &Block,
    file_content: &str,
    pattern: Option<&Regex>,
    mut trace: Option<&mut Vec<LineCountLine>>,
) -> usize {
    let mut actual = 0;
    for (line_number, line) in block.content.text(file_content).lines().enumerate() {
        let value = line_value(line, pattern);
        if value.is_some() {
            actual += 1;
        }
        if let Some(trace) = &mut trace
            && !line.trim().is_empty()
        {
            trace.push(LineCountLine {
                line: block.content.position(line_number, 0).line,
                text: line.trim().to_string(),
                value: value.map(|(value, _)| value.to_string()),
            });
        }
    }
    actual
}

/// Returns the violation for `block`, which is in `block_file_path`, when `actual` lines count but
/// `constraint` is not met. `pattern` is the block's `line-count-pattern`, if any.
fn create_violation(
    block_file_path: &RepoPath,
    block: &Block,
    constraint: Constraint,
    actual: usize,
    pattern: Option<&Regex>,
) -> anyhow::Result<Violation> {
    let counted = match pattern {
        None => "lines",
        Some(_) => "lines that match line-count-pattern",
    };
    let message = format!(
        "Block {} has {} {}, which does not satisfy {}{}",
        block.label(block_file_path),
        actual,
        counted,
        constraint.op_str(),
        constraint.expected(),
    );
    Ok(Violation::new(
        block.start_tag_position_range.clone().into(),
        block_file_path,
        block,
        Validator::LineCount,
        message,
        // A line count is one fact about the whole block, so there is never a sibling to tell apart.
        None,
        Some(serde_json::to_value(LineCountViolation {
            actual,
            op: constraint.op_str().to_string(),
            expected: constraint.expected(),
        })?),
    ))
}

/// What `line-count` saw in one block. The keys are those of its violation.
#[derive(Serialize, Debug)]
struct LineCountTrace {
    /// The lines of the content that aren't blank, or that `line-count-pattern` gives a value for.
    actual: usize,
    expected: usize,
    /// The lines that aren't blank.
    lines: Vec<LineCountLine>,
    op: &'static str,
}

/// One line that `line-count` read.
#[derive(Serialize, Debug)]
struct LineCountLine {
    /// The line in the block's file.
    line: usize,
    /// The line, trimmed.
    text: String,
    /// The `value` group of `line-count-pattern`, the whole match, or the trimmed line. `None`
    /// when the pattern skips the line, so the line isn't counted.
    value: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo_path::RepoPath;
    use crate::validators::test_utils::validation_context;
    use crate::validators::test_utils::{checked_lines, violation_count};
    use serde_json::json;

    #[test]
    fn validate_with_incorrect_number_of_lines_returns_violations() -> anyhow::Result<()> {
        let validator = LineCountValidator;
        let context = validation_context(
            "example.py",
            r#"# <block line-count="<3">
        a
        b
        c
        # </block>
        # <block line-count="<=3">
        a
        b
        c
        d
        # </block>
        # <block line-count="==3">
        a
        b
        c
        d
        # </block>
        # <block line-count="==3">
        a
        b
        # </block>
        # <block line-count=">=3">
        a
        b
        # </block>
        # <block line-count=">3">
        a
        b
        c
        # </block>"#,
        );

        let violations = validator.validate(&context)?.violations;

        assert_eq!(violations.len(), 1);
        let file2_violations = violations
            .get(&RepoPath::from_reference("example.py")?)
            .unwrap();
        assert_eq!(file2_violations.len(), 6);
        assert_eq!(file2_violations[0].code, Validator::LineCount);
        assert_eq!(
            file2_violations[0].message,
            "Block example.py:(unnamed) at line 1 has 3 lines, which does not satisfy <3"
        );
        assert_eq!(
            file2_violations[0].data,
            Some(json!({
                "actual": 3,
                "op": "<",
                "expected": 3,
            }))
        );

        assert_eq!(file2_violations[1].code, Validator::LineCount);
        assert_eq!(
            file2_violations[1].message,
            "Block example.py:(unnamed) at line 6 has 4 lines, which does not satisfy <=3"
        );
        assert_eq!(
            file2_violations[1].data,
            Some(json!({
                "actual": 4,
                "op": "<=",
                "expected": 3,
            }))
        );

        assert_eq!(file2_violations[2].code, Validator::LineCount);
        assert_eq!(
            file2_violations[2].message,
            "Block example.py:(unnamed) at line 12 has 4 lines, which does not satisfy ==3"
        );
        assert_eq!(
            file2_violations[2].data,
            Some(json!({
                "actual": 4,
                "op": "==",
                "expected": 3,
            }))
        );

        assert_eq!(file2_violations[3].code, Validator::LineCount);
        assert_eq!(
            file2_violations[3].message,
            "Block example.py:(unnamed) at line 18 has 2 lines, which does not satisfy ==3"
        );
        assert_eq!(
            file2_violations[3].data,
            Some(json!({
                "actual": 2,
                "op": "==",
                "expected": 3,
            }))
        );

        assert_eq!(file2_violations[4].code, Validator::LineCount);
        assert_eq!(
            file2_violations[4].message,
            "Block example.py:(unnamed) at line 22 has 2 lines, which does not satisfy >=3"
        );
        assert_eq!(
            file2_violations[4].data,
            Some(json!({
                "actual": 2,
                "op": ">=",
                "expected": 3,
            }))
        );

        assert_eq!(file2_violations[5].code, Validator::LineCount);
        assert_eq!(
            file2_violations[5].message,
            "Block example.py:(unnamed) at line 26 has 3 lines, which does not satisfy >3"
        );
        assert_eq!(
            file2_violations[5].data,
            Some(json!({
                "actual": 3,
                "op": ">",
                "expected": 3,
            }))
        );
        Ok(())
    }

    #[test]
    fn validate_with_correct_number_of_lines_returns_no_violations() -> anyhow::Result<()> {
        let validator = LineCountValidator;
        let context = validation_context(
            "example.py",
            r#"# <block line-count="<3">
        a
        b
        # </block>
        # <block line-count="<=3">
        a
        b
        # </block>
        # <block line-count="<=3">
        a
        b
        c
        # </block>
        # <block line-count="== 2">
        a
        b
        # </block>
        # <block line-count=">= 2">
        a
        b
        # </block>
        # <block line-count=">= 2">
        a
        b
        c
        # </block>
        # <block line-count="> 3">
        a
        b
        c
        d
        # </block>"#,
        );
        let violations = validator.validate(&context)?.violations;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn validate_with_blank_and_whitespace_only_lines_returns_no_violations() -> anyhow::Result<()> {
        let validator = LineCountValidator;
        let context = validation_context(
            "example.py",
            r#"# <block line-count="<=4">
        a
        
        b
        c
         
         
        d
        # </block>"#,
        );
        let violations = validator.validate(&context)?.violations;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn pattern_counts_only_the_lines_it_matches() -> anyhow::Result<()> {
        let context = validation_context(
            "example.py",
            r#"# <block line-count="==2" line-count-pattern='^"\w+",$'>
"red",
# a comment
"blue",
# </block>
# <block line-count="<2" line-count-pattern='^"\w+",$'>
"red",
# a comment
"blue",
# </block>"#,
        );

        let violations = LineCountValidator.validate(&context)?.violations;

        let violations = violations
            .get(&RepoPath::from_reference("example.py")?)
            .unwrap();
        assert_eq!(violations.len(), 1);
        assert_eq!(
            violations[0].message,
            "Block example.py:(unnamed) at line 6 has 2 lines that match line-count-pattern, which does not satisfy <2"
        );
        assert_eq!(
            violations[0].data,
            Some(json!({
                "actual": 2,
                "op": "<",
                "expected": 2,
            }))
        );
        Ok(())
    }

    #[test]
    fn pattern_matching_no_line_counts_zero() -> anyhow::Result<()> {
        let context = validation_context(
            "example.py",
            r#"# <block line-count=">0" line-count-pattern="^id">
red
blue
# </block>"#,
        );

        let violations = LineCountValidator.validate(&context)?.violations;

        let violations = violations
            .get(&RepoPath::from_reference("example.py")?)
            .unwrap();
        assert_eq!(
            violations[0].message,
            "Block example.py:(unnamed) at line 1 has 0 lines that match line-count-pattern, which does not satisfy >0"
        );
        Ok(())
    }

    #[test]
    fn pattern_matches_within_the_trimmed_line() -> anyhow::Result<()> {
        let context = validation_context(
            "example.py",
            r#"# <block line-count="==2" line-count-pattern="^\w+$">
    red
    blue
    red, blue
# </block>"#,
        );

        let violations = LineCountValidator.validate(&context)?.violations;

        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn pattern_with_an_empty_value_skips_the_line() -> anyhow::Result<()> {
        // `red` matches, but its `value` group is empty, so only `1` counts.
        let context = validation_context(
            "example.py",
            r#"# <block line-count="==1" line-count-pattern="(?P<value>\d*)">
red
1
# </block>"#,
        );

        let violations = LineCountValidator.validate(&context)?.violations;

        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn pattern_with_a_value_group_not_in_the_match_skips_the_line() -> anyhow::Result<()> {
        let context = validation_context(
            "example.py",
            r#"# <block line-count="==1" line-count-pattern="^(?:none|(?P<value>\w+))$">
none
red
# </block>"#,
        );

        let violations = LineCountValidator.validate(&context)?.violations;

        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn validate_with_blocks_without_line_count_records_a_check_for_the_examined_ones_only()
    -> anyhow::Result<()> {
        let context = validation_context(
            "example.py",
            r#"# <block name="within" line-count="<=2">
a = 1
# </block>
# <block name="over" line-count="<=1">
a = 1
b = 2
# </block>
# <block name="unrelated">
c = 3
# </block>"#,
        );

        let report = LineCountValidator.validate(&context)?;

        // The block without a line-count attribute is not checked, so it records nothing.
        assert_eq!(checked_lines(&report), vec![1, 4]);
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
    fn block_with_a_pattern_records_the_value_of_each_line() -> anyhow::Result<()> {
        // `# a comment` doesn't match, so it has no value and isn't counted. The blank line is
        // left out.
        let context = with_traces(validation_context(
            "a.py",
            r#"# <block line-count="<=1" line-count-pattern='^"(?P<value>\w+)",$'>
"red",

# a comment
"blue",
# </block>
"#,
        ));

        let report = LineCountValidator.validate(&context)?;

        assert_eq!(
            trace_json(&report),
            json!({
                "actual": 2,
                "expected": 1,
                "lines": [
                    { "line": 2, "text": "\"red\",", "value": "red" },
                    { "line": 4, "text": "# a comment", "value": null },
                    { "line": 5, "text": "\"blue\",", "value": "blue" }
                ],
                "op": "<="
            })
        );
        Ok(())
    }

    #[test]
    fn block_records_the_lines_it_counted_and_the_rule() -> anyhow::Result<()> {
        // The blank line is not counted.
        let context = with_traces(validation_context(
            "a.py",
            "# <block line-count=\"<=1\">\nred\n\n blue\n# </block>\n",
        ));

        let report = LineCountValidator.validate(&context)?;

        assert_eq!(
            trace_json(&report),
            json!({
                "actual": 2,
                "expected": 1,
                "lines": [
                    { "line": 2, "text": "red", "value": "red" },
                    { "line": 4, "text": "blue", "value": "blue" }
                ],
                "op": "<="
            })
        );
        Ok(())
    }

    #[test]
    fn context_without_traces_requested_records_no_trace() -> anyhow::Result<()> {
        let context = validation_context("a.py", "# <block line-count=\"<=1\">\nred\n# </block>\n");

        let report = LineCountValidator.validate(&context)?;

        assert_eq!(report.traces, []);
        Ok(())
    }
}
