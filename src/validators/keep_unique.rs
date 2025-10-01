use crate::blocks::Block;
use crate::validators;
use crate::validators::{ValidatorDetector, ValidatorSync, ValidatorType, Violation};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

pub(super) struct KeepUniqueValidator {}

impl KeepUniqueValidator {
    /// Creates a validator that ensures lines (or regex matches) within a block are unique.
    pub(crate) fn new() -> Self {
        Self {}
    }
}

#[derive(Serialize)]
struct KeepUniqueViolation {
    line_number_duplicated: usize,
}

impl ValidatorSync for KeepUniqueValidator {
    fn validate(
        &self,
        context: Arc<validators::ValidationContext>,
    ) -> anyhow::Result<HashMap<String, Vec<Violation>>> {
        let mut violations = HashMap::new();
        for (file_path, file_blocks) in &context.modified_blocks {
            for block in &file_blocks.blocks {
                if !block.attributes.contains_key("keep-unique") {
                    continue;
                }
                let pattern = block
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
                for (idx, line) in block
                    .content(&file_blocks.file_contents)
                    .lines()
                    .enumerate()
                {
                    let key_opt = match &re {
                        None => Some(line),
                        Some(Ok(re)) => {
                            if let Some(c) = re.captures(line) {
                                // If named group "value" exists use it, otherwise use whole match
                                if let Some(m) = c.name("value") {
                                    Some(m.as_str())
                                } else {
                                    c.get(0).map(|m| m.as_str())
                                }
                            } else {
                                None // skip line when no match
                            }
                        }
                        Some(Err(e)) => {
                            // Invalid regex: return an error for the validator
                            return Err(anyhow::anyhow!(
                                "Invalid keep-unique regex pattern for block {}:{} defined at line {}: {}",
                                file_path,
                                block.name_display(),
                                block.starts_at_line,
                                e
                            ));
                        }
                    };
                    if let Some(key) = key_opt
                        && !key.trim().is_empty()
                        && !seen.insert(key)
                    {
                        let line_no = block.starts_at_line + idx;
                        violations
                            .entry(file_path.clone())
                            .or_insert_with(Vec::new)
                            .push(create_violation(file_path, block, line_no)?);
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
    fn detect(&self, block: &Block) -> anyhow::Result<Option<ValidatorType>> {
        if block.attributes.contains_key("keep-unique") {
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
    block: &Block,
    line_number_duplicated: usize,
) -> anyhow::Result<Violation> {
    let message = format!(
        "Block {}:{} defined at line {} has a duplicated line {}",
        block_file_path,
        block.name_display(),
        block.starts_at_line,
        line_number_duplicated,
    );
    Ok(Violation::new(
        "keep-unique".to_string(),
        message,
        Some(serde_json::to_value(KeepUniqueViolation {
            line_number_duplicated,
        })?),
    ))
}

#[cfg(test)]
mod validate_tests {
    use super::*;
    use crate::blocks::{Block, FileBlocks};
    use crate::test_utils;
    use serde_json::json;

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
                blocks: vec![Arc::new(Block::new(
                    1,
                    2,
                    HashMap::from([("keep-unique".to_string(), "".to_string())]),
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
        let file1_contents = "block contents goes here: A\nB\nC";
        let context = Arc::new(validators::ValidationContext::new(HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_contents: file1_contents.to_string(),
                blocks: vec![Arc::new(Block::new(
                    1,
                    5,
                    HashMap::from([("keep-unique".to_string(), "".to_string())]),
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
        let file1_contents = "block contents goes here: A\nB\n \n \n  \n  \nC";
        let context = Arc::new(validators::ValidationContext::new(HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_contents: file1_contents.to_string(),
                blocks: vec![Arc::new(Block::new(
                    1,
                    5,
                    HashMap::from([("keep-unique".to_string(), "".to_string())]),
                    test_utils::substr_range(file1_contents, "A\nB\n \n \n  \n  \nC"),
                ))],
            },
        )])));

        let violations = validator.validate(context)?;

        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn duplicate_returns_violation_first_dup_line_reported() -> anyhow::Result<()> {
        let validator = KeepUniqueValidator::new();
        let file1_contents = "block contents goes here: A\nB\nC\nB\nC";
        let context = Arc::new(validators::ValidationContext::new(HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_contents: file1_contents.to_string(),
                blocks: vec![Arc::new(Block::new(
                    1,
                    6,
                    HashMap::from([("keep-unique".to_string(), "".to_string())]),
                    test_utils::substr_range(file1_contents, "A\nB\nC\nB\nC"),
                ))],
            },
        )])));

        let violations = validator.validate(context)?;

        assert_eq!(violations.len(), 1);
        assert_eq!(violations.get("file1").unwrap().len(), 1);
        assert_eq!(
            violations.get("file1").unwrap()[0].error,
            "Block file1:(unnamed) defined at line 1 has a duplicated line 4"
        );
        assert_eq!(violations.get("file1").unwrap()[0].violation, "keep-unique");
        assert_eq!(
            violations.get("file1").unwrap()[0].details,
            Some(json!({
                "line_number_duplicated": 4,
            }))
        );
        Ok(())
    }

    #[test]
    fn regex_with_named_group_detects_duplicates() -> anyhow::Result<()> {
        let validator = KeepUniqueValidator::new();
        let attrs = HashMap::from([("keep-unique".to_string(), "^ID:(?P<value>\\d+)".to_string())]);
        let file1_contents = "block contents goes here: ID:1 A\nID:2 B\nID:1 C";
        let context = Arc::new(validators::ValidationContext::new(HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_contents: file1_contents.to_string(),
                blocks: vec![Arc::new(Block::new(
                    1,
                    6,
                    attrs,
                    test_utils::substr_range(file1_contents, "ID:1 A\nID:2 B\nID:1 C"),
                ))],
            },
        )])));
        let violations = validator.validate(context)?;
        assert_eq!(violations.len(), 1);
        assert_eq!(violations.get("file1").unwrap().len(), 1);
        assert_eq!(
            violations.get("file1").unwrap()[0].details,
            Some(json!({"line_number_duplicated": 3}))
        );
        Ok(())
    }

    #[test]
    fn regex_without_named_group_uses_full_match() -> anyhow::Result<()> {
        let validator = KeepUniqueValidator::new();
        let attrs = HashMap::from([("keep-unique".to_string(), "^ID:\\d+".to_string())]);
        let file1_contents = "block contents goes here: ID:1 A\nID:2 B\nID:1 C";
        let context = Arc::new(validators::ValidationContext::new(HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_contents: file1_contents.to_string(),
                blocks: vec![Arc::new(Block::new(
                    1,
                    6,
                    attrs,
                    test_utils::substr_range(file1_contents, "ID:1 A\nID:2 B\nID:1 C"),
                ))],
            },
        )])));
        let violations = validator.validate(context)?;
        assert_eq!(violations.len(), 1);
        assert_eq!(violations.get("file1").unwrap().len(), 1);
        assert_eq!(
            violations.get("file1").unwrap()[0].details,
            Some(json!({"line_number_duplicated": 3}))
        );
        Ok(())
    }

    #[test]
    fn regex_non_matching_lines_are_skipped() -> anyhow::Result<()> {
        let validator = KeepUniqueValidator::new();
        let attrs = HashMap::from([("keep-unique".to_string(), "^ID:(?P<value>\\d+)".to_string())]);
        let file1_contents = "block contents goes here: ID:1\nX:2\nID:2";
        let context = Arc::new(validators::ValidationContext::new(HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_contents: file1_contents.to_string(),
                blocks: vec![Arc::new(Block::new(
                    1,
                    4,
                    attrs,
                    test_utils::substr_range(file1_contents, "ID:1\nX:2\nID:2"),
                ))],
            },
        )])));
        let violations = validator.validate(context)?;
        assert!(violations.is_empty());
        Ok(())
    }
}
