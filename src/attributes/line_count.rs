//! The `line-count` attribute value.

use anyhow::anyhow;
use std::fmt;

/// A comparator and count from `line-count`, such as `<50` or `== 0`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct Constraint {
    op: Op,
    expected: usize,
}

impl Constraint {
    /// Parses `raw` as a line-count constraint such as `<50` or `== 0`.
    pub(super) fn parse(raw: &str) -> anyhow::Result<Self> {
        let (op, expected) = parse_constraint(raw).map_err(|e| {
            anyhow!("expected a comparator like <N, <=N, ==N, >=N, >N; got \"{raw}\" (error: {e})")
        })?;
        Ok(Self { op, expected })
    }

    /// Returns whether `actual` lines satisfy this rule.
    pub(crate) fn accepts(self, actual: usize) -> bool {
        self.op.accepts(actual, self.expected)
    }

    /// The operator as written in diagnostics and traces, such as `<=`.
    pub(crate) fn op_str(self) -> &'static str {
        self.op.as_str()
    }

    /// The target line count to compare against.
    pub(crate) fn expected(self) -> usize {
        self.expected
    }
}

impl fmt::Display for Constraint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.op.as_str(), self.expected)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
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

    /// Returns whether `actual` satisfies the rule this operator forms with `expected`.
    fn accepts(self, actual: usize, expected: usize) -> bool {
        match self {
            Op::Lt => actual < expected,
            Op::Le => actual <= expected,
            Op::Eq => actual == expected,
            Op::Ge => actual >= expected,
            Op::Gt => actual > expected,
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

    #[test]
    fn parse_constraint_with_invalid_syntax_returns_error() {
        assert!(Constraint::parse("50").is_err());
        assert!(Constraint::parse("").is_err());
        assert!(Constraint::parse("> -1").is_err());
        assert!(Constraint::parse("<== 50").is_err());
    }

    #[test]
    fn parse_constraint_with_valid_syntax_returns_correct_result() {
        assert_eq!(
            Constraint::parse("< 50").unwrap(),
            Constraint {
                op: Op::Lt,
                expected: 50
            }
        );
        assert_eq!(
            Constraint::parse(">=10").unwrap(),
            Constraint {
                op: Op::Ge,
                expected: 10
            }
        );
        assert_eq!(
            Constraint::parse("== 0").unwrap(),
            Constraint {
                op: Op::Eq,
                expected: 0
            }
        );
    }
}
