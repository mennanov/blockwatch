//! The `severity` attribute value.

use super::parse_keyword_value;
use serde_repr::Serialize_repr;
use strum_macros::{Display, EnumIter, EnumString};

/// Block's severity.
///
/// Mirrors [LSP DiagnosticSeverity](https://github.com/microsoft/vscode-languageserver-node/blob/3412a17149850f445bf35b4ad71148cfe5f8411e/types/src/main.ts#L614)
#[derive(
    Default, Clone, Copy, Serialize_repr, EnumString, EnumIter, Display, Debug, PartialEq, Eq,
)]
#[strum(ascii_case_insensitive, serialize_all = "lowercase")]
#[repr(u8)]
/* <block name="block-severity" same-as="docs/validators/README.md:severity-levels"
same-as-format="identifier" same-as-pattern="^(?P<value>[A-Z][a-z]+) = \d"> */
pub(crate) enum Severity {
    /// The default. A violation at this level makes the run exit non-zero, failing a hook or CI.
    #[default]
    Error = 1,
    /// Reported like an error but does not affect the exit code.
    Warning = 2,
    /// Reported for information only; does not affect the exit code.
    Info = 3,
    /// The weakest level, for suggestions, does not affect the exit code.
    Hint = 4,
}
// </block>

impl Severity {
    /// Parses `raw` as a severity level after trimming surrounding whitespace.
    pub(super) fn parse(raw: &str) -> anyhow::Result<Self> {
        parse_keyword_value(raw.trim())
    }
}

#[cfg(test)]
mod tests {
    use super::Severity;

    #[test]
    fn known_severity_parse_returns_it() -> anyhow::Result<()> {
        assert_eq!(Severity::parse("warning")?, Severity::Warning);
        Ok(())
    }

    #[test]
    fn unknown_severity_parse_returns_error_with_the_known_ones() {
        let error = Severity::parse("warn").unwrap_err();

        assert_eq!(
            error.to_string(),
            "\"warn\" (expected error, warning, info or hint)"
        );
    }

    #[test]
    fn mixed_case_severity_parse_returns_it() -> anyhow::Result<()> {
        assert_eq!(Severity::parse("InFo")?, Severity::Info);
        Ok(())
    }

    #[test]
    fn severity_with_surrounding_spaces_parse_returns_it() -> anyhow::Result<()> {
        assert_eq!(Severity::parse(" warning ")?, Severity::Warning);
        Ok(())
    }
}
