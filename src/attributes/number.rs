//! Numbers as the `numeric` format of `keep-sorted-format` and `same-as-format` reads them.

use bigdecimal::BigDecimal;

/// Parses a numeric value from string.
///
/// Supports very big numbers (bigger than `f64` may hold) and `_` digits separator.
pub(crate) fn parse_number(value: &str) -> Option<BigDecimal> {
    let bytes = value.as_bytes();
    let separator_positions: Vec<usize> = bytes
        .iter()
        .enumerate()
        .filter(|(_, byte)| **byte == b'_')
        .map(|(index, _)| index)
        .collect();
    // BigDecimal crate does not handle "_" separators parsing well, so we do it here manually.
    let surrounded_by_digits = |index: usize| {
        let previous = index.checked_sub(1).map(|previous| bytes[previous]);
        let next = bytes.get(index + 1).copied();
        matches!((previous, next), (Some(previous), Some(next))
            if previous.is_ascii_digit() && next.is_ascii_digit())
    };
    if !separator_positions
        .iter()
        .copied()
        .all(surrounded_by_digits)
    {
        return None;
    }
    if separator_positions.is_empty() {
        value.parse().ok()
    } else {
        // Remove "_" separators to make BigDecimal parses the actual digits only.
        value.replace('_', "").parse().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::parse_number;

    fn parsed(value: &str) -> Option<String> {
        parse_number(value).map(|number| number.normalized().to_string())
    }

    #[test]
    fn plain_decimal_spellings_are_numbers() {
        assert_eq!(parsed("10"), Some("10".to_string()));
        assert_eq!(parsed("-1.5"), Some("-1.5".to_string()));
        assert_eq!(parsed("1e3"), Some("1000".to_string()));
    }

    #[test]
    fn digit_separators_between_digits_are_ignored() {
        assert_eq!(parsed("1_000"), Some("1000".to_string()));
        assert_eq!(parsed("1_000.000_1"), Some("1000.0001".to_string()));
        assert_eq!(parsed("-1_2"), Some("-12".to_string()));
        assert_eq!(parsed("1e1_0"), Some("10000000000".to_string()));
    }

    #[test]
    fn a_digit_separator_not_between_two_digits_is_not_a_number() {
        // Each of these is rejected by the languages that spell separators this way, so accepting
        // them here would let a typo pass as a number.
        for value in ["_1", "1_", "1__0", "1_.0", "1._0", "1_e3", "1e_3", "-_1"] {
            assert_eq!(parsed(value), None, "{value} unexpectedly parsed");
        }
    }

    #[test]
    fn a_separator_only_input_is_not_a_number() {
        assert_eq!(parsed("_"), None);
        assert_eq!(parsed(""), None);
    }

    #[test]
    fn float_specific_spellings_are_not_numbers() {
        assert_eq!(parsed("inf"), None);
        assert_eq!(parsed("NaN"), None);
    }

    #[test]
    fn an_exponent_beyond_the_supported_range_is_not_a_number() {
        assert_eq!(parsed("1e99999999999999999999"), None);
    }
}
