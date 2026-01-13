use crate::blocks::{Block, BlockWithContext};
use crate::validators::{
    ValidatorDetector, ValidatorSync, ValidatorType, Violation, ViolationRange,
};
use crate::{Position, validators};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
    ) -> anyhow::Result<HashMap<PathBuf, Vec<Violation>>> {
        let mut violations = HashMap::new();
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
                let mut seen: HashSet<&str> = HashSet::new();
                for (line_number, line) in block_with_context
                    .block
                    .content(&file_blocks.file_content)
                    .lines()
                    .enumerate()
                {
                    let line_match = match &re {
                        None => {
                            let trimmed_line = line.trim();
                            if trimmed_line.is_empty() {
                                None
                            } else {
                                let line_character_start =
                                    trimmed_line.as_ptr() as usize - line.as_ptr() as usize + 1;
                                let line_character_end =
                                    line_character_start + trimmed_line.len() - 1;
                                Some((trimmed_line, line_character_start..=line_character_end))
                            }
                        }
                        Some(Ok(re)) => {
                            if let Some(c) = re.captures(line) {
                                // If named group "value" exists use it, otherwise use whole match
                                if let Some(m) = c.name("value") {
                                    let range = m.range();
                                    Some((m.as_str(), range.start + 1..=range.end))
                                } else {
                                    c.get(0).map(|m| {
                                        let range = m.range();
                                        (m.as_str(), range.start + 1..=range.end)
                                    })
                                }
                            } else {
                                None // Skip line when no match
                            }
                        }
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
                        let violation_line_number = block_with_context
                            .block
                            .start_tag_position_range
                            .start()
                            .line
                            + line_number;
                        let line_character_start = *line_range.start(); // Start position is 1-based.
                        let line_character_end = *line_range.end(); // End position is 1-based and inclusive.
                        violations
                            .entry(file_path.clone())
                            .or_insert_with(Vec::new)
                            .push(create_violation(
                                file_path,
                                &block_with_context.block,
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

pub(crate) struct KeepUniqueValidatorDetector();

impl KeepUniqueValidatorDetector {
    pub fn new() -> Self {
        Self {}
    }
}

impl ValidatorDetector for KeepUniqueValidatorDetector {
    fn detect(
        &self,
        block_with_context: &BlockWithContext,
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
    block_file_path: &Path,
    block: &Block,
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
    Ok(Violation::new(
        ViolationRange::new(
            Position::new(violation_line_number, violation_character_start),
            Position::new(violation_line_number, violation_character_end),
        ),
        "keep-unique".to_string(),
        message,
        block.severity()?,
        None,
    ))
}

#[cfg(test)]
mod validate_tests {
    use super::*;
    use crate::test_utils::validation_context;

    #[test]
    fn empty_blocks_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepUniqueValidator::new();
        let context = Arc::new(validators::ValidationContext::new(HashMap::new()));

        let violations = validator.validate(context)?;

        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn blocks_with_empty_content_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepUniqueValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block keep-unique>
# </block>"#,
        );

        let violations = validator.validate(context)?;

        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn all_unique_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepUniqueValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block keep-unique>
A
B
C
# </block>"#,
        );

        let violations = validator.validate(context)?;

        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn empty_lines_and_spaces_are_ignored() -> anyhow::Result<()> {
        let validator = KeepUniqueValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block keep-unique>
A
B
 
 
  
  
C
# </block>"#,
        );

        let violations = validator.validate(context)?;

        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn spaces_in_regex_are_not_ignored() -> anyhow::Result<()> {
        let validator = KeepUniqueValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block keep-unique=" \d+ ">
 1 
 2 
1
 1 
# </block>"#,
        );

        let violations = validator.validate(context)?;

        assert_eq!(violations.len(), 1);
        let file_violations = violations.get(&PathBuf::from("example.py")).unwrap();
        assert_eq!(file_violations.len(), 1);
        // The last line ` 1 ` is the only duplicate.
        assert_eq!(
            file_violations[0].range,
            ViolationRange::new(Position::new(5, 1), Position::new(5, 3))
        );
        Ok(())
    }

    #[test]
    fn duplicate_returns_violation_first_dup_line_reported() -> anyhow::Result<()> {
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

        let violations = validator.validate(context)?;

        assert_eq!(violations.len(), 1);
        let file_violations = violations.get(&PathBuf::from("example.py")).unwrap();
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
    fn regex_with_named_group_detects_duplicates() -> anyhow::Result<()> {
        let validator = KeepUniqueValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block keep-unique="^ID:(?P<value>\d+)">
ID:1 A
ID:2 B
ID:1 C
# </block>"#,
        );

        let violations = validator.validate(context)?;
        assert_eq!(violations.len(), 1);
        let file_violations = violations.get(&PathBuf::from("example.py")).unwrap();
        assert_eq!(file_violations.len(), 1);
        // Only the matched value group is in the range.
        assert_eq!(
            file_violations[0].range,
            ViolationRange::new(Position::new(4, 4), Position::new(4, 4))
        );
        Ok(())
    }

    #[test]
    fn regex_without_named_group_uses_full_match() -> anyhow::Result<()> {
        let validator = KeepUniqueValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block keep-unique="^ID:\d+">
ID:1 A
ID:2 B
ID:1 C
# </block>"#,
        );

        let violations = validator.validate(context)?;
        assert_eq!(violations.len(), 1);
        let file_violations = violations.get(&PathBuf::from("example.py")).unwrap();
        assert_eq!(file_violations.len(), 1);
        // Full regex match is in the range.
        assert_eq!(
            file_violations[0].range,
            ViolationRange::new(Position::new(4, 1), Position::new(4, 4))
        );
        Ok(())
    }

    #[test]
    fn regex_non_matching_lines_are_skipped() -> anyhow::Result<()> {
        let validator = KeepUniqueValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block keep-unique="^ID:(?P<value>\d+)">
ID:1
X:2
ID:2
# </block>"#,
        );

        let violations = validator.validate(context)?;
        assert!(violations.is_empty());
        Ok(())
    }
}
