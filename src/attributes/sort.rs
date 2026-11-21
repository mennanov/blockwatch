//! Attribute values for `keep-sorted` and `keep-sorted-format`.

use super::parse_number;
use anyhow::anyhow;
use serde::Serialize;
use std::cmp::Ordering;
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
    /// The order of `a` and `b` under this format.
    ///
    /// # Errors
    /// Returns an error under [`SortFormat::Numeric`] when `a` or `b` is not a number.
    pub(crate) fn compare(self, a: &str, b: &str) -> anyhow::Result<Ordering> {
        match self {
            Self::Lexicographic => Ok(a.cmp(b)),
            Self::Numeric => {
                let a_num =
                    parse_number(a).ok_or_else(|| anyhow!("\"{a}\" is not a valid number"))?;
                let b_num =
                    parse_number(b).ok_or_else(|| anyhow!("\"{b}\" is not a valid number"))?;
                Ok(a_num.cmp(&b_num))
            }
        }
    }
}
