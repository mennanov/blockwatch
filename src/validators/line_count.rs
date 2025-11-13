use crate::blocks::{Block, BlockWithContext};
use crate::validators::{
    ValidatorDetector, ValidatorSync, ValidatorType, Violation, ViolationRange,
};
use crate::{Position, validators};
use anyhow::anyhow;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;

pub(crate) struct LineCountValidator {}

impl LineCountValidator {
    pub(super) fn new() -> Self {
        Self {}
    }
}

#[derive(Serialize)]
struct LineCountViolation {
    actual: usize,
    op: String,
    expected: usize,
}

impl ValidatorSync for LineCountValidator {
    fn validate(
        &self,
        context: Arc<validators::ValidationContext>,
    ) -> anyhow::Result<HashMap<String, Vec<Violation>>> {
        let mut violations = HashMap::new();
        for (file_path, file_blocks) in &context.modified_blocks {
            for block_with_context in &file_blocks.blocks_with_context {
                let Some(expr) = block_with_context.block.attributes.get("line-count") else {
                    continue;
                };
                let (op, expected) = parse_constraint(expr).map_err(|e| anyhow!(
                    "line-count expected a comparator like <N, <=N, ==N, >=N, >N; got \"{}\" in {}:{} at line {} (error: {})",
                    expr,
                    file_path,
                    block_with_context.block.name_display(),
                    block_with_context.block.starts_at_line,
                    e
                ))?;
                let actual = if block_with_context
                    .block
                    .content(&file_blocks.file_content)
                    .is_empty()
                {
                    0
                } else {
                    block_with_context
                        .block
                        .content(&file_blocks.file_content)
                        .lines()
                        .filter(|line| !line.trim().is_empty())
                        .count()
                };
                let ok = match op {
                    Op::Lt => actual < expected,
                    Op::Le => actual <= expected,
                    Op::Eq => actual == expected,
                    Op::Ge => actual >= expected,
                    Op::Gt => actual > expected,
                };
                if !ok {
                    violations
                        .entry(file_path.clone())
                        .or_insert_with(Vec::new)
                        .push(create_violation(
                            file_path,
                            Arc::clone(&block_with_context.block),
                            &file_blocks.file_content_new_lines,
                            op,
                            expected,
                            actual,
                        )?);
                }
            }
        }
        Ok(violations)
    }
}

fn create_violation(
    block_file_path: &str,
    block: Arc<Block>,
    new_line_positions: &[usize],
    operation: Op,
    expected: usize,
    actual: usize,
) -> anyhow::Result<Violation> {
    let message = format!(
        "Block {}:{} defined at line {} has {} lines, which does not satisfy {}{}",
        block_file_path,
        block.name_display(),
        block.starts_at_line,
        actual,
        operation.as_str(),
        expected
    );
    Ok(Violation::new(
        ViolationRange::new(
            Position::from_byte_offset(block.start_tag_range.start, new_line_positions),
            Position::from_byte_offset(block.start_tag_range.end - 1, new_line_positions),
        ),
        "line-count".to_string(),
        message,
        block,
        Some(serde_json::to_value(LineCountViolation {
            actual,
            op: operation.as_str().to_string(),
            expected,
        })?),
    ))
}

pub(crate) struct LineCountValidatorDetector();

impl LineCountValidatorDetector {
    pub fn new() -> Self {
        Self {}
    }
}

impl ValidatorDetector for LineCountValidatorDetector {
    fn detect(
        &self,
        block_with_context: &BlockWithContext,
    ) -> anyhow::Result<Option<ValidatorType>> {
        if block_with_context
            .block
            .attributes
            .contains_key("line-count")
        {
            Ok(Some(ValidatorType::Sync(Box::new(
                LineCountValidator::new(),
            ))))
        } else {
            Ok(None)
        }
    }
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
    use crate::blocks::{Block, FileBlocks};
    use crate::test_utils;
    use crate::test_utils::{block_with_context_default, new_line_positions};
    use serde_json::json;

    #[test]
    fn parse_constraint_with_valid_syntax_returns_correct_result() {
        assert!(matches!(parse_constraint("< 50").unwrap(), (Op::Lt, 50)));
        assert!(matches!(parse_constraint(">=10").unwrap(), (Op::Ge, 10)));
        assert!(matches!(parse_constraint("== 0").unwrap(), (Op::Eq, 0)));
    }

