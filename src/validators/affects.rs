use crate::blocks::{Block, BlockWithContext};
use crate::validators::{ValidatorType, Violation, ViolationRange};
use crate::{Position, validators};
use anyhow::Context;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;

pub(crate) struct AffectsValidator {}

impl AffectsValidator {
    pub(super) fn new() -> Self {
        Self {}
    }
}

#[derive(Serialize)]
struct AffectsViolation<'a> {
    affected_block_file_path: &'a str,
    affected_block_name: &'a str,
}

impl validators::ValidatorSync for AffectsValidator {
    fn validate(
        &self,
        context: Arc<validators::ValidationContext>,
    ) -> anyhow::Result<HashMap<String, Vec<Violation>>> {
        let mut named_modified_blocks = HashMap::new();
        for (file_path, file_blocks) in &context.modified_blocks {
            for block_with_context in &file_blocks.blocks_with_context {
                if !block_with_context.is_content_modified {
                    // Blocks with unmodified content are not considered modified by this validator.
                    continue;
                }
                if let Some(name) = block_with_context.block.name() {
                    named_modified_blocks
                        .entry((file_path.clone(), name.to_string()))
                        .or_insert_with(Vec::new)
                        .push(block_with_context);
                }
            }
        }
        let mut violations = HashMap::new();
        for (modified_block_file_path, file_blocks) in &context.modified_blocks {
            for block_with_context in &file_blocks.blocks_with_context {
                if !block_with_context.is_content_modified {
                    // Blocks with unmodified content are not considered modified by this validator.
                    continue;
                }
                if let Some(affects) = block_with_context.block.attributes.get("affects") {
                    let affected_blocks = parse_affects_attribute(affects)?;
                    for (affected_file_path, affected_block_name) in affected_blocks {
                        let affected_file_path =
                            affected_file_path.unwrap_or_else(|| modified_block_file_path.clone());
                        if !named_modified_blocks.contains_key(&(
                            affected_file_path.clone(),
                            affected_block_name.clone(),
                        )) {
                            violations
                                .entry(modified_block_file_path.clone())
                                .or_insert_with(Vec::new)
                                .push(create_violation(
                                    modified_block_file_path,
                                    Arc::clone(&block_with_context.block),
                                    &file_blocks.file_content_new_lines,
                                    affected_file_path.as_str(),
                                    affected_block_name.as_str(),
                                )?);
                        }
                    }
                }
            }
        }
        Ok(violations)
    }
}

pub(crate) struct AffectsValidatorDetector();

impl AffectsValidatorDetector {
    pub fn new() -> Self {
        Self {}
    }
}

impl validators::ValidatorDetector for AffectsValidatorDetector {
    fn detect(
        &self,
        block_with_context: &BlockWithContext,
    ) -> anyhow::Result<Option<ValidatorType>> {
        if block_with_context.is_content_modified
            && block_with_context.block.attributes.contains_key("affects")
        {
            Ok(Some(ValidatorType::Sync(Box::new(AffectsValidator::new()))))
        } else {
            Ok(None)
        }
    }
}

fn create_violation(
    modified_block_file_path: &str,
    modified_block: Arc<Block>,
    modified_block_new_line_positions: &[usize],
    affected_block_file_path: &str,
    affected_block_name: &str,
) -> anyhow::Result<Violation> {
    let message = format!(
        "Block {}:{} at line {} is modified, but {}:{} is not",
        modified_block_file_path,
        modified_block.name_display(),
        modified_block.starts_at_line,
        affected_block_file_path,
        affected_block_name
    );
    let details = serde_json::to_value(AffectsViolation {
        affected_block_file_path,
        affected_block_name,
    })
    .context("failed to serialize AffectsViolation block")?;
    Ok(Violation::new(
        ViolationRange::new(
            Position::from_byte_offset(
                modified_block.start_tag_range.start,
                modified_block_new_line_positions,
            ),
            Position::from_byte_offset(
                modified_block.start_tag_range.end - 1, // start_tag_range is non-inclusive.
                modified_block_new_line_positions,
            ),
        ),
        "affects".to_string(),
        message,
        modified_block,
        Some(details),
    ))
}

fn parse_affects_attribute(value: &str) -> anyhow::Result<Vec<(Option<String>, String)>> {
    let mut result = Vec::new();
    for block_ref in value.split(',') {
        let block = block_ref.trim();
        let (mut filename, block_name) = block
            .split_once(":")
            .context(format!("Invalid \"affects\" attribute value: \"{block}\"",))?;
        filename = filename.trim();
        result.push((
            if filename.is_empty() {
                None
            } else {
                Some(filename.to_string())
            },
            block_name.trim().to_string(),
        ));
    }
    Ok(result)
}

