use crate::blocks::{Block, BlockWithContext};
use crate::repo_path::RepoPath;
use crate::validators::{
    SyncValidator, ValidationReport, Validator, ValidatorTrace, ValidatorType, Violation,
    ViolationRange, compile_pattern,
};
use crate::{Position, validators};
use anyhow::anyhow;
use regex::Regex;
use serde::Serialize;
use std::ops::Range;
use std::sync::Arc;

/// Enforces `line-pattern="<regex>"`, which every non-empty line in the block must match, and
/// `line-pattern-forbidden="<regex>"`, which no non-empty line may match.
///
/// Keeps hand-maintained lists in shape — a table of `KEY=value` settings, or entries that must
/// all be relative paths — and keeps unwanted text, such as `TODO`, out of a block.
struct LinePatternValidator;

/// Why a line fails. It serializes as the violation's data: `{"pattern": ...}` or
/// `{"forbidden_pattern": ...}`.
#[derive(Serialize)]
#[serde(untagged)]
enum LinePatternViolation<'a> {
    /// The line doesn't match `line-pattern`.
    NonMatching { pattern: &'a str },
    /// The line matches `line-pattern-forbidden`. `text` is the matched part of the line.
    Forbidden {
        forbidden_pattern: &'a str,
        #[serde(skip)]
        text: &'a str,
    },
}

impl SyncValidator for LinePatternValidator {
    fn validate(
        &self,
        context: Arc<validators::ValidationContext>,
    ) -> anyhow::Result<ValidationReport> {
        let mut report = ValidationReport::default();
        for (file_path, file_blocks) in &context.blocks {
            for block_with_context in &file_blocks.blocks_with_context {
                let block = &block_with_context.block;
                let required = compile_attribute(file_path, block, "line-pattern")?;
                let forbidden = compile_attribute(file_path, block, "line-pattern-forbidden")?;
                if required.is_none() && forbidden.is_none() {
                    continue;
                }
                let mut lines = context.traces_requested.then(Vec::new);
                let violation = first_violation(
                    file_path,
                    block,
                    &file_blocks.file_content,
                    required.as_ref(),
                    forbidden.as_ref(),
                    lines.as_mut(),
                )?;
                report.add_all(file_path, block, violation.into_iter().collect());
                if let Some(lines) = lines {
                    let trace = ValidatorTrace::LinePattern(LinePatternTrace { lines });
                    report.add_trace(file_path, block, trace);
                }
            }
        }
        Ok(report)
    }
}

/// Compiles the regex in `attribute` of `block`, which is in `file_path`. Returns the attribute's
/// value with its regex, or `None` when the block doesn't have the attribute.
///
/// # Errors
/// Returns an error when the value is not a valid regex or has a line break.
fn compile_attribute<'b>(
    file_path: &RepoPath,
    block: &'b Block,
    attribute: &str,
) -> anyhow::Result<Option<(&'b str, Regex)>> {
    let Some(pattern) = block.attributes.get(attribute) else {
        return Ok(None);
    };
    let re = compile_pattern(pattern).map_err(|e| {
        anyhow!(
            "{attribute} expected a valid regular expression, got \"{pattern}\" in {} (error: {e})",
            block.label(file_path),
        )
    })?;
    Ok(Some((pattern.as_str(), re)))
}

