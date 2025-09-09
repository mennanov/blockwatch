use crate::blocks::Block;
use crate::validators;
use crate::validators::Violation;
use anyhow::Context;
use async_trait::async_trait;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;

pub(crate) struct AffectsValidator {}

impl AffectsValidator {
    pub(crate) fn new() -> Self {
        Self {}
    }
}

#[derive(Serialize)]
struct AffectsViolation<'a> {
    modified_block: &'a Block,
}

#[async_trait]
impl validators::Validator for AffectsValidator {
    async fn validate(
        &self,
        context: Arc<validators::Context>,
    ) -> anyhow::Result<HashMap<String, Vec<Violation>>> {
        let mut named_modified_blocks = HashMap::new();
        for (file_path, blocks) in &context.modified_blocks {
            for block in blocks {
                match block.name() {
                    None => continue,
                    Some(name) => {
                        named_modified_blocks
                            .entry((file_path.clone(), name.to_string()))
                            .or_insert_with(Vec::new)
                            .push(block);
                    }
                }
            }
        }
        let mut violations = HashMap::new();
        for (modified_block_file_path, blocks) in &context.modified_blocks {
            for modified_block in blocks {
                match modified_block.attributes.get("affects") {
                    None => continue,
                    Some(affects) => {
                        let affected_blocks = parse_affects_attribute(affects)?;
                        for (affected_file_path, affected_block_name) in affected_blocks {
                            let affected_file_path = affected_file_path
                                .unwrap_or_else(|| modified_block_file_path.clone());
                            if !named_modified_blocks.contains_key(&(
                                affected_file_path.clone(),
                                affected_block_name.clone(),
                            )) {
                                violations
                                    .entry(modified_block_file_path.clone())
                                    .or_insert_with(Vec::new)
                                    .push(create_violation(
                                        modified_block_file_path,
                                        modified_block,
                                        affected_file_path.as_str(),
                                        affected_block_name.as_str(),
                                    )?);
                            }
                        }
                    }
                }
            }
        }
        Ok(violations)
    }
}

#[cfg(test)]
mod validate_tests {
    use super::*;
    use crate::validators::Validator;

