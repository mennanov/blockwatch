use crate::blocks::{Block, BlockWithContext};
use crate::fs::FileSystem;
use crate::validators;
use crate::validators::{
    ValidationReport, ValidatorDetector, ValidatorSync, ValidatorType, Violation, ViolationRange,
};
use anyhow::anyhow;
use serde::Serialize;
use std::path::Path;
use std::sync::Arc;

/// Enforces `line-count="<N"` and friends: the number of non-empty lines in the block must satisfy
/// the given comparison.
///
/// Useful for keeping a section from silently growing past the size it was designed for — a code
/// sample that must stay readable, or a list with a hard limit.
pub(crate) struct LineCountValidator {}

impl LineCountValidator {
    /// Creates the validator. It is stateless; all input arrives through the validation context.
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
    ) -> anyhow::Result<ValidationReport> {
        let mut report = ValidationReport::default();
        for (file_path, file_blocks) in &context.blocks {
            for block_with_context in &file_blocks.blocks_with_context {
                let Some(expr) = block_with_context.block.attributes.get("line-count") else {
                    continue;
                };
                let (op, expected) = parse_constraint(expr).map_err(|e| anyhow!(
                    "line-count expected a comparator like <N, <=N, ==N, >=N, >N; got \"{}\" in {}:{} at line {} (error: {})",
                    expr,
                    file_path.display(),
                    block_with_context.block.name_display(),
                    block_with_context.block.start_tag_position_range.start().line,
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
                let block_violations = if ok {
                    Vec::new()
                } else {
                    vec![create_violation(
                        file_path,
                        &block_with_context.block,
                        op,
                        expected,
                        actual,
                    )?]
                };
                report.add_all(file_path, &block_with_context.block, block_violations);
            }
        }
        Ok(report)
    }
}

fn create_violation(
    block_file_path: &Path,
    block: &Block,
    operation: Op,
    expected: usize,
    actual: usize,
) -> anyhow::Result<Violation> {
    let message = format!(
        "Block {}:{} defined at line {} has {} lines, which does not satisfy {}{}",
        block_file_path.display(),
        block.name_display(),
        block.start_tag_position_range.start().line,
        actual,
        operation.as_str(),
        expected
    );
    Ok(Violation::new(
        ViolationRange::new(
            block.start_tag_position_range.start().clone(),
            block.start_tag_position_range.end().clone(),
        ),
        "line-count".to_string(),
        message,
        block.severity()?,
        Some(serde_json::to_value(LineCountViolation {
            actual,
            op: operation.as_str().to_string(),
            expected,
        })?),
    ))
}

/// Selects [`LineCountValidator`] for blocks carrying a `line-count` attribute.
pub(crate) struct LineCountValidatorDetector();

impl LineCountValidatorDetector {
    /// Creates the detector. Registered in [`crate::validators::detector_factories`].
    pub fn new() -> Self {
        Self {}
    }
}

impl<Fs: FileSystem> ValidatorDetector<Fs> for LineCountValidatorDetector {
    fn detect(
        &self,
        block_with_context: &BlockWithContext,
        _file_system: &Arc<Fs>,
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
    use crate::repo_path::RepoPath;
    use crate::test_utils::{checked_lines, validation_context, violation_count};
    use serde_json::json;

    #[test]
    fn validate_with_incorrect_number_of_lines_returns_violations() -> anyhow::Result<()> {
        let validator = LineCountValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block line-count="<3">
        a
        b
        c
        # </block>
        # <block line-count="<=3">
        a
        b
        c
        d
        # </block>
        # <block line-count="==3">
        a
        b
        c
        d
        # </block>
        # <block line-count="==3">
        a
        b
        # </block>
        # <block line-count=">=3">
        a
        b
        # </block>
        # <block line-count=">3">
        a
        b
        c
        # </block>"#,
        );

        let violations = validator.validate(context)?.violations;

        assert_eq!(violations.len(), 1);
        let file2_violations = violations
            .get(&RepoPath::from_reference("example.py")?)
            .unwrap();
        assert_eq!(file2_violations.len(), 6);
        assert_eq!(file2_violations[0].code, "line-count");
        assert_eq!(
            file2_violations[0].message,
            "Block example.py:(unnamed) defined at line 1 has 3 lines, which does not satisfy <3"
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
            "Block example.py:(unnamed) defined at line 6 has 4 lines, which does not satisfy <=3"
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
            "Block example.py:(unnamed) defined at line 12 has 4 lines, which does not satisfy ==3"
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
            "Block example.py:(unnamed) defined at line 18 has 2 lines, which does not satisfy ==3"
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
            "Block example.py:(unnamed) defined at line 22 has 2 lines, which does not satisfy >=3"
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
            "Block example.py:(unnamed) defined at line 26 has 3 lines, which does not satisfy >3"
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
    fn validate_with_correct_number_of_lines_returns_no_violations() -> anyhow::Result<()> {
        let validator = LineCountValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block line-count="<3">
        a
        b
        # </block>
        # <block line-count="<=3">
        a
        b
        # </block>
        # <block line-count="<=3">
        a
        b
        c
        # </block>
        # <block line-count="== 2">
        a
        b
        # </block>
        # <block line-count=">= 2">
        a
        b
        # </block>
        # <block line-count=">= 2">
        a
        b
        c
        # </block>
        # <block line-count="> 3">
        a
        b
        c
        d
        # </block>"#,
        );
        let violations = validator.validate(context)?.violations;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn validate_with_blank_and_whitespace_only_lines_returns_no_violations() -> anyhow::Result<()> {
        let validator = LineCountValidator::new();
        let context = validation_context(
            "example.py",
            r#"# <block line-count="<=4">
        a
        
        b
        c
         
         
        d
        # </block>"#,
        );
        let violations = validator.validate(context)?.violations;
        assert!(violations.is_empty());
        Ok(())
    }

    #[test]
    fn parse_constraint_with_invalid_syntax_returns_error() {
        assert!(parse_constraint("50").is_err());
        assert!(parse_constraint("").is_err());
        assert!(parse_constraint("> -1").is_err());
        assert!(parse_constraint("<== 50").is_err());
    }

    #[test]
    fn parse_constraint_with_valid_syntax_returns_correct_result() {
        assert!(matches!(parse_constraint("< 50").unwrap(), (Op::Lt, 50)));
        assert!(matches!(parse_constraint(">=10").unwrap(), (Op::Ge, 10)));
        assert!(matches!(parse_constraint("== 0").unwrap(), (Op::Eq, 0)));
    }

    #[test]
    fn validate_with_blocks_without_line_count_records_a_check_for_the_examined_ones_only()
    -> anyhow::Result<()> {
        let context = validation_context(
            "example.py",
            r#"# <block name="within" line-count="<=2">
a = 1
# </block>
# <block name="over" line-count="<=1">
a = 1
b = 2
# </block>
# <block name="unrelated">
c = 3
# </block>"#,
        );

        let report = LineCountValidator::new().validate(context)?;

        // The block without a line-count attribute is not checked, so it records nothing.
        assert_eq!(checked_lines(&report), vec![1, 4]);
        assert_eq!(violation_count(&report), 1);
        Ok(())
    }
}
