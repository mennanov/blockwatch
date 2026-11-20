//! Picks the values that a `*-pattern` attribute matches, in one line or in a block's content.

use crate::blocks::BlockWithContext;
use std::ops::Range;

/// Returns the value that `captures`, a match of `regex`, gives: the named group `value` when
/// `regex` has one, or else the whole match. Returns `None` when `regex` has a `value` group that is
/// not in this match.
fn value_match<'h>(
    regex: &regex::Regex,
    captures: &regex::Captures<'h>,
) -> Option<regex::Match<'h>> {
    // Decided by the pattern, not by the match. A pattern can put the group in one branch only,
    // such as `^(?:none|(?P<value>\w+))$`, so that a match of the other branch gives no value.
    if regex.capture_names().any(|name| name == Some("value")) {
        captures.name("value")
    } else {
        captures.get(0)
    }
}

/// Returns the substring compared on `line` and its 0-based character range `[start, end)` in
/// `line`.
///
/// With `pattern`, matches the trimmed line (see [`value_match`]). Matching after trimming keeps
/// `^` and `$` working on indented lines. Without `pattern`, returns the trimmed line. Returns
/// `None` for a blank line, a line `pattern` does not match, or an empty match.
pub(super) fn line_value<'a>(
    line: &'a str,
    pattern: Option<&regex::Regex>,
) -> Option<(&'a str, Range<usize>)> {
    let trimmed_line = line.trim();
    if trimmed_line.is_empty() {
        return None;
    }
    let trimmed_byte_offset = trimmed_line.as_ptr() as usize - line.as_ptr() as usize;
    let (value, match_offset) = match pattern {
        None => (trimmed_line, 0),
        Some(regex) => {
            let caps = regex.captures(trimmed_line)?;
            let matched = value_match(regex, &caps)?;
            if matched.is_empty() {
                return None;
            }
            (matched.as_str(), matched.start())
        }
    };
    #[expect(
        clippy::string_slice,
        reason = "`value` is a slice inside `trimmed_line`, which is inside `line`"
    )]
    let start = line[..trimmed_byte_offset + match_offset].chars().count();
    let end = start + value.chars().count();
    Some((value, start..end))
}

/// The content for the `*-pattern` attribute extracted from the block.
pub(super) enum PatternContent<'c> {
    /// No `*-pattern` attribute: the block's whole content (trimmed).
    Whole(&'c str),
    /// A `*-pattern` attribute: the value of every match, in the order they appear in the block.
    Matches(Vec<&'c str>),
}

/// Returns the content for the `*-pattern` attribute, e.g. `check-ai-pattern` for `check-ai`.
pub(super) fn block_content_for_pattern<'c>(
    block_with_context: &'c BlockWithContext,
    file_content: &'c str,
    pattern: Option<&regex::Regex>,
) -> PatternContent<'c> {
    let content = block_with_context.block.content.text(file_content);
    let Some(re) = pattern else {
        return PatternContent::Whole(content.trim());
    };
    PatternContent::Matches(
        re.captures_iter(content)
            .filter_map(|captures| value_match(re, &captures).map(|matched| matched.as_str()))
            .filter(|value| !value.is_empty())
            .collect(),
    )
}
