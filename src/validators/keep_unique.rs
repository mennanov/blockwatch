use crate::blocks::{Block, BlockWithContext};
use crate::fs::FileSystem;
use crate::repo_path::RepoPath;
use crate::validators::{
    ValidationReport, ValidatorDetector, ValidatorSync, ValidatorType, Violation, ViolationRange,
    regex_value, trimmed_line_value,
};
use crate::{Position, validators};
use std::collections::HashSet;
use std::sync::Arc;

/// Enforces `keep-unique`: no two non-empty lines inside the block may be equal.
///
/// The attribute's value optionally supplies a regex selecting the part of each line that must be
/// unique, which is how lists of ids or keys are checked without regard to the rest of the line.
pub(super) struct KeepUniqueValidator {}

impl KeepUniqueValidator {
    /// Creates a validator that ensures lines (or regex matches) within a block are unique.
    pub(crate) fn new() -> Self {
        Self {}
    }
}

impl ValidatorSync for KeepUniqueValidator {
    fn validate(
        &self,
        context: Arc<validators::ValidationContext>,
    ) -> anyhow::Result<ValidationReport> {
        let mut report = ValidationReport::default();
        for (file_path, file_blocks) in &context.blocks {
            for block_with_context in &file_blocks.blocks_with_context {
                if !block_with_context
                    .block
                    .attributes
                    .contains_key("keep-unique")
                {
                    continue;
                }
                let pattern = block_with_context
                    .block
                    .attributes
                    .get("keep-unique")
                    .cloned()
                    .unwrap_or_default();
                let re = if pattern.is_empty() {
                    None
                } else {
                    Some(regex::Regex::new(&pattern))
                };
                let mut block_violations = Vec::new();
                let mut seen: HashSet<&str> = HashSet::new();
                for (line_number, line) in block_with_context
                    .block
                    .content(&file_blocks.file_content)
                    .lines()
                    .enumerate()
                {
                    let line_match = match &re {
                        None => trimmed_line_value(line),
                        Some(Ok(re)) => regex_value(line, re),
                        Some(Err(e)) => {
                            // Invalid regex: return an error for the validator
                            return Err(anyhow::anyhow!(
                                "Invalid keep-unique regex pattern for block {}:{} defined at line {}: {}",
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
                    if let Some((matched_line, line_range)) = line_match
                        && !seen.insert(matched_line)
                    {
                        // `line_range` is a 1-based column range within the content line, which is
                        // not where that line starts in the source: content begins where the start
                        // tag's comment ends.
                        let violation_start = block_with_context
                            .block
                            .content_position(line_number, *line_range.start() - 1);
                        let line_character_end =
                            violation_start.character + (*line_range.end() - *line_range.start()); // End position is inclusive.
                        block_violations.push(create_violation(
                            file_path,
                            &block_with_context.block,
                            line.trim(),
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

/// Selects [`KeepUniqueValidator`] for blocks carrying a `keep-unique` attribute.
pub(crate) struct KeepUniqueValidatorDetector();

impl KeepUniqueValidatorDetector {
    /// Creates the detector. Registered in [`validators::detector_factories`].
    pub fn new() -> Self {
        Self {}
    }
}

impl<Fs: FileSystem> ValidatorDetector<Fs> for KeepUniqueValidatorDetector {
    fn detect(
        &self,
        block_with_context: &BlockWithContext,
        _file_system: &Arc<Fs>,
    ) -> anyhow::Result<Option<ValidatorType>> {
        if block_with_context
            .block
            .attributes
            .contains_key("keep-unique")
        {
            Ok(Some(ValidatorType::Sync(Box::new(
                KeepUniqueValidator::new(),
            ))))
        } else {
            Ok(None)
        }
    }
}

fn create_violation(
    block_file_path: &RepoPath,
    block: &Block,
    violation_line: &str,
    violation_line_number: usize,
    violation_character_start: usize,
    violation_character_end: usize,
) -> anyhow::Result<Violation> {
    let message = format!(
        "Block {}:{} defined at line {} has a duplicated line {}",
        block_file_path.display(),
        block.name_display(),
        block.start_tag_position_range.start().line,
        violation_line_number,
    );
    Violation::new(
        ViolationRange::new(
            Position::new(violation_line_number, violation_character_start),
            Position::new(violation_line_number, violation_character_end),
        ),
        block_file_path,
        block,
        "keep-unique".to_string(),
        message,
        // Identify the violation by the offending line rather than its number, which shifts
        // whenever anything above it is added or removed.
        Some(violation_line),
        None,
    )
}

#[cfg(test)]
mod validate_tests {
    use super::*;
    use crate::repo_path::RepoPath;
    use crate::test_utils::{checked_lines, validation_context, violation_count};
    use std::collections::HashMap;

    #[test]
    fn block_with_a_duplicate_line_returns_a_violation_on_the_repeat() -> anyhow::Result<()> {
        let validator = KeepUniqueValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block keep-unique>
A
BB
C
BB
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
            "Block example.py:(unnamed) defined at line 1 has a duplicated line 5"
        );
        assert_eq!(file_violations[0].code, "keep-unique");
        // Entire line is in the range.
        assert_eq!(
            file_violations[0].range,
            ViolationRange::new(Position::new(5, 1), Position::new(5, 2))
        );
        Ok(())
    }

    #[test]
    fn duplicated_line_holding_non_ascii_returns_a_range_measured_in_characters()
    -> anyhow::Result<()> {
        let validator = KeepUniqueValidator::new();
        let context = validation_context(
            "example.py",
            "# <block keep-unique>\ncafé\ncafé\n# </block>",
        );

        let violations = validator.validate(context)?.violations;

        let file_violations = violations
            .get(&RepoPath::from_reference("example.py")?)
            .unwrap();
        // `café` is four characters long even though it takes five bytes.
        assert_eq!(
            file_violations[0].range,
            ViolationRange::new(Position::new(3, 1), Position::new(3, 4))
        );
        Ok(())
    }

    #[test]
    fn block_with_all_unique_lines_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepUniqueValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block keep-unique>
A
B
C
# </block>"#,
        );

        let violations = validator.validate(context)?.violations;

        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn block_with_a_multiline_start_tag_returns_a_violation_on_the_duplicate_line()
    -> anyhow::Result<()> {
        let context = validation_context(
            "example.rs",
            "/* <block\nkeep-unique> */\nA\nA\n/* </block> */",
        );

        let violations = KeepUniqueValidator::new().validate(context)?.violations;

        let file_violations = violations
            .get(&RepoPath::from_reference("example.rs")?)
            .unwrap();
        // The start tag spans lines 1-2, so the content starts on line 2 and the repeat sits on
        // line 4. Anchoring to the start tag instead would name line 3, the first occurrence.
        assert_eq!(
            file_violations[0].range,
            ViolationRange::new(Position::new(4, 1), Position::new(4, 1))
        );
        Ok(())
    }

    #[test]
    fn pattern_with_a_named_group_returns_a_violation_for_a_duplicate_group_value()
    -> anyhow::Result<()> {
        let validator = KeepUniqueValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block keep-unique="^ID:(?P<value>\d+)">
ID:1 A
ID:2 B
ID:1 C
# </block>"#,
        );

        let violations = validator.validate(context)?.violations;
        assert_eq!(violations.len(), 1);
        let file_violations = violations
            .get(&RepoPath::from_reference("example.py")?)
            .unwrap();
        assert_eq!(file_violations.len(), 1);
        // Only the matched value group is in the range.
        assert_eq!(
            file_violations[0].range,
            ViolationRange::new(Position::new(4, 4), Position::new(4, 4))
        );
        Ok(())
    }

    #[test]
    fn pattern_matches_within_the_trimmed_line() -> anyhow::Result<()> {
        let validator = KeepUniqueValidator::new();
        // `^` and `$` anchor to the trimmed line, so entries keep matching once they are nested
        // inside a list or a block of code.
        let context = validation_context(
            "example.py",
            "# <block keep-unique=\"^ID:(?P<value>\\d+)$\">\n  ID:1\n  ID:1\n# </block>",
        );

        let violations = validator.validate(context)?.violations;

        let file_violations = violations
            .get(&RepoPath::from_reference("example.py")?)
            .unwrap();
        // The range still points into the original line, indentation included.
        assert_eq!(
            file_violations[0].range,
            ViolationRange::new(Position::new(3, 6), Position::new(3, 6))
        );
        Ok(())
    }

    #[test]
    fn pattern_matching_empty_text_skips_blank_lines() -> anyhow::Result<()> {
        let validator = KeepUniqueValidator::new();
        let context = validation_context(
            "example.py",
            "# <block keep-unique=\"(?P<value>.*)\">\nA\n\nB\n\n# </block>",
        );

        let violations = validator.validate(context)?.violations;

        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn pattern_matching_empty_text_skips_the_line() -> anyhow::Result<()> {
        let validator = KeepUniqueValidator::new();
        // An empty match carries no value to compare, so the line counts as unmatched rather than
        // as an entry that is equal to every other empty match.
        let context = validation_context(
            "example.py",
            "# <block keep-unique=\"(?P<value>\\d*)\">\nabc\ndef\n# </block>",
        );

        let violations = validator.validate(context)?.violations;

        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn pattern_without_a_named_group_returns_a_violation_for_a_duplicate_whole_match()
    -> anyhow::Result<()> {
        let validator = KeepUniqueValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block keep-unique="^ID:\d+">
ID:1 A
ID:2 B
ID:1 C
# </block>"#,
        );

        let violations = validator.validate(context)?.violations;
        assert_eq!(violations.len(), 1);
        let file_violations = violations
            .get(&RepoPath::from_reference("example.py")?)
            .unwrap();
        assert_eq!(file_violations.len(), 1);
        // Full regex match is in the range.
        assert_eq!(
            file_violations[0].range,
            ViolationRange::new(Position::new(4, 1), Position::new(4, 4))
        );
        Ok(())
    }

    #[test]
    fn block_with_lines_not_matching_the_pattern_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepUniqueValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block keep-unique="^ID:(?P<value>\d+)">
ID:1
X:2
ID:2
# </block>"#,
        );

        let violations = validator.validate(context)?.violations;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn pattern_spaces_match_within_the_trimmed_line() -> anyhow::Result<()> {
        let validator = KeepUniqueValidator::new();
        // Spaces the pattern asks for still have to be there inside the entry; the padding around
        // the entry is not part of what the pattern sees, so the same value written with and
        // without padding is one value.
        let context = validation_context(
            "example.py",
            "# <block keep-unique=\"^ID: (?P<value>\\d+)$\">\n  ID: 1  \nID:2\nID: 1\n# </block>",
        );

        let violations = validator.validate(context)?.violations;

        assert_eq!(violations.len(), 1);
        let file_violations = violations
            .get(&RepoPath::from_reference("example.py")?)
            .unwrap();
        assert_eq!(file_violations.len(), 1);
        // `ID:2` lacks the space the pattern requires, so line 4 is the first repeat of `1`.
        assert_eq!(
            file_violations[0].range,
            ViolationRange::new(Position::new(4, 5), Position::new(4, 5))
        );
        Ok(())
    }

    #[test]
    fn block_with_blank_and_whitespace_only_lines_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepUniqueValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block keep-unique>
A
B
 
 
  
  
C
# </block>"#,
        );

        let violations = validator.validate(context)?.violations;

        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn block_with_empty_content_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepUniqueValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block keep-unique>
# </block>"#,
        );

        let violations = validator.validate(context)?.violations;

        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn context_without_any_blocks_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepUniqueValidator::new();
        let context = Arc::new(validators::ValidationContext::new(
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
        ));

        let violations = validator.validate(context)?.violations;

        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn blocks_with_and_without_keep_unique_records_a_check_for_the_examined_ones_only()
    -> anyhow::Result<()> {
        let context = validation_context(
            "example.py",
            r#"# <block name="unique" keep-unique>
'apple',
'banana',
# </block>
# <block name="duplicated" keep-unique>
'apple',
'apple',
# </block>
# <block name="unrelated">
'apple',
'apple',
# </block>"#,
        );

        let report = KeepUniqueValidator::new().validate(context)?;

        // The block without a keep-unique attribute is not checked, so it records nothing.
        assert_eq!(checked_lines(&report), vec![1, 5]);
        assert_eq!(violation_count(&report), 1);
        Ok(())
    }
}
