use crate::blocks::{Block, BlockWithContext};
use crate::repo_path::RepoPath;
use crate::validators;
use crate::validators::{
    SyncValidator, ValidationReport, Validator, ValidatorTrace, ValidatorType, Violation,
    compile_pattern, regex_value, require_main, trimmed_line_value,
};
use anyhow::{Context, anyhow};
use regex::Regex;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;

/// Enforces `line-count="<N"` and friends: the number of non-empty lines in the block must satisfy
/// the given comparison. With `line-count-pattern`, only the lines that give a value count.
///
/// Useful for keeping a section from silently growing past the size it was designed for — a code
/// sample that must stay readable, or a list with a hard limit.
struct LineCountValidator;

#[derive(Serialize)]
struct LineCountViolation {
    actual: usize,
    op: String,
    expected: usize,
}

impl SyncValidator for LineCountValidator {
    fn validate(
        &self,
        context: Arc<validators::ValidationContext>,
    ) -> anyhow::Result<ValidationReport> {
        let mut report = ValidationReport::default();
        for (file_path, file_blocks) in &context.blocks {
            for block_with_context in &file_blocks.blocks_with_context {
                let block = &block_with_context.block;
                let Some(expr) = block.attributes.get("line-count") else {
                    continue;
                };
                let (op, expected) = parse_constraint(expr).map_err(|e| anyhow!(
                    "line-count expected a comparator like <N, <=N, ==N, >=N, >N; got \"{}\" in {} (error: {})",
                    expr,
                    block.label(file_path),
                    e
                ))?;
                let pattern = compile_count_pattern(file_path, block)?;
                let mut lines = context.traces_requested.then(Vec::new);
                let actual = count_lines(
                    block,
                    &file_blocks.file_content,
                    pattern.as_ref(),
                    lines.as_mut(),
                );
                let violation = (!op.accepts(actual, expected))
                    .then(|| {
                        create_violation(file_path, block, op, expected, actual, pattern.as_ref())
                    })
                    .transpose()?;
                report.add_all(file_path, block, violation.into_iter().collect());
                if let Some(lines) = lines {
                    let trace = ValidatorTrace::LineCount(LineCountTrace {
                        actual,
                        expected,
                        lines,
                        op: op.as_str(),
                    });
                    report.add_trace(file_path, block, trace);
                }
            }
        }
        Ok(report)
    }
}

/// Compiles the `line-count-pattern` of `block`, which is in `file_path`. Returns `None` when the
/// block doesn't have one.
///
/// # Errors
/// Returns an error when the pattern is not a valid regex or has a line break.
fn compile_count_pattern(file_path: &RepoPath, block: &Block) -> anyhow::Result<Option<Regex>> {
    block
        .attributes
        .get("line-count-pattern")
        .map(|pattern| {
            compile_pattern(pattern).with_context(|| {
                format!("invalid line-count-pattern in {}", block.label(file_path))
            })
        })
        .transpose()
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
        let value = match pattern {
            None => trimmed_line_value(line),
            Some(pattern) => regex_value(line, pattern),
        };
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
/// the rule is `operation` and `expected`. `pattern` is the block's `line-count-pattern`, if any.
fn create_violation(
    block_file_path: &RepoPath,
    block: &Block,
    operation: Op,
    expected: usize,
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
        operation.as_str(),
        expected
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
            op: operation.as_str().to_string(),
            expected,
        })?),
    ))
}

/// What `line-count` saw in one block. The keys are those of its violation.
///
/// serde writes the keys in the order the fields are declared, and the output keeps its keys
/// sorted. So declare the fields in alphabetical order.
#[derive(Serialize, Debug)]
pub(crate) struct LineCountTrace {
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

/// Checks that `line-count-pattern` comes with `line-count`.
///
/// # Errors
/// Returns an error that quotes `line-count-pattern` when it comes without `line-count`.
pub(super) fn check_attributes(attributes: &HashMap<String, String>) -> anyhow::Result<()> {
    require_main(attributes, &["line-count"], &["line-count-pattern"])
}

/// Selects [`LineCountValidator`] for blocks carrying a `line-count` attribute.
pub(super) fn detect(block_with_context: &BlockWithContext) -> Option<ValidatorType> {
    block_with_context
        .block
        .attributes
        .contains_key("line-count")
        .then(|| ValidatorType::Sync(Box::new(LineCountValidator)))
}

#[derive(Copy, Clone)]
enum Op {
    Lt,
    Le,
    Eq,
    Ge,
    Gt,
}
impl Op {
    fn as_str(&self) -> &'static str {
        match self {
            Op::Lt => "<",
            Op::Le => "<=",
            Op::Eq => "==",
            Op::Ge => ">=",
            Op::Gt => ">",
        }
    }