#[cfg(test)]
mod validate_tests {
    use super::*;
    use crate::blocks::FileBlocks;
    use crate::test_utils::{block_with_context, block_with_context_default, file_blocks_default};
    use crate::validators::ValidatorSync;

    #[test]
    fn no_blocks_with_affects_attr_returns_ok() -> anyhow::Result<()> {
        let validator = AffectsValidator::new();
        let modified_blocks = HashMap::from([(
            "file1".to_string(),
            file_blocks_default(vec![block_with_context_default(Block::new(
                1,
                10,
                HashMap::from([("name".to_string(), "foo".to_string())]),
                0..0,
                0..0,
            ))]),
        )]);

        let violations = validator.validate(Arc::new(validators::ValidationContext::new(
            modified_blocks,
        )))?;

        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn with_missing_blocks_in_same_file_returns_violations() -> anyhow::Result<()> {
        let validator = AffectsValidator::new();
        let modified_blocks = HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_content: "".to_string(),
                file_content_new_lines: vec![0],
                blocks_with_context: vec![
                    block_with_context(
                        Block::new(
                            1,
                            10,
                            HashMap::from([("affects".to_string(), ":foo".to_string())]),
                            1..10,
                            0..0,
                        ),
                        false,
                        true,
                    ),
                    block_with_context(
                        Block::new(
                            12,
                            16,
                            HashMap::from([("affects".to_string(), ":foo".to_string())]),
                            0..1,
                            0..0,
                        ),
                        false,
                        true,
                    ),
                ],
            },
        )]);

        let violations = validator.validate(Arc::new(validators::ValidationContext::new(
            modified_blocks,
        )))?;

        assert_eq!(violations.len(), 1);
        let file1_violations = violations.get("file1").unwrap();
        assert_eq!(file1_violations.len(), 2);
        assert_eq!(
            file1_violations[0].message,
            "Block file1:(unnamed) at line 1 is modified, but file1:foo is not"
        );
        assert_eq!(
            file1_violations[1].message,
            "Block file1:(unnamed) at line 12 is modified, but file1:foo is not"
        );

        Ok(())
    }

    #[test]
    fn with_missing_blocks_in_different_files_returns_violations() -> anyhow::Result<()> {
        let validator = AffectsValidator::new();
        let modified_blocks = HashMap::from([
            (
                "file1".to_string(),
                file_blocks_default(vec![
                    block_with_context(
                        Block::new(
                            1,
                            10,
                            HashMap::from([("affects".to_string(), "file2:foo".to_string())]),
                            0..1,
                            0..0,
                        ),
                        false,
                        true,
                    ),
                    block_with_context(
                        Block::new(
                            12,
                            16,
                            HashMap::from([("affects".to_string(), "file3:bar".to_string())]),
                            0..1,
                            0..0,
                        ),
                        false,
                        true,
                    ),
                ]),
            ),
            (
                "file2".to_string(),
                file_blocks_default(vec![block_with_context(
                    Block::new(
                        1,
                        10,
                        HashMap::from([("name".to_string(), "not-foo".to_string())]),
                        0..0,
                        0..0,
                    ),
                    false,
                    true,
                )]),
            ),
            (
                "file3".to_string(),
                file_blocks_default(vec![block_with_context(
                    Block::new(
                        1,
                        10,
                        HashMap::from([("name".to_string(), "not-bar".to_string())]),
                        0..0,
                        0..0,
                    ),
                    false,
                    true,
                )]),
            ),
        ]);

        let violations = validator.validate(Arc::new(validators::ValidationContext::new(
            modified_blocks,
        )))?;

        assert_eq!(violations.len(), 1);
        let file1_violations = violations.get("file1").unwrap();
        assert_eq!(file1_violations.len(), 2);
        assert_eq!(
            file1_violations[0].message,
            "Block file1:(unnamed) at line 1 is modified, but file2:foo is not"
        );
        assert_eq!(
            file1_violations[1].message,
            "Block file1:(unnamed) at line 12 is modified, but file3:bar is not"
        );

        Ok(())
    }

    #[test]
    fn with_cyclic_references_returns_ok() -> anyhow::Result<()> {
        let validator = AffectsValidator::new();
        let modified_blocks = HashMap::from([(
            "file1".to_string(),
            file_blocks_default(vec![
                block_with_context(
                    Block::new(
                        1,
                        10,
                        HashMap::from([
                            ("name".to_string(), "foo".to_string()),
                            ("affects".to_string(), ":bar".to_string()),
                        ]),
                        0..0,
                        0..0,
                    ),
                    false,
                    true,
                ),
                block_with_context(
                    Block::new(
                        12,
                        16,
                        HashMap::from([
                            ("name".to_string(), "bar".to_string()),
                            ("affects".to_string(), ":foo".to_string()),
                        ]),
                        0..0,
                        0..0,
                    ),
                    false,
                    true,
                ),
            ]),
        )]);

        let violations = validator.validate(Arc::new(validators::ValidationContext::new(
            modified_blocks,
        )))?;

        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn with_multiple_references_returns_ok() -> anyhow::Result<()> {
        let validator = AffectsValidator::new();
        let modified_blocks = HashMap::from([
            (
                "file1".to_string(),
                file_blocks_default(vec![
                    block_with_context(
                        Block::new(
                            1,
                            10,
                            HashMap::from([
                                ("name".to_string(), "foo".to_string()),
                                ("affects".to_string(), ":bar, file2:buzz".to_string()),
                            ]),
                            0..0,
                            0..0,
                        ),
                        false,
                        true,
                    ),
                    block_with_context(
                        Block::new(
                            12,
                            16,
                            HashMap::from([
                                ("name".to_string(), "bar".to_string()),
                                ("affects".to_string(), ":foo".to_string()),
                            ]),
                            0..0,
                            0..0,
                        ),
                        false,
                        true,
                    ),
                ]),
            ),
            (
                "file2".to_string(),
                file_blocks_default(vec![block_with_context(
                    Block::new(
                        1,
                        10,
                        HashMap::from([
                            ("name".to_string(), "buzz".to_string()),
                            ("affects".to_string(), "file1:bar".to_string()),
                        ]),
                        0..0,
                        0..0,
                    ),
                    false,
                    true,
                )]),
            ),
        ]);

        let violations = validator.validate(Arc::new(validators::ValidationContext::new(
            modified_blocks,
        )))?;

        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn with_multiple_references_and_some_missing_returns_violations() -> anyhow::Result<()> {
        let validator = AffectsValidator::new();
        let modified_blocks = HashMap::from([
            (
                "file1".to_string(),
                file_blocks_default(vec![
                    block_with_context(
                        Block::new(
                            1,
                            10,
                            HashMap::from([
                                ("name".to_string(), "foo".to_string()),
                                ("affects".to_string(), ":bar, file2:buzz".to_string()),
                            ]),
                            0..1,
                            0..0,
                        ),
                        false,
                        true,
                    ),
                    block_with_context(
                        Block::new(
                            12,
                            16,
                            HashMap::from([
                                ("name".to_string(), "bar".to_string()),
                                ("affects".to_string(), ":foo".to_string()),
                            ]),
                            0..1,
                            0..0,
                        ),
                        false,
                        true,
                    ),
                ]),
            ),
            (
                "file2".to_string(),
                file_blocks_default(vec![block_with_context(
                    Block::new(
                        1,
                        10,
                        HashMap::from([
                            ("name".to_string(), "not-buzz".to_string()),
                            ("affects".to_string(), "file1:bar".to_string()),
                        ]),
                        0..1,
                        0..0,
                    ),
                    false,
                    true,
                )]),
            ),
        ]);

        let violations = validator.validate(Arc::new(validators::ValidationContext::new(
            modified_blocks,
        )))?;

        assert_eq!(violations.len(), 1);
        let file1_violations = violations.get("file1").unwrap();
        assert_eq!(file1_violations.len(), 1);
        assert_eq!(
            file1_violations[0].message,
            "Block file1:foo at line 1 is modified, but file2:buzz is not"
        );
        Ok(())
    }

    #[test]
    fn with_no_missing_blocks_returns_ok() -> anyhow::Result<()> {
        let validator = AffectsValidator::new();
        let modified_blocks = HashMap::from([
            (
                "file1".to_string(),
                file_blocks_default(vec![
                    block_with_context(
                        Block::new(
                            1,
                            10,
                            HashMap::from([("affects".to_string(), "file2:foo".to_string())]),
                            0..0,
                            0..0,
                        ),
                        false,
                        true,
                    ),
                    block_with_context(
                        Block::new(
                            12,
                            16,
                            HashMap::from([("affects".to_string(), "file3:bar".to_string())]),
                            0..0,
                            0..0,
                        ),
                        false,
                        true,
                    ),
                ]),
            ),
            (
                "file2".to_string(),
                file_blocks_default(vec![block_with_context(
                    Block::new(
                        1,
                        10,
                        HashMap::from([("name".to_string(), "foo".to_string())]),
                        0..0,
                        0..0,
                    ),
                    false,
                    true,
                )]),
            ),
            (
                "file3".to_string(),
                file_blocks_default(vec![block_with_context(
                    Block::new(
                        1,
                        10,
                        HashMap::from([("name".to_string(), "bar".to_string())]),
                        0..0,
                        0..0,
                    ),
                    false,
                    true,
                )]),
            ),
        ]);

        let violations = validator.validate(Arc::new(validators::ValidationContext::new(
            modified_blocks,
        )))?;

        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn blocks_with_unmodified_content_returns_ok() -> anyhow::Result<()> {
        let validator = AffectsValidator::new();
        let modified_blocks = HashMap::from([
            (
                "file1".to_string(),
                file_blocks_default(vec![
                    block_with_context_default(Block::new(
                        1,
                        10,
                        HashMap::from([("affects".to_string(), "file2:foo".to_string())]),
                        0..0,
                        0..0,
                    )),
                    block_with_context(
                        Block::new(
                            12,
                            16,
                            HashMap::from([("affects".to_string(), "file3:bar".to_string())]),
                            0..0,
                            0..0,
                        ),
                        true, // Start tag is modified only, not the content.
                        false,
                    ),
                ]),
            ),
            (
                "file2".to_string(),
                file_blocks_default(vec![block_with_context(
                    Block::new(
                        1,
                        10,
                        HashMap::from([("name".to_string(), "foo".to_string())]),
                        0..0,
                        0..0,
                    ),
                    true,
                    false,
                )]),
            ),
            (
                "file3".to_string(),
                file_blocks_default(vec![block_with_context_default(Block::new(
                    1,
                    10,
                    HashMap::from([("name".to_string(), "bar".to_string())]),
                    0..0,
                    0..0,
                ))]),
            ),
        ]);

        let violations = validator.validate(Arc::new(validators::ValidationContext::new(
            modified_blocks,
        )))?;

        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn dependent_blocks_with_unmodified_content_returns_violations() -> anyhow::Result<()> {
        let validator = AffectsValidator::new();
        let modified_blocks = HashMap::from([(
            "file1".to_string(),
            file_blocks_default(vec![
                block_with_context(
                    Block::new(
                        1,
                        10,
                        HashMap::from([
                            ("name".to_string(), "foo".to_string()),
                            ("affects".to_string(), ":bar".to_string()),
                        ]),
                        0..1,
                        0..0,
                    ),
                    true,
                    true,
                ),
                block_with_context(
                    Block::new(
                        12,
                        16,
                        HashMap::from([
                            ("name".to_string(), "bar".to_string()),
                            ("affects".to_string(), ":foo".to_string()),
                        ]),
                        0..1,
                        0..0,
                    ),
                    true, // Start tag is modified only, not the content.
                    false,
                ),
            ]),
        )]);

        let violations = validator.validate(Arc::new(validators::ValidationContext::new(
            modified_blocks,
        )))?;

        assert!(!violations.is_empty());
        Ok(())
    }
}

#[cfg(test)]
mod parse_affects_attribute_tests {
    use super::*;

    #[test]
    fn single_reference() -> anyhow::Result<()> {
        let result = parse_affects_attribute("file.rs:block_name")?;
        assert_eq!(
            result,
            vec![(Some("file.rs".to_string()), "block_name".to_string())]
        );
        Ok(())
    }

    #[test]
    fn multiple_references() -> anyhow::Result<()> {
        let result = parse_affects_attribute("file1.rs:block1, file2.rs:block2")?;
        assert_eq!(
            result,
            vec![
                (Some("file1.rs".to_string()), "block1".to_string()),
                (Some("file2.rs".to_string()), "block2".to_string())
            ]
        );
        Ok(())
    }

    #[test]
    fn empty_filename_returns_none_for_filename() -> anyhow::Result<()> {
        let result = parse_affects_attribute(":block_name")?;
        assert_eq!(result, vec![(None, "block_name".to_string())]);
        Ok(())
    }

    #[test]
    fn multiple_empty_filename_references_returns_non_for_filename() -> anyhow::Result<()> {
        let result = parse_affects_attribute(":block1, :block2")?;
        assert_eq!(
            result,
            vec![(None, "block1".to_string()), (None, "block2".to_string())]
        );
        Ok(())
    }

    #[test]
    fn invalid_block_returns_error() {
        let result = parse_affects_attribute("invalid_reference");
        assert!(result.is_err());
    }
}
