use crate::blocks::{Block, BlockWithContext};
use crate::validators::{ValidatorType, Violation, ViolationRange};
use crate::{Position, validators};
use anyhow::Context;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(crate) struct AffectsValidator {}

impl AffectsValidator {
    pub(super) fn new() -> Self {
        Self {}
    }
}

#[derive(Serialize)]
struct AffectsViolation<'a> {
    affected_block_file_path: &'a Path,
    affected_block_name: &'a str,
}

impl validators::ValidatorSync for AffectsValidator {
    fn validate(
        &self,
        context: Arc<validators::ValidationContext>,
    ) -> anyhow::Result<HashMap<PathBuf, Vec<Violation>>> {
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
                                    &block_with_context.block,
                                    &file_blocks.file_content_new_lines,
                                    &affected_file_path,
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
    modified_block_file_path: &Path,
    modified_block: &Block,
    modified_block_new_line_positions: &[usize],
    affected_block_file_path: &Path,
    affected_block_name: &str,
) -> anyhow::Result<Violation> {
    let message = format!(
        "Block {}:{} at line {} is modified, but {}:{} is not",
        modified_block_file_path.display(),
        modified_block.name_display(),
        modified_block.starts_at_line,
        affected_block_file_path.display(),
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
        modified_block.severity()?,
        Some(details),
    ))
}

fn parse_affects_attribute(value: &str) -> anyhow::Result<Vec<(Option<PathBuf>, String)>> {
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
                Some(filename.into())
            },
            block_name.trim().to_string(),
        ));
    }
    Ok(result)
}

#[cfg(test)]
mod validate_tests {
    use super::*;
    use crate::diff_parser::LineChange;
    use crate::test_utils::{
        merge_validation_contexts, validation_context, validation_context_with_changes,
    };
    use crate::validators::ValidatorSync;