/// Returns the violation for the first non-empty line of `block` that fails a check, or `None`
/// when every line passes. Each line is trimmed before it is checked. Adds each line it checks to
/// `trace`, when given.
///
/// `required` and `forbidden` are the compiled `line-pattern` and `line-pattern-forbidden`.
fn first_violation(
    file_path: &RepoPath,
    block: &Block,
    file_content: &str,
    required: Option<&(&str, Regex)>,
    forbidden: Option<&(&str, Regex)>,
    mut trace: Option<&mut Vec<LinePatternLine>>,
) -> anyhow::Result<Option<Violation>> {
    for (line_idx, line) in block.content.text(file_content).lines().enumerate() {
        let trimmed_line = line.trim();
        if trimmed_line.is_empty() {
            continue;
        }
        let failure = check_line(trimmed_line, required, forbidden);
        if let Some(lines) = trace.as_deref_mut() {
            let (fails, forbidden_match) = match &failure {
                None => (None, None),
                Some((_, LinePatternViolation::NonMatching { .. })) => (Some("line-pattern"), None),
                Some((_, LinePatternViolation::Forbidden { text, .. })) => {
                    (Some("line-pattern-forbidden"), Some(text.to_string()))
                }
            };
            // The check only asks whether the line matches. The text it matched shows a pattern
            // that is looser than its author meant, such as one without `^` and `$`.
            let matched = required
                .and_then(|(_, re)| re.find(trimmed_line))
                .map(|found| found.as_str().to_string());
            lines.push(LinePatternLine {
                fails,
                forbidden_match,
                line: block.content.position(line_idx, 0).line,
                matched,
                text: trimmed_line.to_string(),
            });
        }
        if let Some((failing_bytes, violation)) = failure {
            let range = source_range(block, line_idx, line, trimmed_line, failing_bytes);
            return create_violation(file_path, block, violation, trimmed_line, range).map(Some);
        }
    }
    Ok(None)
}

/// Returns the first check `line` fails, `required` before `forbidden`: the bytes of `line` to
/// report, and why it fails. Returns `None` when `line` passes both.
fn check_line<'a>(
    line: &'a str,
    required: Option<&'a (&'a str, Regex)>,
    forbidden: Option<&'a (&'a str, Regex)>,
) -> Option<(Range<usize>, LinePatternViolation<'a>)> {
    if let Some((pattern, re)) = required
        && !re.is_match(line)
    {
        return Some((0..line.len(), LinePatternViolation::NonMatching { pattern }));
    }
    if let Some((forbidden_pattern, re)) = forbidden
        && let Some(found) = re.find(line)
    {
        return Some((
            found.range(),
            LinePatternViolation::Forbidden {
                forbidden_pattern,
                text: found.as_str(),
            },
        ));
    }
    None
}

/// Returns where `bytes` of `trimmed_line` are in the source file.
///
/// `line` is line `line_idx` of the block's content, and `trimmed_line` is `line` trimmed.
fn source_range(
    block: &Block,
    line_idx: usize,
    line: &str,
    trimmed_line: &str,
    bytes: Range<usize>,
) -> ViolationRange {
    // Editors count columns in characters, so the bytes before the range, the trimmed
    // indentation included, are turned into a character count.
    let byte_offset = trimmed_line.as_ptr() as usize - line.as_ptr() as usize + bytes.start;
    #[expect(
        clippy::string_slice,
        reason = "`trimmed_line` is in `line`, and `bytes` is a match"
    )]
    let start = block
        .content
        .position(line_idx, line[..byte_offset].chars().count());
    #[expect(
        clippy::string_slice,
        reason = "`bytes` is a regex match in `trimmed_line`"
    )]
    let end = Position::new(
        start.line,
        start.character + trimmed_line[bytes].chars().count(),
    );
    ViolationRange::new(start, end)
}

/// What `line-pattern` saw in one block.
#[derive(Serialize, Debug)]
pub(crate) struct LinePatternTrace {
    /// The lines that aren't blank, up to the first one that fails, where the validator stops.
    lines: Vec<LinePatternLine>,
}

/// One line that `line-pattern` checked.
///
/// serde writes the keys in the order the fields are declared, and the output keeps its keys
/// sorted. So declare the fields in alphabetical order.
#[derive(Serialize, Debug)]
struct LinePatternLine {
    /// The attribute whose check the line fails. `None` when the line passes.
    #[serde(skip_serializing_if = "Option::is_none")]
    fails: Option<&'static str>,
    /// The text that `line-pattern-forbidden` found. `None` for any other line.
    #[serde(skip_serializing_if = "Option::is_none")]
    forbidden_match: Option<String>,
    /// The line in the block's file.
    line: usize,
    /// The text that `line-pattern` matched. `None` when the line doesn't match it, or the block
    /// has no `line-pattern`.
    #[serde(rename = "match", skip_serializing_if = "Option::is_none")]
    matched: Option<String>,
    /// The line, trimmed.
    text: String,
}

