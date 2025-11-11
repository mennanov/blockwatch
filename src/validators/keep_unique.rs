use crate::blocks::{Block, BlockWithContext};
use crate::validators;
use crate::validators::{
    ValidatorDetector, ValidatorSync, ValidatorType, Violation, ViolationRange,
};
use std::collections::{HashMap, HashSet};
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
    ) -> anyhow::Result<HashMap<String, Vec<Violation>>> {
        let mut violations = HashMap::new();
        for (file_path, file_blocks) in &context.modified_blocks {
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
                    .content(&file_blocks.file_contents)
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
                                file_path,
                                block_with_context.block.name_display(),
                                block_with_context.block.starts_at_line,
                                e
                            ));
                        }
                    };
                    if let Some((matched_line, line_range)) = line_match
                        && !seen.insert(matched_line)
                    {
                        let violation_line_number =
                            block_with_context.block.starts_at_line + line_number;
                        let line_character_start = *line_range.start(); // Start position is 1-based.
                        let line_character_end = *line_range.end(); // End position is 1-based and inclusive.
                        violations
                            .entry(file_path.clone())
                            .or_insert_with(Vec::new)
                            .push(create_violation(
                                file_path,
                                Arc::clone(&block_with_context.block),
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
    block_file_path: &str,
    block: Arc<Block>,
    violation_line_number: usize,
    violation_character_start: usize,
    violation_character_end: usize,
) -> anyhow::Result<Violation> {
    let message = format!(
        "Block {}:{} defined at line {} has a duplicated line {}",
        block_file_path,
        block.name_display(),
        block.starts_at_line,
        violation_line_number,
    );
    Ok(Violation::new(
        ViolationRange::new(
            violation_line_number,
            violation_character_start,
            violation_line_number,
            violation_character_end,
        ),
        "keep-unique".to_string(),
        message,
        block,
        None,
    ))
}

#[cfg(test)]
mod validate_tests {
    use super::*;
    use crate::blocks::{Block, FileBlocks};
    use crate::test_utils;
    use crate::test_utils::block_with_context_default;

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
        let context = Arc::new(validators::ValidationContext::new(HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_contents: "".to_string(),
                blocks_with_context: vec![block_with_context_default(Block::new(
                    1,
                    2,
                    HashMap::from([("keep-unique".to_string(), "".to_string())]),
                    0..0,
                    0..0,
                ))],
            },
        )])));

        let violations = validator.validate(context)?;

        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn all_unique_returns_no_violations() -> anyhow::Result<()> {
        let validator = KeepUniqueValidator::new();
        let file1_contents = "/*<block>*/block contents goes here: A\nB\nC//</block>";
        let context = Arc::new(validators::ValidationContext::new(HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_contents: file1_contents.to_string(),
                blocks_with_context: vec![block_with_context_default(Block::new(
                    1,
                    5,
                    HashMap::from([("keep-unique".to_string(), "".to_string())]),
                    test_utils::substr_range(file1_contents, "<block>"),
                    test_utils::substr_range(file1_contents, "A\nB\nC"),
                ))],
            },
        )])));

        let violations = validator.validate(context)?;

        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn empty_lines_and_spaces_are_ignored() -> anyhow::Result<()> {
        let validator = KeepUniqueValidator::new();
        let file1_contents = "/*<block>*/block contents goes here: A\nB\n \n \n  \n  \nC//</block>";
        let context = Arc::new(validators::ValidationContext::new(HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_contents: file1_contents.to_string(),
                blocks_with_context: vec![block_with_context_default(Block::new(
                    1,
                    5,
                    HashMap::from([("keep-unique".to_string(), "".to_string())]),
                    test_utils::substr_range(file1_contents, "<block>"),
                    test_utils::substr_range(file1_contents, "A\nB\n \n \n  \n  \nC"),
                ))],
            },
        )])));

        let violations = validator.validate(context)?;

        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn spaces_in_regex_are_not_ignored() -> anyhow::Result<()> {
        let validator = KeepUniqueValidator::new();
        let file1_contents = "/*<block>*/block contents goes here:  1 \n 2 \n1\n 1 //</block>";
        let context = Arc::new(validators::ValidationContext::new(HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_contents: file1_contents.to_string(),
                blocks_with_context: vec![block_with_context_default(Block::new(
                    1,
                    5,
                    HashMap::from([("keep-unique".to_string(), " \\d+ ".to_string())]),
                    test_utils::substr_range(file1_contents, "<block>"),
                    test_utils::substr_range(file1_contents, " 1 \n 2 \n1\n 1 "),
                ))],
            },
        )])));

        let violations = validator.validate(context)?;

        assert_eq!(violations.len(), 1);
        let file1_violations = violations.get("file1").unwrap();
        assert_eq!(file1_violations.len(), 1);
        // The last line ` 1 ` is the only duplicate.
        assert_eq!(file1_violations[0].range, ViolationRange::new(4, 1, 4, 3));
        Ok(())
    }

    #[test]
    fn duplicate_returns_violation_first_dup_line_reported() -> anyhow::Result<()> {
        let validator = KeepUniqueValidator::new();
        let file1_contents = "/*<block>*/block contents goes here: A\nBB\nC\nBB\nC\nBB//</block>";
        let context = Arc::new(validators::ValidationContext::new(HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_contents: file1_contents.to_string(),
                blocks_with_context: vec![block_with_context_default(Block::new(
                    1,
                    6,
                    HashMap::from([("keep-unique".to_string(), "".to_string())]),
                    test_utils::substr_range(file1_contents, "<block>"),
                    test_utils::substr_range(file1_contents, "A\nBB\nC\nBB\nC\nBB"),
                ))],
            },
        )])));

        let violations = validator.validate(context)?;

        assert_eq!(violations.len(), 1);
        let file1_violations = violations.get("file1").unwrap();
        assert_eq!(file1_violations.len(), 1);
        assert_eq!(
            file1_violations[0].message,
            "Block file1:(unnamed) defined at line 1 has a duplicated line 4"
        );
        assert_eq!(file1_violations[0].code, "keep-unique");
        // Entire line is in the range.
        assert_eq!(file1_violations[0].range, ViolationRange::new(4, 1, 4, 2));
        Ok(())
    }

    #[test]
    fn regex_with_named_group_detects_duplicates() -> anyhow::Result<()> {
        let validator = KeepUniqueValidator::new();
        let attrs = HashMap::from([("keep-unique".to_string(), "^ID:(?P<value>\\d+)".to_string())]);
        let file1_contents =
            "/*<block>*/block contents goes here: ID:1 A\nID:2 B\nID:1 C//</block>";
        let context = Arc::new(validators::ValidationContext::new(HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_contents: file1_contents.to_string(),
                blocks_with_context: vec![block_with_context_default(Block::new(
                    1,
                    6,
                    attrs,
                    test_utils::substr_range(file1_contents, "<block>"),
                    test_utils::substr_range(file1_contents, "ID:1 A\nID:2 B\nID:1 C"),
                ))],
            },
        )])));
        let violations = validator.validate(context)?;
        assert_eq!(violations.len(), 1);
        let file1_violations = violations.get("file1").unwrap();
        assert_eq!(file1_violations.len(), 1);
        // Only the matched value group is in the range.
        assert_eq!(file1_violations[0].range, ViolationRange::new(3, 4, 3, 4));
        Ok(())
    }

    #[test]
    fn regex_without_named_group_uses_full_match() -> anyhow::Result<()> {
        let validator = KeepUniqueValidator::new();
        let attrs = HashMap::from([("keep-unique".to_string(), "^ID:\\d+".to_string())]);
        let file1_contents =
            "/*<block>*/block contents goes here: ID:1 A\nID:2 B\nID:1 C//</block>";
        let context = Arc::new(validators::ValidationContext::new(HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_contents: file1_contents.to_string(),
                blocks_with_context: vec![block_with_context_default(Block::new(
                    1,
                    6,
                    attrs,
                    test_utils::substr_range(file1_contents, "<block>"),
                    test_utils::substr_range(file1_contents, "ID:1 A\nID:2 B\nID:1 C"),
                ))],
            },
        )])));
        let violations = validator.validate(context)?;
        assert_eq!(violations.len(), 1);
        let file1_violations = violations.get("file1").unwrap();
        assert_eq!(file1_violations.len(), 1);
        // Full regex match is in the range.
        assert_eq!(file1_violations[0].range, ViolationRange::new(3, 1, 3, 4));
        Ok(())
    }

    #[test]
    fn regex_non_matching_lines_are_skipped() -> anyhow::Result<()> {
        let validator = KeepUniqueValidator::new();
        let attrs = HashMap::from([("keep-unique".to_string(), "^ID:(?P<value>\\d+)".to_string())]);
        let file1_contents = "/*<block>*/block contents goes here: ID:1\nX:2\nID:2//</block>";
        let context = Arc::new(validators::ValidationContext::new(HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_contents: file1_contents.to_string(),
                blocks_with_context: vec![block_with_context_default(Block::new(
                    1,
                    4,
                    attrs,
                    test_utils::substr_range(file1_contents, "<block>"),
                    test_utils::substr_range(file1_contents, "ID:1\nX:2\nID:2"),
                ))],
            },
        )])));
        let violations = validator.validate(context)?;
        assert!(violations.is_empty());
        Ok(())
    }
}