    /// Returns whether `actual` satisfies the rule this operator forms with `expected`.
    fn accepts(self, actual: usize, expected: usize) -> bool {
        match self {
            Op::Lt => actual < expected,
            Op::Le => actual <= expected,
            Op::Eq => actual == expected,
            Op::Ge => actual >= expected,
            Op::Gt => actual > expected,
        }
    }
}

fn parse_constraint(s: &str) -> anyhow::Result<(Op, usize)> {
    let trimmed = s.trim();
    let (op, rest) = if let Some(r) = trimmed.strip_prefix("<=") {
        (Op::Le, r)
    } else if let Some(r) = trimmed.strip_prefix(">=") {
        (Op::Ge, r)
    } else if let Some(r) = trimmed.strip_prefix("==") {
        (Op::Eq, r)
    } else if let Some(r) = trimmed.strip_prefix('<') {
        (Op::Lt, r)
    } else if let Some(r) = trimmed.strip_prefix('>') {
        (Op::Gt, r)
    } else {
        return Err(anyhow!("missing comparator"));
    };
    let num_str = rest.trim();
    if num_str.is_empty() {
        return Err(anyhow!("missing number"));
    }
    let expected: usize = num_str.parse().map_err(|_| anyhow!("invalid number"))?;
    Ok((op, expected))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo_path::RepoPath;
    use crate::test_utils::validation_context;
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

        let violations = validator.validate(context)?.violations;

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
        let violations = validator.validate(context)?.violations;
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
        let violations = validator.validate(context)?.violations;
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

        let violations = LineCountValidator.validate(context)?.violations;

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

        let violations = LineCountValidator.validate(context)?.violations;

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

        let violations = LineCountValidator.validate(context)?.violations;

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

        let violations = LineCountValidator.validate(context)?.violations;

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

        let violations = LineCountValidator.validate(context)?.violations;

        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn pattern_with_invalid_regex_returns_an_error() {
        let context = validation_context(
            "example.py",
            r#"# <block line-count="==1" line-count-pattern="(unclosed">
red
# </block>"#,
        );

        let error = LineCountValidator.validate(context).unwrap_err();

        assert!(
            format!("{error:#}")
                .starts_with("invalid line-count-pattern in example.py:(unnamed) at line 1: "),
            "{error}"
        );
    }

    #[test]
    fn pattern_with_a_line_break_returns_an_error() {
        let context = validation_context(
            "example.py",
            "# <block line-count=\"==1\" line-count-pattern=\"^red\n# $\">\nred\n# </block>",
        );

        let error = LineCountValidator.validate(context).unwrap_err();

        assert!(
            format!("{error:#}").contains("the pattern has a line break"),
            "{error}"
        );
    }

    #[test]
    fn parse_constraint_with_invalid_syntax_returns_error() {
        assert!(parse_constraint("50").is_err());
        assert!(parse_constraint("").is_err());
        assert!(parse_constraint("> -1").is_err());
        assert!(parse_constraint("<== 50").is_err());
    }

    #[test]
    fn parse_constraint_with_valid_syntax_returns_correct_result() {
        assert!(matches!(parse_constraint("< 50").unwrap(), (Op::Lt, 50)));
        assert!(matches!(parse_constraint(">=10").unwrap(), (Op::Ge, 10)));
        assert!(matches!(parse_constraint("== 0").unwrap(), (Op::Eq, 0)));
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

        let report = LineCountValidator.validate(context)?;

        // The block without a line-count attribute is not checked, so it records nothing.
        assert_eq!(checked_lines(&report), vec![1, 4]);
        assert_eq!(violation_count(&report), 1);
        Ok(())
    }
}

#[cfg(test)]
mod trace_tests {
    use super::*;
    use crate::test_utils::validation_context;
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

        let report = LineCountValidator.validate(context)?;

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

        let report = LineCountValidator.validate(context)?;

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

        let report = LineCountValidator.validate(context)?;

        assert!(report.traces.is_empty());
        Ok(())
    }
}

#[cfg(test)]
mod check_attributes_tests {
    use crate::validators::Validator;
    use std::collections::HashMap;

    #[test]
    fn pattern_without_line_count_returns_an_error() {
        let attributes = HashMap::from([("line-count-pattern".to_string(), "\\w+".to_string())]);

        let error = Validator::LineCount
            .check_attributes(&attributes)
            .unwrap_err();

        assert_eq!(
            error.to_string(),
            "`line-count-pattern` without `line-count`"
        );
    }
}