/// Selects [`LinePatternValidator`] for blocks carrying a `line-pattern` or a
/// `line-pattern-forbidden` attribute.
pub(super) fn detect(block_with_context: &BlockWithContext) -> Option<ValidatorType> {
    let attributes = &block_with_context.block.attributes;
    (attributes.contains_key("line-pattern") || attributes.contains_key("line-pattern-forbidden"))
        .then(|| ValidatorType::Sync(Box::new(LinePatternValidator)))
}

/// Builds the violation of `block`, which is in `block_file_path`, for `violation_line`: the
/// trimmed line that failed. `range` is where the failing text is in the source file.
fn create_violation(
    block_file_path: &RepoPath,
    block: &Block,
    violation: LinePatternViolation,
    violation_line: &str,
    range: ViolationRange,
) -> anyhow::Result<Violation> {
    let line_number = range.start().line;
    let failure = match &violation {
        LinePatternViolation::NonMatching { pattern } => {
            format!("a non-matching line {line_number} (pattern: /{pattern}/)")
        }
        LinePatternViolation::Forbidden {
            forbidden_pattern,
            text,
        } => format!(
            "the forbidden text \"{text}\" on line {line_number} (pattern: /{forbidden_pattern}/)"
        ),
    };
    let message = format!("Block {} has {failure}", block.label(block_file_path));
    Ok(Violation::new(
        range,
        block_file_path,
        block,
        Validator::LinePattern,
        message,
        // Identify the violation by the offending line rather than its number, which shifts
        // whenever anything above it is added or removed.
        Some(violation_line),
        Some(serde_json::to_value(violation)?),
    ))
}

#[cfg(test)]
mod validate_tests {
    use super::*;
    use crate::repo_path::RepoPath;
    use crate::test_utils::validation_context;
    use crate::validators::test_utils::{checked_lines, violation_count};
    use serde_json::json;

