//! Attribute values for `keep-sorted` and `keep-sorted-format`.

use super::parse_keyword_value;
use serde::Serialize;
use strum_macros::{Display, EnumIter, EnumString};

/// The direction of a `keep-sorted` block.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq, EnumString, EnumIter, Display, Serialize)]
#[strum(ascii_case_insensitive, serialize_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub(crate) enum SortOrder {
    #[default]
    Asc,
    Desc,
}

impl SortOrder {
    /// Parses a `keep-sorted` attribute value after trimming whitespace. An empty value defaults to
    /// ascending order.
    pub(super) fn parse(raw: &str) -> anyhow::Result<Self> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            Ok(Self::default())
        } else {
            parse_keyword_value(trimmed)
        }
    }
}

/// How values are compared when sorting a `keep-sorted` block.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq, EnumString, EnumIter, Display, Serialize)]
#[strum(ascii_case_insensitive, serialize_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub(crate) enum SortFormat {
    #[default]
    Lexicographic,
    Numeric,
}

impl SortFormat {
    /// Parses a `keep-sorted-format` attribute value after trimming whitespace. An empty value
    /// defaults to lexicographic order.
    pub(super) fn parse(raw: &str) -> anyhow::Result<Self> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            Ok(Self::default())
        } else {
            parse_keyword_value(trimmed)
        }
    }
}
