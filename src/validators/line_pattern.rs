use crate::blocks::{Block, BlockWithContext};
use crate::character_column_at;
use crate::fs::FileSystem;
use crate::validators::{
    ValidationReport, ValidatorDetector, ValidatorSync, ValidatorType, Violation, ViolationRange,
};
use crate::{Position, validators};
use anyhow::anyhow;
use regex::Regex;
use serde::Serialize;
use std::path::Path;
use std::sync::Arc;

/// Enforces `line-pattern="<regex>"`: every non-empty line in the block must match the regex.
///
/// Keeps hand-maintained lists in shape — a table of `KEY=value` settings, or entries that must
/// all be relative paths.
pub(crate) struct LinePatternValidator {}

impl LinePatternValidator {
    /// Creates the validator. It is stateless; all input arrives through the validation context.
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
    ) -> anyhow::Result<ValidationReport> {
        let mut report = ValidationReport::default();
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
                let mut block_violations = Vec::new();
                for (line_idx, line) in block_with_context
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
                        let byte_offset = trimmed_line.as_ptr() as usize - line.as_ptr() as usize;
                        let column_offset = character_column_at(line, byte_offset) - 1;
                        let violation_start = block_with_context
                            .block
                            .content_position(line_idx, column_offset);
                        let line_character_end =
                            violation_start.character + trimmed_line.chars().count() - 1; // End position is inclusive.
                        block_violations.push(create_violation(
                            file_path,
                            &block_with_context.block,
                            pattern,
                            violation_start.line,
                            violation_start.character,
                            line_character_end,
                        )?);
                        break;
                    }
                }
                report.add_all(file_path, &block_with_context.block, block_violations);
            }
        }
        Ok(report)
    }
}

/// Selects [`LinePatternValidator`] for blocks carrying a `line-pattern` attribute.
pub(crate) struct LinePatternValidatorDetector();

impl LinePatternValidatorDetector {
    /// Creates the detector. Registered in [`crate::validators::detector_factories`].
    pub fn new() -> Self {
        Self {}
    }
}

impl<Fs: FileSystem> ValidatorDetector<Fs> for LinePatternValidatorDetector {
    fn detect(
        &self,
        block_with_context: &BlockWithContext,
        _file_system: &Arc<Fs>,
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
    use crate::repo_path::RepoPath;
    use crate::test_utils::{checked_lines, validation_context, violation_count};
    use serde_json::json;

    #[test]
    fn block_with_several_non_matching_lines_returns_the_first_violation_only() -> anyhow::Result<()>
    {
        let validator = LinePatternValidator::new();
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
    fn non_matching_line_holding_non_ascii_returns_a_range_measured_in_characters()
    -> anyhow::Result<()> {
        let context = validation_context(
            "example.py",
            "# <block line-pattern=\"^[A-Z]+$\">\ncaféx\n# </block>",
        );

        let violations = LinePatternValidator::new().validate(context)?.violations;

        let file_violations = violations
            .get(&RepoPath::from_reference("example.py")?)
            .unwrap();
        // `caféx` is five characters long even though it takes six bytes.
        assert_eq!(
            file_violations[0].range,
            ViolationRange::new(Position::new(2, 1), Position::new(2, 5))
        );
        Ok(())
    }

    #[test]
    fn block_with_every_line_matching_returns_no_violations() -> anyhow::Result<()> {
        let validator = LinePatternValidator::new();
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
    fn inline_block_with_a_non_matching_line_returns_violation_with_source_line_columns()
    -> anyhow::Result<()> {
        let context = validation_context(
            "example.rs",
            r#"const X: &str = /* <block line-pattern="^GOOD$"> */ "BAD" /* </block> */;"#,
        );

        let violations = LinePatternValidator::new().validate(context)?.violations;

        let file_violations = violations
            .get(&RepoPath::from_reference("example.rs")?)
            .unwrap();
        // `"BAD"` sits at columns 53..=57 of the source line; the block's content starts at
        // column 52, so a range measured from the content alone points at `const` instead.
        assert_eq!(
            file_violations[0].range,
            ViolationRange::new(Position::new(1, 53), Position::new(1, 57))
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

        let violations = LinePatternValidator::new().validate(context)?.violations;

        let file_violations = violations
            .get(&RepoPath::from_reference("example.rs")?)
            .unwrap();
        // The start tag opens on line 1 but the content only begins on line 2, after the tag's
        // comment closes.
        assert_eq!(
            file_violations[0].range,
            ViolationRange::new(Position::new(2, 27), Position::new(2, 31))
        );
        Ok(())
    }

    #[test]
    fn block_with_blank_and_whitespace_only_lines_returns_no_violations() -> anyhow::Result<()> {
        let validator = LinePatternValidator::new();
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
        let validator = LinePatternValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block line-pattern="[A-Z+">
        # </block>"#,
        );

        let result = validator.validate(context);

        assert!(result.is_err());
    }

    #[test]
    fn block_without_a_line_pattern_attribute_returns_no_violations() -> anyhow::Result<()> {
        let validator = LinePatternValidator::new();
        let context = validation_context("example.py", "#<block>\n#</block>");
        let violations = validator.validate(context)?.violations;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn block_with_empty_content_returns_no_violations() -> anyhow::Result<()> {
        let validator = LinePatternValidator::new();
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

        let report = LinePatternValidator::new().validate(context)?;

        // The block without a line-pattern attribute is not checked, so it records nothing.
        assert_eq!(checked_lines(&report), vec![1, 4]);
        assert_eq!(violation_count(&report), 1);
        Ok(())
    }
}
