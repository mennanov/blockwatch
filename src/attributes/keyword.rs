//! Keywords, the values of an attribute that can take only a few words, such as `asc` or `desc`.

use anyhow::anyhow;
use std::str::FromStr;
use strum::IntoEnumIterator;

/// Parses `value` as one of the keywords of `T`.
///
/// # Errors
/// Returns `"\"value\" (expected ...)"` when `value` does not match a variant of `T`.
#[expect(
    clippy::map_err_ignore,
    reason = "strum's error only says that no keyword matched, and the message lists the keywords"
)]
pub(crate) fn parse_keyword_value<T>(value: &str) -> anyhow::Result<T>
where
    T: FromStr + IntoEnumIterator + ToString,
{
    T::from_str(value).map_err(|_| {
        let keywords: Vec<String> = T::iter().map(|keyword| keyword.to_string()).collect();
        let (last, others) = keywords
            .split_last()
            .expect("a keyword type has at least one keyword");
        anyhow!("\"{value}\" (expected {} or {last})", others.join(", "))
    })
}