    #[test]
    fn block_with_several_non_matching_lines_returns_the_first_violation_only() -> anyhow::Result<()>
    {
        let validator = LinePatternValidator;
        let context = validation_context(
            "example.py",
            r#"# <block line-pattern="^[A-Z]+$">
        OK
        fail
        NOT OK
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
            "Block example.py:(unnamed) at line 1 has a non-matching line 3 (pattern: /^[A-Z]+$/)"
        );
        assert_eq!(file_violations[0].code, Validator::LinePattern);
        assert_eq!(
            file_violations[0].range,
            ViolationRange::new(Position::new(3, 9), Position::new(3, 13))
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
    fn non_matching_line_holding_non_ascii_returns_a_range_measured_in_characters()
    -> anyhow::Result<()> {
        let context = validation_context(
            "example.py",
            "# <block line-pattern=\"^[A-Z]+$\">\ncaféx\n# </block>",
        );

        let violations = LinePatternValidator.validate(context)?.violations;

        let file_violations = violations
            .get(&RepoPath::from_reference("example.py")?)
            .unwrap();
        // `caféx` is five characters long even though it takes six bytes.
        assert_eq!(
            file_violations[0].range,
            ViolationRange::new(Position::new(2, 1), Position::new(2, 6))
        );
        Ok(())
    }

    #[test]
    fn block_with_every_line_matching_returns_no_violations() -> anyhow::Result<()> {
        let validator = LinePatternValidator;
        let context = validation_context(
            "example.py",
            r#"# <block line-pattern="^[A-Z]+$">
        FOO
        BAR
        Z
        # </block>"#,
        );
        let violations = validator.validate(context)?.violations;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn block_with_several_forbidden_matches_returns_a_violation_at_the_first_match()
    -> anyhow::Result<()> {
        let context = validation_context(
            "example.py",
            r#"# <block line-pattern-forbidden="red">
blue = 1
café = "red"
green = "red"
# </block>"#,
        );

        let violations = LinePatternValidator.validate(context)?.violations;

        let file_violations = violations
            .get(&RepoPath::from_reference("example.py")?)
            .unwrap();
        assert_eq!(file_violations.len(), 1);
        assert_eq!(
            file_violations[0].message,
            r#"Block example.py:(unnamed) at line 1 has the forbidden text "red" on line 3 (pattern: /red/)"#
        );
        assert_eq!(file_violations[0].code, Validator::LinePattern);
        // `red` starts at character 9, but at byte 10, because `é` takes two bytes.
        assert_eq!(
            file_violations[0].range,
            ViolationRange::new(Position::new(3, 9), Position::new(3, 12))
        );
        assert_eq!(
            file_violations[0].data,
            Some(json!({
                "forbidden_pattern": "red"
            }))
        );
        Ok(())
    }

    #[test]
    fn block_without_a_forbidden_match_returns_no_violations() -> anyhow::Result<()> {
        let context = validation_context(
            "example.py",
            r#"# <block line-pattern-forbidden="red">
blue = 1
# </block>"#,
        );

        let violations = LinePatternValidator.validate(context)?.violations;

        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn blocks_with_both_patterns_return_the_first_line_that_fails_either() -> anyhow::Result<()> {
        let context = validation_context(
            "example.py",
            r#"# <block name="non-matching" line-pattern="^[a-z]+$" line-pattern-forbidden="red">
BLUE
# </block>
# <block name="forbidden" line-pattern="^[a-z]+$" line-pattern-forbidden="red">
red
# </block>"#,
        );

        let violations = LinePatternValidator.validate(context)?.violations;

        let messages: Vec<&str> = violations
            .get(&RepoPath::from_reference("example.py")?)
            .unwrap()
            .iter()
            .map(|violation| violation.message.as_str())
            .collect();
        assert_eq!(
            messages,
            vec![
                "Block example.py:non-matching at line 1 has a non-matching line 2 (pattern: /^[a-z]+$/)",
                r#"Block example.py:forbidden at line 4 has the forbidden text "red" on line 5 (pattern: /red/)"#,
            ]
        );
        Ok(())
    }

    #[test]
    fn inline_block_with_a_non_matching_line_returns_violation_with_source_line_columns()
    -> anyhow::Result<()> {
        let context = validation_context(
            "example.rs",
            r#"const X: &str = /* <block line-pattern="^GOOD$"> */ "BAD" /* </block> */;"#,
        );

        let violations = LinePatternValidator.validate(context)?.violations;

        let file_violations = violations
            .get(&RepoPath::from_reference("example.rs")?)
            .unwrap();
        // `"BAD"` sits at columns 53..=57 of the source line; the block's content starts at
        // column 52, so a range measured from the content alone points at `const` instead.
        assert_eq!(
            file_violations[0].range,
            ViolationRange::new(Position::new(1, 53), Position::new(1, 58))
        );
        Ok(())
    }

    #[test]
    fn block_with_a_multiline_start_tag_returns_violation_with_line_the_content_starts_on()
    -> anyhow::Result<()> {
        let context = validation_context(
            "example.rs",
            "/* <block\nline-pattern=\"^GOOD$\"> */ \"BAD\" /* </block> */",
        );

        let violations = LinePatternValidator.validate(context)?.violations;

        let file_violations = violations
            .get(&RepoPath::from_reference("example.rs")?)
            .unwrap();
        // The start tag opens on line 1 but the content only begins on line 2, after the tag's
        // comment closes.
        assert_eq!(
            file_violations[0].range,
            ViolationRange::new(Position::new(2, 27), Position::new(2, 32))
        );
        Ok(())
    }

    #[test]
    fn block_with_blank_and_whitespace_only_lines_returns_no_violations() -> anyhow::Result<()> {
        let validator = LinePatternValidator;
        let context = validation_context(
            "example.py",
            r#"# <block line-pattern="^[A-Z]+$">
        FOO
         
        
         BAR 
        Z 
        # </block>"#,
        );

        let violations = validator.validate(context)?.violations;

        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn block_with_an_invalid_regex_returns_an_error() {
        let validator = LinePatternValidator;
        let context = validation_context(
            "example.py",
            r#"# <block line-pattern="[A-Z+">
        # </block>"#,
        );

        let result = validator.validate(context);

        assert!(result.is_err());
    }

    #[test]
    fn block_with_an_invalid_forbidden_regex_returns_an_error() {
        let context = validation_context(
            "example.py",
            r#"# <block line-pattern-forbidden="[a-z+">
# </block>"#,
        );

        let err = LinePatternValidator.validate(context).unwrap_err();

        assert!(
            err.to_string()
                .starts_with("line-pattern-forbidden expected a valid regular expression"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn block_with_a_line_break_in_the_pattern_returns_an_error() {
        let validator = LinePatternValidator;
        let context = validation_context(
            "example.py",
            "# <block line-pattern=\"^[a-z]+\n# $\">\nabc\n# </block>",
        );

        let err = validator.validate(context).unwrap_err();
        assert!(
            err.to_string().contains("line break"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn block_without_a_line_pattern_attribute_returns_no_violations() -> anyhow::Result<()> {
        let validator = LinePatternValidator;
        let context = validation_context("example.py", "#<block>\n#</block>");
        let violations = validator.validate(context)?.violations;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn block_with_empty_content_returns_no_violations() -> anyhow::Result<()> {
        let validator = LinePatternValidator;
        let context = validation_context(
            "example.py",
            r#"# <block line-pattern="[A-Z]+">
        # </block>"#,
        );
        let violations = validator.validate(context)?.violations;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn blocks_with_and_without_a_line_pattern_records_a_check_for_the_examined_ones_only()
    -> anyhow::Result<()> {
        let context = validation_context(
            "example.py",
            r#"# <block name="matching" line-pattern="^[a-z]+$">
apple
# </block>
# <block name="failing" line-pattern="^[a-z]+$">
APPLE
# </block>
# <block name="unrelated">
anything
# </block>"#,
        );

        let report = LinePatternValidator.validate(context)?;

        // The block without a line-pattern attribute is not checked, so it records nothing.
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
    fn block_records_the_lines_up_to_the_first_that_fails() -> anyhow::Result<()> {
        // The validator stops at `blue`, so it never reads `- green`. It skips the blank line.
        let context = with_traces(validation_context(
            "a.py",
            "# <block line-pattern=\"^- \">\n- red\n\nblue\n- green\n# </block>\n",
        ));

        let report = LinePatternValidator.validate(context)?;

        assert_eq!(
            trace_json(&report),
            json!({
                "lines": [
                    { "line": 2, "match": "- ", "text": "- red" },
                    { "fails": "line-pattern", "line": 4, "text": "blue" }
                ]
            })
        );
        Ok(())
    }

    #[test]
    fn line_with_forbidden_text_records_the_text_of_both_patterns() -> anyhow::Result<()> {
        let context = with_traces(validation_context(
            "a.py",
            "# <block line-pattern=\"^- \" line-pattern-forbidden=\"TODO\">\n- TODO red\n# </block>\n",
        ));

        let report = LinePatternValidator.validate(context)?;

        assert_eq!(
            trace_json(&report),
            json!({
                "lines": [
                    {
                        "fails": "line-pattern-forbidden",
                        "forbidden_match": "TODO",
                        "line": 2,
                        "match": "- ",
                        "text": "- TODO red"
                    }
                ]
            })
        );
        Ok(())
    }

    #[test]
    fn context_without_traces_requested_records_no_trace() -> anyhow::Result<()> {
        let context = validation_context("a.py", "# <block line-pattern=\"^a$\">\nb\n# </block>\n");

        let report = LinePatternValidator.validate(context)?;

        assert!(report.traces.is_empty());
        Ok(())
    }
}