    #[test]
    fn parse_constraint_with_invalid_syntax_returns_error() {
        assert!(parse_constraint("50").is_err());
        assert!(parse_constraint("").is_err());
        assert!(parse_constraint("> -1").is_err());
        assert!(parse_constraint("<== 50").is_err());
    }

    #[test]
    fn validate_with_correct_number_of_lines_returns_no_violations() -> anyhow::Result<()> {
        let validator = LineCountValidator::new();
        let file1_contents = "/*<block>*/blocks content goes here: a\nb\nc\nd//</block>";
        let context = Arc::new(validators::ValidationContext::new(HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_content: file1_contents.to_string(),
                file_content_new_lines: new_line_positions(file1_contents),
                blocks_with_context: vec![
                    block_with_context_default(Block::new(
                        1,
                        4,
                        HashMap::from([("line-count".to_string(), "<3".to_string())]),
                        test_utils::substr_range(file1_contents, "<block>"),
                        test_utils::substr_range(file1_contents, "a\nb"),
                    )),
                    block_with_context_default(Block::new(
                        5,
                        8,
                        HashMap::from([("line-count".to_string(), "<=3".to_string())]),
                        test_utils::substr_range(file1_contents, "<block>"),
                        test_utils::substr_range(file1_contents, "a\nb"),
                    )),
                    block_with_context_default(Block::new(
                        9,
                        13,
                        HashMap::from([("line-count".to_string(), "<=3".to_string())]),
                        test_utils::substr_range(file1_contents, "<block>"),
                        test_utils::substr_range(file1_contents, "a\nb\nc"),
                    )),
                    block_with_context_default(Block::new(
                        15,
                        18,
                        HashMap::from([("line-count".to_string(), "== 2".to_string())]),
                        test_utils::substr_range(file1_contents, "<block>"),
                        test_utils::substr_range(file1_contents, "a\nb"),
                    )),
                    block_with_context_default(Block::new(
                        20,
                        23,
                        HashMap::from([("line-count".to_string(), ">= 2".to_string())]),
                        test_utils::substr_range(file1_contents, "<block>"),
                        test_utils::substr_range(file1_contents, "a\nb"),
                    )),
                    block_with_context_default(Block::new(
                        30,
                        34,
                        HashMap::from([("line-count".to_string(), ">= 2".to_string())]),
                        test_utils::substr_range(file1_contents, "<block>"),
                        test_utils::substr_range(file1_contents, "a\nb\nc"),
                    )),
                    block_with_context_default(Block::new(
                        40,
                        45,
                        HashMap::from([("line-count".to_string(), "> 3".to_string())]),
                        test_utils::substr_range(file1_contents, "<block>"),
                        test_utils::substr_range(file1_contents, "a\nb\nc\nd"),
                    )),
                ],
            },
        )])));
        let violations = validator.validate(context)?;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn validate_with_incorrect_number_of_lines_returns_violations() -> anyhow::Result<()> {
        let validator = LineCountValidator::new();
        let file1_contents = "/*<block>*/blocks content goes here: a\nb\nc\nd //</block>";
        let context = Arc::new(validators::ValidationContext::new(HashMap::from([(
            "file2".to_string(),
            FileBlocks {
                file_content: file1_contents.to_string(),
                file_content_new_lines: new_line_positions(file1_contents),
                blocks_with_context: vec![
                    block_with_context_default(Block::new(
                        1,
                        5,
                        HashMap::from([("line-count".to_string(), "<3".to_string())]),
                        test_utils::substr_range(file1_contents, "<block>"),
                        test_utils::substr_range(file1_contents, "a\nb\nc"),
                    )),
                    block_with_context_default(Block::new(
                        7,
                        12,
                        HashMap::from([("line-count".to_string(), "<=3".to_string())]),
                        test_utils::substr_range(file1_contents, "<block>"),
                        test_utils::substr_range(file1_contents, "a\nb\nc\nd"),
                    )),
                    block_with_context_default(Block::new(
                        14,
                        19,
                        HashMap::from([("line-count".to_string(), "==3".to_string())]),
                        test_utils::substr_range(file1_contents, "<block>"),
                        test_utils::substr_range(file1_contents, "a\nb\nc\nd"),
                    )),
                    block_with_context_default(Block::new(
                        20,
                        23,
                        HashMap::from([("line-count".to_string(), "==3".to_string())]),
                        test_utils::substr_range(file1_contents, "<block>"),
                        test_utils::substr_range(file1_contents, "a\nb"),
                    )),
                    block_with_context_default(Block::new(
                        25,
                        28,
                        HashMap::from([("line-count".to_string(), ">=3".to_string())]),
                        test_utils::substr_range(file1_contents, "<block>"),
                        test_utils::substr_range(file1_contents, "a\nb"),
                    )),
                    block_with_context_default(Block::new(
                        25,
                        28,
                        HashMap::from([("line-count".to_string(), ">3".to_string())]),
                        test_utils::substr_range(file1_contents, "<block>"),
                        test_utils::substr_range(file1_contents, "a\nb\nc"),
                    )),
                ],
            },
        )])));

        let violations = validator.validate(context)?;

        assert_eq!(violations.len(), 1);
        let file2_violations = violations.get("file2").unwrap();
        assert_eq!(file2_violations.len(), 6);
        assert_eq!(file2_violations[0].code, "line-count");
        assert_eq!(
            file2_violations[0].message,
            "Block file2:(unnamed) defined at line 1 has 3 lines, which does not satisfy <3"
        );
        assert_eq!(
            file2_violations[0].data,
            Some(json!({
                "actual": 3,
                "op": "<",
                "expected": 3,
            }))
        );

        assert_eq!(file2_violations[1].code, "line-count");
        assert_eq!(
            file2_violations[1].message,
            "Block file2:(unnamed) defined at line 7 has 4 lines, which does not satisfy <=3"
        );
        assert_eq!(
            file2_violations[1].data,
            Some(json!({
                "actual": 4,
                "op": "<=",
                "expected": 3,
            }))
        );

        assert_eq!(file2_violations[2].code, "line-count");
        assert_eq!(
            file2_violations[2].message,
            "Block file2:(unnamed) defined at line 14 has 4 lines, which does not satisfy ==3"
        );
        assert_eq!(
            file2_violations[2].data,
            Some(json!({
                "actual": 4,
                "op": "==",
                "expected": 3,
            }))
        );

        assert_eq!(file2_violations[3].code, "line-count");
        assert_eq!(
            file2_violations[3].message,
            "Block file2:(unnamed) defined at line 20 has 2 lines, which does not satisfy ==3"
        );
        assert_eq!(
            file2_violations[3].data,
            Some(json!({
                "actual": 2,
                "op": "==",
                "expected": 3,
            }))
        );

        assert_eq!(file2_violations[4].code, "line-count");
        assert_eq!(
            file2_violations[4].message,
            "Block file2:(unnamed) defined at line 25 has 2 lines, which does not satisfy >=3"
        );
        assert_eq!(
            file2_violations[4].data,
            Some(json!({
                "actual": 2,
                "op": ">=",
                "expected": 3,
            }))
        );

        assert_eq!(file2_violations[5].code, "line-count");
        assert_eq!(
            file2_violations[5].message,
            "Block file2:(unnamed) defined at line 25 has 3 lines, which does not satisfy >3"
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
    fn empty_lines_and_lines_with_spaces_only_are_ignored() -> anyhow::Result<()> {
        let validator = LineCountValidator::new();
        let file1_contents = "/*<block>*/blocks content goes here: a\n\nb\nc\n \n \nd//</block>";
        let context = Arc::new(validators::ValidationContext::new(HashMap::from([(
            "file1".to_string(),
            FileBlocks {
                file_content: file1_contents.to_string(),
                file_content_new_lines: new_line_positions(file1_contents),
                blocks_with_context: vec![block_with_context_default(Block::new(
                    1,
                    4,
                    HashMap::from([("line-count".to_string(), "<=4".to_string())]),
                    test_utils::substr_range(file1_contents, "<block>"),
                    test_utils::substr_range(file1_contents, "a\n\nb\nc\n \n \nd"),
                ))],
            },
        )])));
        let violations = validator.validate(context)?;
        assert!(violations.is_empty());
        Ok(())
    }
}