    #[tokio::test]
    async fn no_blocks_with_affects_attr_returns_ok() -> anyhow::Result<()> {
        let validator = AffectsValidator::new();
        let modified_blocks = HashMap::from([(
            "file1".to_string(),
            vec![Arc::new(Block::new(
                1,
                10,
                HashMap::from([("name".to_string(), "foo".to_string())]),
                "".to_string(),
            ))],
        )]);

        let violations = validator
            .validate(Arc::new(validators::Context::new(modified_blocks)))
            .await?;

        assert!(violations.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn with_missing_blocks_in_same_file_returns_violations() -> anyhow::Result<()> {
        let validator = AffectsValidator::new();
        let modified_blocks = HashMap::from([(
            "file1".to_string(),
            vec![
                Arc::new(Block::new(
                    1,
                    10,
                    HashMap::from([("affects".to_string(), ":foo".to_string())]),
                    "".to_string(),
                )),
                Arc::new(Block::new(
                    12,
                    16,
                    HashMap::from([("affects".to_string(), ":foo".to_string())]),
                    "".to_string(),
                )),
            ],
        )]);

        let violations = validator
            .validate(Arc::new(validators::Context::new(modified_blocks)))
            .await?;

        assert_eq!(violations.len(), 1);
        assert_eq!(violations.get("file1").unwrap().len(), 2);
        assert_eq!(
            violations.get("file1").unwrap()[0].error,
            "Block file1:(unnamed) at line 1 is modified, but file1:foo is not"
        );
        assert_eq!(
            violations.get("file1").unwrap()[1].error,
            "Block file1:(unnamed) at line 12 is modified, but file1:foo is not"
        );

        Ok(())
    }

    #[tokio::test]
    async fn with_missing_blocks_in_different_files_returns_violations() -> anyhow::Result<()> {
        let validator = AffectsValidator::new();
        let modified_blocks = HashMap::from([
            (
                "file1".to_string(),
                vec![
                    Arc::new(Block::new(
                        1,
                        10,
                        HashMap::from([("affects".to_string(), "file2:foo".to_string())]),
                        "".to_string(),
                    )),
                    Arc::new(Block::new(
                        12,
                        16,
                        HashMap::from([("affects".to_string(), "file3:bar".to_string())]),
                        "".to_string(),
                    )),
                ],
            ),
            (
                "file2".to_string(),
                vec![Arc::new(Block::new(
                    1,
                    10,
                    HashMap::from([("name".to_string(), "not-foo".to_string())]),
                    "".to_string(),
                ))],
            ),
            (
                "file3".to_string(),
                vec![Arc::new(Block::new(
                    1,
                    10,
                    HashMap::from([("name".to_string(), "not-bar".to_string())]),
                    "".to_string(),
                ))],
            ),
        ]);

        let violations = validator
            .validate(Arc::new(validators::Context::new(modified_blocks)))
            .await?;

        assert_eq!(violations.len(), 1);
        assert_eq!(violations.get("file1").unwrap().len(), 2);
        assert_eq!(
            violations.get("file1").unwrap()[0].error,
            "Block file1:(unnamed) at line 1 is modified, but file2:foo is not"
        );
        assert_eq!(
            violations.get("file1").unwrap()[1].error,
            "Block file1:(unnamed) at line 12 is modified, but file3:bar is not"
        );

        Ok(())
    }

    #[tokio::test]
    async fn with_cyclic_references_returns_ok() -> anyhow::Result<()> {
        let validator = AffectsValidator::new();
        let modified_blocks = HashMap::from([(
            "file1".to_string(),
            vec![
                Arc::new(Block::new(
                    1,
                    10,
                    HashMap::from([
                        ("name".to_string(), "foo".to_string()),
                        ("affects".to_string(), ":bar".to_string()),
                    ]),
                    "".to_string(),
                )),
                Arc::new(Block::new(
                    12,
                    16,
                    HashMap::from([
                        ("name".to_string(), "bar".to_string()),
                        ("affects".to_string(), ":foo".to_string()),
                    ]),
                    "".to_string(),
                )),
            ],
        )]);

        let violations = validator
            .validate(Arc::new(validators::Context::new(modified_blocks)))
            .await?;

        assert!(violations.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn with_multiple_references_returns_ok() -> anyhow::Result<()> {
        let validator = AffectsValidator::new();
        let modified_blocks = HashMap::from([
            (
                "file1".to_string(),
                vec![
                    Arc::new(Block::new(
                        1,
                        10,
                        HashMap::from([
                            ("name".to_string(), "foo".to_string()),
                            ("affects".to_string(), ":bar, file2:buzz".to_string()),
                        ]),
                        "".to_string(),
                    )),
                    Arc::new(Block::new(
                        12,
                        16,
                        HashMap::from([
                            ("name".to_string(), "bar".to_string()),
                            ("affects".to_string(), ":foo".to_string()),
                        ]),
                        "".to_string(),
                    )),
                ],
            ),
            (
                "file2".to_string(),
                vec![Arc::new(Block::new(
                    1,
                    10,
                    HashMap::from([
                        ("name".to_string(), "buzz".to_string()),
                        ("affects".to_string(), "file1:bar".to_string()),
                    ]),
                    "".to_string(),
                ))],
            ),
        ]);

        let violations = validator
            .validate(Arc::new(validators::Context::new(modified_blocks)))
            .await?;

        assert!(violations.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn with_multiple_references_and_some_missing_returns_violations() -> anyhow::Result<()> {
        let validator = AffectsValidator::new();
        let modified_blocks = HashMap::from([
            (
                "file1".to_string(),
                vec![
                    Arc::new(Block::new(
                        1,
                        10,
                        HashMap::from([
                            ("name".to_string(), "foo".to_string()),
                            ("affects".to_string(), ":bar, file2:buzz".to_string()),
                        ]),
                        "".to_string(),
                    )),
                    Arc::new(Block::new(
                        12,
                        16,
                        HashMap::from([
                            ("name".to_string(), "bar".to_string()),
                            ("affects".to_string(), ":foo".to_string()),
                        ]),
                        "".to_string(),
                    )),
                ],
            ),
            (
                "file2".to_string(),
                vec![Arc::new(Block::new(
                    1,
                    10,
                    HashMap::from([
                        ("name".to_string(), "not-buzz".to_string()),
                        ("affects".to_string(), "file1:bar".to_string()),
                    ]),
                    "".to_string(),
                ))],
            ),
        ]);

        let violations = validator
            .validate(Arc::new(validators::Context::new(modified_blocks)))
            .await?;

        assert_eq!(violations.len(), 1);
        assert_eq!(violations.get("file1").unwrap().len(), 1);
        assert_eq!(
            violations.get("file1").unwrap()[0].error,
            "Block file1:foo at line 1 is modified, but file2:buzz is not"
        );
        Ok(())
    }

    #[tokio::test]
    async fn with_no_missing_blocks_returns_ok() -> anyhow::Result<()> {
        let validator = AffectsValidator::new();
        let modified_blocks = HashMap::from([
            (
                "file1".to_string(),
                vec![
                    Arc::new(Block::new(
                        1,
                        10,
                        HashMap::from([("affects".to_string(), "file2:foo".to_string())]),
                        "".to_string(),
                    )),
                    Arc::new(Block::new(
                        12,
                        16,
                        HashMap::from([("affects".to_string(), "file3:bar".to_string())]),
                        "".to_string(),
                    )),
                ],
            ),
            (
                "file2".to_string(),
                vec![Arc::new(Block::new(
                    1,
                    10,
                    HashMap::from([("name".to_string(), "foo".to_string())]),
                    "".to_string(),
                ))],
            ),
            (
                "file3".to_string(),
                vec![Arc::new(Block::new(
                    1,
                    10,
                    HashMap::from([("name".to_string(), "bar".to_string())]),
                    "".to_string(),
                ))],
            ),
        ]);

        let violations = validator
            .validate(Arc::new(validators::Context::new(modified_blocks)))
            .await?;

        assert!(violations.is_empty());
        Ok(())
    }
}

fn create_violation(
    modified_block_file_path: &str,
    modified_block: &Block,
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
    Ok(Violation::new(
        "affects".to_string(),
        message,
        Some(
            serde_json::to_value(AffectsViolation { modified_block })
                .context("failed to serialize AffectsViolation block")?,
        ),
    ))
}

fn parse_affects_attribute(value: &str) -> anyhow::Result<Vec<(Option<String>, String)>> {
    let mut result = Vec::new();
    for block_ref in value.split(",") {
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