    #[test]
    fn no_blocks_with_affects_attr_returns_ok() -> anyhow::Result<()> {
        let validator = AffectsValidator::new();
        let context = validation_context(
            "file1.py",
            r#"# <block name="foo">
pass
# </block>
"#,
        );

        let violations = validator.validate(context)?;

        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn with_missing_blocks_in_same_file_returns_violations() -> anyhow::Result<()> {
        let validator = AffectsValidator::new();
        let context = validation_context_with_changes(
            "file1.py",
            r#"# <block affects=":foo">
print("first")
# </block>

# <block name="foo">
print("second")
# </block>
"#,
            vec![LineChange {
                line: 2,
                ranges: None,
            }],
        );

        let violations = validator.validate(context)?;

        assert_eq!(violations.len(), 1);
        let file1_violations = violations.get(&PathBuf::from("file1.py")).unwrap();
        assert_eq!(file1_violations.len(), 1);
        assert_eq!(
            file1_violations[0].message,
            "Block file1.py:(unnamed) at line 1 is modified, but file1.py:foo is not"
        );

        Ok(())
    }

    #[test]
    fn with_missing_blocks_in_different_files_returns_violations() -> anyhow::Result<()> {
        let validator = AffectsValidator::new();
        let context = merge_validation_contexts(vec![
            validation_context(
                "file1.py",
                r#"# <block affects="file2.py:foo">
print("first")
# </block>

# <block affects="file3.py:bar">
print("second")
# </block>
"#,
            ),
            validation_context_with_changes(
                "file2.py",
                r#"# <block name="foo">
print("file2")
# </block>
"#,
                vec![LineChange {
                    line: 1, // Only the start tag is changed, not the content.
                    ranges: Some(vec![3..8, 10..15]),
                }],
            ),
            validation_context_with_changes(
                "file3.py",
                r#"# <block name="not-bar">
print("file3")
# </block>
"#,
                vec![LineChange {
                    line: 3, // Only the end tag is modified, not the content.
                    ranges: None,
                }],
            ),
        ]);

        let violations = validator.validate(context)?;

        assert_eq!(violations.len(), 1);
        let file1_violations = violations.get(&PathBuf::from("file1.py")).unwrap();
        assert_eq!(file1_violations.len(), 2);
        assert_eq!(
            file1_violations[0].message,
            "Block file1.py:(unnamed) at line 1 is modified, but file2.py:foo is not"
        );
        assert_eq!(
            file1_violations[1].message,
            "Block file1.py:(unnamed) at line 5 is modified, but file3.py:bar is not"
        );

        Ok(())
    }

    #[test]
    fn with_cyclic_references_returns_ok() -> anyhow::Result<()> {
        let validator = AffectsValidator::new();
        let context = validation_context(
            "file1.py",
            r#"# <block name="foo" affects=":bar">
print("foo")
# </block>

# <block name="bar" affects=":foo">
print("bar")
# </block>
"#,
        );

        let violations = validator.validate(context)?;

        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn with_multiple_references_returns_ok() -> anyhow::Result<()> {
        let validator = AffectsValidator::new();
        let context = merge_validation_contexts(vec![
            validation_context(
                "file1.py",
                r#"# <block name="foo" affects=":bar, file2.py:buzz">
print("foo")
# </block>

# <block name="bar" affects=":foo">
print("bar")
# </block>
"#,
            ),
            validation_context(
                "file2.py",
                r#"# <block name="buzz" affects="file1.py:bar">
print("buzz")
# </block>
"#,
            ),
        ]);

        let violations = validator.validate(context)?;

        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn with_multiple_references_and_some_missing_returns_violations() -> anyhow::Result<()> {
        let validator = AffectsValidator::new();
        let context = merge_validation_contexts(vec![
            validation_context(
                "file1.py",
                r#"# <block name="foo" affects=":bar, file2.py:buzz">
print("foo")
# </block>

# <block name="bar" affects=":foo">
print("bar")
# </block>
"#,
            ),
            validation_context_with_changes(
                "file2.py",
                r#"# <block name="buzz" affects="file1.py:bar">
print("not-buzz")
# </block>
print("hello")
"#,
                vec![LineChange {
                    line: 4, // Line outside the block is changed.
                    ranges: None,
                }],
            ),
        ]);

        let violations = validator.validate(context)?;

        assert_eq!(violations.len(), 1);
        let file1_violations = violations.get(&PathBuf::from("file1.py")).unwrap();
        assert_eq!(file1_violations.len(), 1);
        assert_eq!(
            file1_violations[0].message,
            "Block file1.py:foo at line 1 is modified, but file2.py:buzz is not"
        );
        Ok(())
    }

    #[test]
    fn with_no_missing_blocks_returns_ok() -> anyhow::Result<()> {
        let validator = AffectsValidator::new();
        let context = merge_validation_contexts(vec![
            validation_context(
                "file1.py",
                r#"# <block affects="file2.py:foo">
print("first")
# </block>

# <block affects="file3.py:bar">
print("second")
# </block>
"#,
            ),
            validation_context(
                "file2.py",
                r#"# <block name="foo">
print("foo")
# </block>
"#,
            ),
            validation_context(
                "file3.py",
                r#"# <block name="bar">
print("bar")
# </block>
"#,
            ),
        ]);

        let violations = validator.validate(context)?;

        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn blocks_with_unmodified_content_returns_ok() -> anyhow::Result<()> {
        let validator = AffectsValidator::new();
        let context = merge_validation_contexts(vec![
            validation_context_with_changes(
                "file1.py",
                r#"# <block affects="file2.py:foo">
pass
# </block>

# <block affects="file3.py:bar">
pass
# </block>
"#,
                vec![
                    LineChange {
                        line: 1,
                        ranges: Some(vec![0..10, 12..15]),
                    }, // First block start tag
                    LineChange {
                        line: 7,
                        ranges: None,
                    }, // Second block end tag
                ],
            ),
            validation_context_with_changes(
                "file2.py",
                r#"# <block name="foo">
pass
# </block>
"#,
                vec![LineChange {
                    line: 1,
                    ranges: Some(vec![0..4, 6..10]),
                }], // Only start tag modified
            ),
            validation_context_with_changes(
                "file3.py",
                r#"# <block name="bar">
pass
# </block>
"#,
                vec![LineChange {
                    line: 3,
                    ranges: None,
                }], // Only end tag modified
            ),
        ]);

        let violations = validator.validate(context)?;

        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn dependent_blocks_with_unmodified_content_returns_violations() -> anyhow::Result<()> {
        use crate::diff_parser::LineChange;
        use crate::test_utils::validation_context_with_changes;

        let validator = AffectsValidator::new();
        let contents = r#"# <block name="foo" affects=":bar">
print("foo")
# </block>

# <block name="bar" affects=":foo">
pass
# </block>
"#;
        let line_changes = vec![
            LineChange {
                line: 2,
                ranges: None,
            }, // First block's content line
            LineChange {
                line: 4, // Not in any of the blocks.
                ranges: None,
            },
        ];
        let context = validation_context_with_changes("file1.py", contents, line_changes);

        let violations = validator.validate(context)?;

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
            vec![(Some("file.rs".into()), "block_name".to_string())]
        );
        Ok(())
    }

    #[test]
    fn multiple_references() -> anyhow::Result<()> {
        let result = parse_affects_attribute("file1.rs:block1, file2.rs:block2")?;
        assert_eq!(
            result,
            vec![
                (Some("file1.rs".into()), "block1".to_string()),
                (Some("file2.rs".into()), "block2".to_string())
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
