//! Block attributes, their names, and their parsed values.

mod line_count;
mod same_as;
mod severity;
mod sort;

pub(crate) use line_count::Constraint;
pub(crate) use same_as::{Format, Formats, Mode, TargetPaths, WHOLE_GROUP};
pub(crate) use severity::Severity;
pub(crate) use sort::{SortFormat, SortOrder};

use crate::target_reference::TargetReference;
use anyhow::{Context, anyhow, bail};
use regex::Regex;
use serde::Serialize;
use std::fmt;
use std::str::FromStr;
use std::time::Duration;
use strum::IntoEnumIterator;
use strum_macros::{Display, EnumIter, EnumString, IntoStaticStr};

/// Every attribute name a block tag or `[[block]]` entry can hold.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Display,
    EnumString,
    IntoStaticStr,
    EnumIter,
)]
#[strum(serialize_all = "kebab-case")]
pub(crate) enum AttributeName {
    Affects,
    CheckAi,
    CheckAiPattern,
    CheckLua,
    CheckLuaPattern,
    CheckLuaTimeout,
    KeepSorted,
    KeepSortedFormat,
    KeepSortedPattern,
    KeepUnique,
    LineCount,
    LineCountPattern,
    LinePattern,
    LinePatternForbidden,
    Name,
    SameAs,
    SameAsFormat,
    SameAsMode,
    SameAsPaths,
    SameAsPattern,
    Severity,
}

impl AttributeName {
    /// The attribute name as written in tags and `blockwatch.toml`, such as `"keep-sorted"`.
    pub(crate) fn as_str(self) -> &'static str {
        self.into()
    }

    /// Attributes of which at least one must appear alongside `self`. Empty means `self` can stand
    /// on its own.
    fn required_with(self) -> &'static [Self] {
        match self {
            Self::CheckAiPattern => &[Self::CheckAi],
            Self::CheckLuaPattern | Self::CheckLuaTimeout => &[Self::CheckLua],
            Self::KeepSortedFormat | Self::KeepSortedPattern => &[Self::KeepSorted],
            Self::LineCountPattern => &[Self::LineCount],
            Self::SameAsFormat | Self::SameAsMode => &[Self::SameAs, Self::SameAsPaths],
            Self::SameAsPattern => &[Self::SameAs, Self::SameAsPaths, Self::Name],
            Self::Affects
            | Self::CheckAi
            | Self::CheckLua
            | Self::KeepSorted
            | Self::KeepUnique
            | Self::LineCount
            | Self::LinePattern
            | Self::LinePatternForbidden
            | Self::Name
            | Self::SameAs
            | Self::SameAsPaths
            | Self::Severity => &[],
        }
    }
}

/* <block name="attribute-names" same-as="src/skill.md:validator-reference"
same-as-format="identifier" same-as-pattern='^\s*(?P<value>[a-z_]+):'> */
/// The attributes of a block. Only `affects` and `same-as` can hold a list.
#[derive(Debug, Clone, Default)]
pub(crate) struct Attributes {
    // <block keep-sorted keep-sorted-pattern="^\s*(?P<value>[a-z_]+):">
    affects: Option<Vec<TargetReference>>,
    check_ai: Option<String>,
    check_ai_pattern: Option<Regex>,
    check_lua: Option<String>,
    check_lua_pattern: Option<Regex>,
    check_lua_timeout: Option<Duration>,
    keep_sorted: Option<SortOrder>,
    keep_sorted_format: Option<SortFormat>,
    keep_sorted_pattern: Option<Regex>,
    keep_unique: Option<Option<Regex>>,
    line_count: Option<Constraint>,
    line_count_pattern: Option<Regex>,
    line_pattern: Option<Regex>,
    line_pattern_forbidden: Option<Regex>,
    name: Option<String>,
    same_as: Option<Vec<TargetReference>>,
    same_as_format: Option<Formats>,
    same_as_mode: Option<Mode>,
    same_as_paths: Option<TargetPaths>,
    same_as_pattern: Option<Regex>,
    severity: Option<Severity>,
    // </block>
}
// </block>

impl Serialize for Attributes {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_map(self.values().map(|(name, value)| (name.as_str(), value)))
    }
}

/// Two attributes are equal when their [`values`](Attributes::values) are, so `keep-sorted` and
/// `keep-sorted="asc"` are equal.
impl PartialEq for Attributes {
    fn eq(&self, other: &Self) -> bool {
        // `Regex` has no `PartialEq`, so the fields can't be compared by a derive.
        self.values().eq(other.values())
    }
}

impl Eq for Attributes {}

impl Attributes {
    /// The attributes of a start tag, from `values`: each attribute's name and its value.
    ///
    /// # Errors
    /// See [`Self::from_entries`] for error cases.
    pub(crate) fn from_tag(
        values: impl IntoIterator<Item = (String, String)>,
    ) -> anyhow::Result<Self> {
        Self::from_entries(
            values
                .into_iter()
                .map(|(name, value)| (name, AttributeEntry::Text(value))),
        )
    }

    /// Builds attributes from `entries`: each attribute's name and its value as written.
    ///
    /// Parses each attribute's text or list into its domain value and checks cross-attribute rules,
    /// such as requiring `keep-sorted` alongside `keep-sorted-format` or checking `same-as-format`
    /// group names against `same-as-pattern`.
    ///
    /// # Errors
    /// Returns an error for an unrecognized attribute name, duplicate names, a wrong value type
    /// (such as a list passed to a non-reference attribute), an empty list on a reference
    /// attribute, an invalid attribute value, or a modifier attribute without its main attribute.
    pub(crate) fn from_entries(
        entries: impl IntoIterator<Item = (String, AttributeEntry)>,
    ) -> anyhow::Result<Self> {
        let mut entries: Vec<(String, AttributeEntry)> = entries.into_iter().collect();
        // Sort by name so multiple errors report in the same order regardless of map iteration.
        entries.sort_by(|(left, _), (right, _)| left.cmp(right));

        let mut attributes = Self::default();
        for (raw_name, entry) in entries {
            let name: AttributeName = raw_name
                .parse()
                .map_err(|_| anyhow!("unrecognized attribute `{raw_name}`"))?;
            if attributes.has(name) {
                bail!("duplicate attribute `{name}`");
            }
            attributes.set_entry(name, entry)?;
        }
        // Without a main attribute, no validator reads a modifier, so the block would pass unchecked.
        for name in AttributeName::iter() {
            if !attributes.has(name) {
                continue;
            }
            let required = name.required_with();
            if !required.is_empty() && !required.iter().any(|&main| attributes.has(main)) {
                bail!("{}", missing_main_message(name, required));
            }
        }
        if let Some(formats) = attributes.same_as_format() {
            formats.validate_groups(attributes.same_as_pattern())?;
        }
        Ok(attributes)
    }

    /// Parses `entry` and stores it in the matching field of `self`.
    fn set_entry(&mut self, name: AttributeName, entry: AttributeEntry) -> anyhow::Result<()> {
        match name {
            AttributeName::Affects => set_refs(&mut self.affects, name, entry),
            AttributeName::CheckAi => set_non_empty_text(&mut self.check_ai, name, entry),
            AttributeName::CheckAiPattern => {
                set_parsed(&mut self.check_ai_pattern, name, entry, parse_regex)
            }
            AttributeName::CheckLua => set_non_empty_text(&mut self.check_lua, name, entry),
            AttributeName::CheckLuaPattern => {
                set_parsed(&mut self.check_lua_pattern, name, entry, parse_regex)
            }
            AttributeName::CheckLuaTimeout => {
                set_parsed(&mut self.check_lua_timeout, name, entry, parse_timeout)
            }
            AttributeName::KeepSorted => {
                set_parsed(&mut self.keep_sorted, name, entry, parse_keyword_or_default)
            }
            AttributeName::KeepSortedFormat => set_parsed(
                &mut self.keep_sorted_format,
                name,
                entry,
                parse_keyword_or_default,
            ),
            AttributeName::KeepSortedPattern => {
                set_parsed(&mut self.keep_sorted_pattern, name, entry, parse_regex)
            }
            AttributeName::KeepUnique => {
                set_parsed(&mut self.keep_unique, name, entry, parse_optional_regex)
            }
            AttributeName::LineCount => {
                set_parsed(&mut self.line_count, name, entry, Constraint::parse)
            }
            AttributeName::LineCountPattern => {
                set_parsed(&mut self.line_count_pattern, name, entry, parse_regex)
            }
            AttributeName::LinePattern => {
                set_parsed(&mut self.line_pattern, name, entry, parse_regex)
            }
            AttributeName::LinePatternForbidden => {
                set_parsed(&mut self.line_pattern_forbidden, name, entry, parse_regex)
            }
            AttributeName::Name => {
                self.name = Some(text_entry(name, entry)?);
                Ok(())
            }
            AttributeName::SameAs => set_refs(&mut self.same_as, name, entry),
            AttributeName::SameAsFormat => {
                set_parsed(&mut self.same_as_format, name, entry, Formats::parse)
            }
            AttributeName::SameAsMode => {
                set_parsed(&mut self.same_as_mode, name, entry, Mode::parse)
            }
            AttributeName::SameAsPaths => {
                set_parsed(&mut self.same_as_paths, name, entry, TargetPaths::parse)
            }
            AttributeName::SameAsPattern => {
                set_parsed(&mut self.same_as_pattern, name, entry, parse_regex)
            }
            AttributeName::Severity => set_parsed(&mut self.severity, name, entry, Severity::parse),
        }
    }

    /// The `affects` target references, or `None` when not set.
    pub(crate) fn affects(&self) -> Option<&[TargetReference]> {
        self.affects.as_deref()
    }

    /// The `check-ai` attribute, or `None` when not set.
    pub(crate) fn check_ai(&self) -> Option<&str> {
        self.check_ai.as_deref()
    }

    /// The `check-ai-pattern` regex, or `None` when not set.
    pub(crate) fn check_ai_pattern(&self) -> Option<&Regex> {
        self.check_ai_pattern.as_ref()
    }

    /// The `check-lua` attribute, or `None` when not set.
    pub(crate) fn check_lua(&self) -> Option<&str> {
        self.check_lua.as_deref()
    }

    /// The `check-lua-pattern` regex, or `None` when not set.
    pub(crate) fn check_lua_pattern(&self) -> Option<&Regex> {
        self.check_lua_pattern.as_ref()
    }

    /// The `check-lua-timeout` duration, or `None` when not set.
    pub(crate) fn check_lua_timeout(&self) -> Option<Duration> {
        self.check_lua_timeout
    }

    /// The `keep-sorted` sort direction, or `None` when not set.
    pub(crate) fn keep_sorted(&self) -> Option<SortOrder> {
        self.keep_sorted
    }

    /// The `keep-sorted-format` comparison mode, or `None` when not set.
    pub(crate) fn keep_sorted_format(&self) -> Option<SortFormat> {
        self.keep_sorted_format
    }

    /// The `keep-sorted-pattern` regex, or `None` when not set.
    pub(crate) fn keep_sorted_pattern(&self) -> Option<&Regex> {
        self.keep_sorted_pattern.as_ref()
    }

    /// The `keep-unique` attribute when set (`Some(None)` when empty), or `None` when omitted.
    pub(crate) fn keep_unique(&self) -> Option<Option<&Regex>> {
        self.keep_unique.as_ref().map(Option::as_ref)
    }

    /// The `line-count` constraint, or `None` when not set.
    pub(crate) fn line_count(&self) -> Option<Constraint> {
        self.line_count
    }

    /// The `line-count-pattern` regex, or `None` when not set.
    pub(crate) fn line_count_pattern(&self) -> Option<&Regex> {
        self.line_count_pattern.as_ref()
    }

    /// The `line-pattern` regex, or `None` when not set.
    pub(crate) fn line_pattern(&self) -> Option<&Regex> {
        self.line_pattern.as_ref()
    }

    /// The `line-pattern-forbidden` regex, or `None` when not set.
    pub(crate) fn line_pattern_forbidden(&self) -> Option<&Regex> {
        self.line_pattern_forbidden.as_ref()
    }

    /// The `name` attribute, or `None` when not set.
    pub(crate) fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// The `same-as` target references, or `None` when not set.
    pub(crate) fn same_as(&self) -> Option<&[TargetReference]> {
        self.same_as.as_deref()
    }

    /// The `same-as-format` rules, or `None` when not set.
    pub(crate) fn same_as_format(&self) -> Option<&Formats> {
        self.same_as_format.as_ref()
    }

    /// The `same-as-mode` comparison mode, or `None` when not set.
    pub(crate) fn same_as_mode(&self) -> Option<Mode> {
        self.same_as_mode
    }

    /// The `same-as-paths` target glob, or `None` when not set.
    pub(crate) fn same_as_paths(&self) -> Option<&TargetPaths> {
        self.same_as_paths.as_ref()
    }

    /// The `same-as-pattern` regex, or `None` when not set.
    pub(crate) fn same_as_pattern(&self) -> Option<&Regex> {
        self.same_as_pattern.as_ref()
    }

    /// How serious a violation of this block is: its `severity` attribute, or
    /// [`Severity::Error`] when not set.
    pub(crate) fn severity(&self) -> Severity {
        self.severity.unwrap_or_default()
    }

    /// Whether `name` is set on this block.
    pub(crate) fn has(&self, name: AttributeName) -> bool {
        // Formatting a value costs a little, but then `value` is the only place that pairs each
        // name with its field.
        self.value(name).is_some()
    }

    /// The formatted value of `name`, or `None` when not set.
    fn value(&self, name: AttributeName) -> Option<AttributeValue> {
        fn regex_value(value: Option<&Regex>) -> Option<AttributeValue> {
            value.map(|re| AttributeValue::Text(re.as_str().to_string()))
        }
        fn display_value<T: fmt::Display>(value: Option<&T>) -> Option<AttributeValue> {
            value.map(|v| AttributeValue::Text(v.to_string()))
        }
        fn refs_value(value: Option<&[TargetReference]>) -> Option<AttributeValue> {
            value.map(|refs| AttributeValue::List(refs.iter().map(ToString::to_string).collect()))
        }

        match name {
            AttributeName::Affects => refs_value(self.affects.as_deref()),
            AttributeName::CheckAi => self.check_ai.clone().map(AttributeValue::Text),
            AttributeName::CheckAiPattern => regex_value(self.check_ai_pattern.as_ref()),
            AttributeName::CheckLua => self.check_lua.clone().map(AttributeValue::Text),
            AttributeName::CheckLuaPattern => regex_value(self.check_lua_pattern.as_ref()),
            AttributeName::CheckLuaTimeout => self
                .check_lua_timeout
                .map(|duration| AttributeValue::Text(duration.as_secs().to_string())),
            AttributeName::KeepSorted => display_value(self.keep_sorted.as_ref()),
            AttributeName::KeepSortedFormat => display_value(self.keep_sorted_format.as_ref()),
            AttributeName::KeepSortedPattern => regex_value(self.keep_sorted_pattern.as_ref()),
            AttributeName::KeepUnique => self
                .keep_unique
                .as_ref()
                .map(|re| AttributeValue::Text(re.as_ref().map_or("", Regex::as_str).to_string())),
            AttributeName::LineCount => display_value(self.line_count.as_ref()),
            AttributeName::LineCountPattern => regex_value(self.line_count_pattern.as_ref()),
            AttributeName::LinePattern => regex_value(self.line_pattern.as_ref()),
            AttributeName::LinePatternForbidden => {
                regex_value(self.line_pattern_forbidden.as_ref())
            }
            AttributeName::Name => self.name.clone().map(AttributeValue::Text),
            AttributeName::SameAs => refs_value(self.same_as.as_deref()),
            AttributeName::SameAsFormat => display_value(self.same_as_format.as_ref()),
            AttributeName::SameAsMode => display_value(self.same_as_mode.as_ref()),
            AttributeName::SameAsPaths => self
                .same_as_paths
                .as_ref()
                .map(|paths| AttributeValue::Text(paths.as_str().to_string())),
            AttributeName::SameAsPattern => regex_value(self.same_as_pattern.as_ref()),
            AttributeName::Severity => display_value(self.severity.as_ref()),
        }
    }

    /// Every set attribute paired with its formatted value, in alphabetical order of name.
    pub(crate) fn values(&self) -> impl Iterator<Item = (AttributeName, AttributeValue)> + '_ {
        AttributeName::iter().filter_map(|name| Some((name, self.value(name)?)))
    }
}

/// One attribute value passed to [`Attributes::from_entries`] before parsing.
#[derive(Debug, PartialEq, Eq, Clone)]
pub(crate) enum AttributeEntry {
    /// Text from a tag, or a string, `true`, or integer from a `[[block]]` entry.
    Text(String),
    /// A list of strings from a `[[block]]` entry (`affects` or `same-as`).
    List(Vec<String>),
    /// A value whose type cannot hold any attribute, such as `false` or a float.
    InvalidType,
}

/// The formatted value of a set attribute, for JSON output and Lua's `ctx.attrs`.
#[derive(Debug, PartialEq, Eq, Clone, Serialize)]
#[serde(untagged)]
pub(crate) enum AttributeValue {
    /// A single value, such as `"asc"` or `"<=50"`.
    Text(String),
    /// The target references of `affects` or `same-as`.
    List(Vec<String>),
}

/// Parses `value` as one of the keywords of `T`.
///
/// # Errors
/// Returns `"\"value\" (expected ...)"` when `value` does not match a variant of `T`.
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

/// Parses `raw`, after trimming whitespace, as one of the keywords of `T`. An empty value gives
/// `T::default()`.
///
/// # Errors
/// Returns the error of [`parse_keyword_value`] when the value is not empty and not a keyword.
fn parse_keyword_or_default<T>(raw: &str) -> anyhow::Result<T>
where
    T: Default + FromStr + IntoEnumIterator + ToString,
{
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        Ok(T::default())
    } else {
        parse_keyword_value(trimmed)
    }
}

/// Extracts a text value or returns the config type error for `name`.
fn text_entry(name: AttributeName, entry: AttributeEntry) -> anyhow::Result<String> {
    match entry {
        AttributeEntry::Text(text) => Ok(text),
        AttributeEntry::List(_) | AttributeEntry::InvalidType => {
            bail!("{}", wrong_type_message(name))
        }
    }
}

/// Stores the text of `entry` in `slot`. Fails with ``invalid `{name}` `` when the text is empty or
/// only spaces.
fn set_non_empty_text(
    slot: &mut Option<String>,
    name: AttributeName,
    entry: AttributeEntry,
) -> anyhow::Result<()> {
    let raw = text_entry(name, entry)?;
    if raw.trim().is_empty() {
        bail!("invalid `{name}`: must not be empty");
    }
    *slot = Some(raw);
    Ok(())
}

/// Parses a text attribute with `parse` and stores its typed value.
fn set_parsed<T>(
    slot: &mut Option<T>,
    name: AttributeName,
    entry: AttributeEntry,
    parse: fn(&str) -> anyhow::Result<T>,
) -> anyhow::Result<()> {
    let raw = text_entry(name, entry)?;
    let value = parse(&raw).with_context(|| format!("invalid `{name}`"))?;
    *slot = Some(value);
    Ok(())
}

/// Parses `affects` or `same-as` from a text value or a non-empty list of entries.
fn set_refs(
    slot: &mut Option<Vec<TargetReference>>,
    name: AttributeName,
    entry: AttributeEntry,
) -> anyhow::Result<()> {
    let value = match entry {
        AttributeEntry::Text(text) => TargetReference::parse_separated(&text),
        AttributeEntry::List(entries) if entries.is_empty() => {
            bail!("`{name}` must list at least one reference");
        }
        AttributeEntry::List(entries) => TargetReference::parse_list(&entries),
        AttributeEntry::InvalidType => bail!("{}", wrong_type_message(name)),
    }
    .with_context(|| format!("invalid `{name}`"))?;
    *slot = Some(value);
    Ok(())
}

/// Compiles a required attribute regex such as `line-pattern`.
fn parse_regex(raw: &str) -> anyhow::Result<Regex> {
    if raw.is_empty() {
        bail!("must not be empty");
    }
    // A formatter that wraps long comments can break a tag inside a pattern. Where it breaks at
    // a space, the space is gone, so the line break can't be turned back into the text it
    // replaced. Left in, the line break makes the pattern match no line, and a rule such as
    // line-pattern then checks the wrong text.
    if raw.contains('\n') {
        bail!("the pattern has a line break. Put the pattern on one line");
    }
    Ok(Regex::new(raw)?)
}

/// Compiles an optional attribute regex (`keep-unique`), returning `None` when `raw` is empty.
fn parse_optional_regex(raw: &str) -> anyhow::Result<Option<Regex>> {
    if raw.is_empty() {
        Ok(None)
    } else {
        Ok(Some(parse_regex(raw)?))
    }
}

/// Parses `raw` as a `check-lua-timeout` wall-clock budget in whole seconds.
///
/// A value below one second is rejected: a timeout that short cannot tell a runaway script from a
/// slow-but-terminating one, which is the false-violation trap the timeout exists to avoid.
fn parse_timeout(raw: &str) -> anyhow::Result<Duration> {
    let seconds: u64 = raw
        .trim()
        .parse()
        .ok()
        .filter(|&seconds| seconds >= 1)
        .ok_or_else(|| anyhow!("must be a whole number of seconds >= 1, got {raw:?}"))?;
    Ok(Duration::from_secs(seconds))
}

/// Formats `` `{extra}` without ... `` quoting each attribute in `main`, which must be non-empty.
fn missing_main_message(extra: AttributeName, main: &[AttributeName]) -> String {
    let main: Vec<String> = main.iter().map(|name| format!("`{name}`")).collect();
    let (last, others) = main.split_last().expect("required_with is non-empty");
    if others.is_empty() {
        format!("`{extra}` without {last}")
    } else {
        format!("`{extra}` without {} or {last}", others.join(", "))
    }
}

/// Formats the config type error for `name`, quoting the accepted TOML value shapes.
fn wrong_type_message(name: AttributeName) -> String {
    format!(
        "`{name}` must be a string, `true` or an integer. `affects` and `same-as` can also be a \
         list of strings"
    )
}

#[cfg(test)]
mod attribute_names_tests {
    use super::{
        AttributeName, Attributes,
        test_utils::{sample_all_attributes, sample_entries},
    };
    use crate::validators::Validator;
    use strum::IntoEnumIterator;

    #[test]
    fn single_attribute_returns_an_error_only_when_it_needs_another() {
        // A new attribute must either work alone or stop the run alone. Otherwise a block with
        // only that attribute would pass without being checked.
        let needs_no_other: Vec<AttributeName> = Validator::iter()
            .flat_map(Validator::attributes)
            .copied()
            .chain([AttributeName::Name, AttributeName::Severity])
            .collect();
        for (name, entry) in sample_entries() {
            let result = Attributes::from_entries([(name.as_str().to_string(), entry)]);

            assert_eq!(result.is_ok(), needs_no_other.contains(&name), "{name}");
        }
    }

    #[test]
    fn known_names_are_in_alphabetical_order() {
        // JSON output relies on AttributeName variant order matching alphabetical order.
        let names: Vec<&str> = AttributeName::iter().map(AttributeName::as_str).collect();
        assert!(names.is_sorted(), "{names:?}");
    }

    #[test]
    fn every_attribute_is_read_from_its_own_field() -> anyhow::Result<()> {
        let sample = sample_all_attributes();

        // Each sample value is different, so a name paired with the wrong field changes this.
        assert_eq!(
            serde_json::to_value(&sample)?,
            serde_json::json!({
                "affects": ["affected.py:target"],
                "check-ai": "must be valid",
                "check-ai-pattern": "ai",
                "check-lua": "check.lua",
                "check-lua-pattern": "lua",
                "check-lua-timeout": "10",
                "keep-sorted": "asc",
                "keep-sorted-format": "lexicographic",
                "keep-sorted-pattern": "sorted",
                "keep-unique": "",
                "line-count": ">0",
                "line-count-pattern": "count",
                "line-pattern": "allowed",
                "line-pattern-forbidden": "forbidden",
                "name": "sample",
                "same-as": ["same.py:target"],
                "same-as-format": "numeric",
                "same-as-mode": "set",
                "same-as-paths": "*.py",
                "same-as-pattern": "same",
                "severity": "error",
            })
        );
        // Each value written back as its attribute is stored in the field it came from.
        let entries = sample_entries()
            .into_iter()
            .map(|(name, entry)| (name.as_str().to_string(), entry));
        assert_eq!(Attributes::from_entries(entries)?, sample);
        Ok(())
    }

    #[test]
    fn required_with_lists_the_attributes_that_turn_on_the_modified_validator() {
        // A modifier's name starts with its validator's name, such as `keep-sorted-format`.
        for name in AttributeName::iter() {
            let required = name.required_with();
            if required.is_empty() {
                continue;
            }
            let validator = Validator::iter()
                .find(|validator| {
                    name.as_str()
                        .starts_with(&format!("{}-", validator.as_str()))
                })
                .unwrap_or_else(|| panic!("no validator for `{name}`"));
            // A block with a `name` can be a `same-as` target, which reads its own pattern.
            let main: Vec<AttributeName> = required
                .iter()
                .copied()
                .filter(|&main| main != AttributeName::Name)
                .collect();

            assert_eq!(main, validator.attributes(), "{name}");
        }
    }
}

#[cfg(test)]
mod attributes_tests {
    use super::{AttributeEntry, Attributes, parse_regex};

    #[test]
    fn unrecognized_name_from_tag_returns_error() {
        let error = Attributes::from_tag([("keep-sortd".into(), "".into())])
            .unwrap_err()
            .to_string();

        assert_eq!(error, "unrecognized attribute `keep-sortd`");
    }

    #[test]
    fn list_on_non_reference_attribute_from_entries_returns_wrong_type_error() {
        let error = Attributes::from_entries([(
            "keep-sorted".into(),
            AttributeEntry::List(vec!["asc".into()]),
        )])
        .unwrap_err()
        .to_string();

        assert_eq!(
            error,
            "`keep-sorted` must be a string, `true` or an integer. `affects` and `same-as` can \
             also be a list of strings"
        );
    }

    #[test]
    fn invalid_type_from_entries_returns_wrong_type_error() {
        let error = Attributes::from_entries([("affects".into(), AttributeEntry::InvalidType)])
            .unwrap_err()
            .to_string();

        assert_eq!(
            error,
            "`affects` must be a string, `true` or an integer. `affects` and `same-as` can also \
             be a list of strings"
        );
    }

    #[test]
    fn duplicate_name_from_entries_returns_error() {
        let error = Attributes::from_entries([
            ("name".into(), AttributeEntry::Text("first".into())),
            ("name".into(), AttributeEntry::Text("second".into())),
        ])
        .unwrap_err()
        .to_string();

        assert_eq!(error, "duplicate attribute `name`");
    }

    #[test]
    fn empty_list_from_entries_returns_reference_error() {
        let error =
            Attributes::from_entries([("same-as".into(), AttributeEntry::List(Vec::new()))])
                .unwrap_err()
                .to_string();

        assert_eq!(error, "`same-as` must list at least one reference");
    }

    #[test]
    fn empty_list_on_non_reference_attribute_from_entries_returns_wrong_type_error() {
        let error =
            Attributes::from_entries([("keep-sorted".into(), AttributeEntry::List(Vec::new()))])
                .unwrap_err()
                .to_string();

        assert_eq!(
            error,
            "`keep-sorted` must be a string, `true` or an integer. `affects` and `same-as` can \
             also be a list of strings"
        );
    }

    #[test]
    fn invalid_affects_text_from_tag_returns_error() {
        let error = Attributes::from_tag([("affects".into(), "file.rs:".into())]).unwrap_err();

        assert_eq!(
            format!("{error:#}"),
            "invalid `affects`: \"file.rs:\" has no block name after ':'; drop the ':' to \
             reference the whole file"
        );
    }

    #[test]
    fn invalid_same_as_list_entry_from_entries_returns_error() {
        let error = Attributes::from_entries([(
            "same-as".into(),
            AttributeEntry::List(vec!["ok.md".into(), "".into()]),
        )])
        .unwrap_err();

        assert_eq!(
            format!("{error:#}"),
            "invalid `same-as`: entry 2 of the list: empty target reference"
        );
    }

    #[test]
    fn blank_check_ai_and_check_lua_from_tag_return_errors() {
        for name in ["check-ai", "check-lua"] {
            for value in ["", "   "] {
                let error = Attributes::from_tag([(name.into(), value.into())]).unwrap_err();
                assert!(
                    format!("{error:#}").starts_with(&format!("invalid `{name}`:")),
                    "{name}={value:?}: {error:#}"
                );
            }
        }
    }

    #[test]
    fn empty_required_pattern_from_tag_returns_error() {
        for name in [
            "check-ai-pattern",
            "check-lua-pattern",
            "keep-sorted-pattern",
            "line-count-pattern",
            "line-pattern",
            "line-pattern-forbidden",
            "same-as-pattern",
        ] {
            let error = Attributes::from_tag([(name.into(), "".into())]).unwrap_err();
            assert!(
                format!("{error:#}").starts_with(&format!("invalid `{name}`:")),
                "{name}: {error:#}"
            );
        }
        // A single space regex is valid for matching literal spaces.
        assert!(parse_regex(" ").is_ok());
    }

    #[test]
    fn empty_same_as_paths_from_tag_returns_error() {
        for value in ["", "   ", "/", " / "] {
            let error = Attributes::from_tag([("same-as-paths".into(), value.into())]).unwrap_err();
            assert!(
                format!("{error:#}").starts_with("invalid `same-as-paths`:"),
                "{value:?}: {error:#}"
            );
        }
    }

    #[test]
    fn empty_and_blank_optional_values_from_tag_return_defaults() {
        let attrs = Attributes::from_tag([
            ("keep-sorted".into(), "  ".into()),
            ("keep-sorted-format".into(), "  ".into()),
            ("keep-unique".into(), "".into()),
        ])
        .unwrap();

        assert_eq!(attrs.keep_sorted(), Some(Default::default()));
        assert_eq!(attrs.keep_sorted_format(), Some(Default::default()));
        assert!(matches!(attrs.keep_unique(), Some(None)));
    }

    #[test]
    fn values_formats_every_attribute_in_normalized_form() -> anyhow::Result<()> {
        let attrs = Attributes::from_tag([
            (
                "affects".into(),
                "./a.md:b, :c, d.json#/x, #/y, e.txt".into(),
            ),
            ("check-lua".into(), "check.lua".into()),
            ("check-lua-timeout".into(), " 30 ".into()),
            ("keep-sorted".into(), "".into()),
            ("keep-sorted-format".into(), " NUMERIC ".into()),
            ("keep-unique".into(), "".into()),
            ("line-count".into(), "<= 50".into()),
            ("same-as-format".into(), " b : flat , a : numeric ".into()),
            ("same-as-mode".into(), " SET ".into()),
            ("same-as-paths".into(), " *.py ".into()),
            ("same-as-pattern".into(), r"(?P<a>\w+) (?P<b>\w+)".into()),
            ("severity".into(), " InFo ".into()),
        ])?;

        assert_eq!(
            serde_json::to_value(&attrs)?,
            serde_json::json!({
                "affects": ["a.md:b", ":c", "d.json#/x", "#/y", "e.txt"],
                "check-lua": "check.lua",
                "check-lua-timeout": "30",
                "keep-sorted": "asc",
                "keep-sorted-format": "numeric",
                "keep-unique": "",
                "line-count": "<=50",
                "same-as-format": "a:numeric, b:flat",
                "same-as-mode": "set",
                "same-as-paths": "*.py",
                "same-as-pattern": r"(?P<a>\w+) (?P<b>\w+)",
                "severity": "info",
            })
        );
        Ok(())
    }

    #[test]
    fn attributes_with_the_same_values_are_equal() -> anyhow::Result<()> {
        let written = Attributes::from_tag([
            ("keep-sorted".into(), "".into()),
            ("severity".into(), "InFo".into()),
        ])?;
        let normalized = Attributes::from_tag([
            ("keep-sorted".into(), "asc".into()),
            ("severity".into(), "info".into()),
        ])?;
        let other = Attributes::from_tag([
            ("keep-sorted".into(), "desc".into()),
            ("severity".into(), "info".into()),
        ])?;

        assert_eq!(written, normalized);
        assert_ne!(written, other);
        Ok(())
    }

    #[test]
    fn pattern_with_an_escaped_line_break_compiles() -> anyhow::Result<()> {
        assert!(parse_regex(r"a\nb")?.is_match("a\nb"));
        Ok(())
    }

    #[test]
    fn check_ai_pattern_without_check_ai_returns_an_error() {
        let error = Attributes::from_tag([("check-ai-pattern".into(), "\\d+".into())]).unwrap_err();

        assert_eq!(error.to_string(), "`check-ai-pattern` without `check-ai`");
    }

    #[test]
    fn check_lua_modifier_without_check_lua_returns_an_error() {
        for (name, value) in [("check-lua-pattern", "\\d+"), ("check-lua-timeout", "60")] {
            let error = Attributes::from_tag([(name.into(), value.into())]).unwrap_err();

            assert_eq!(error.to_string(), format!("`{name}` without `check-lua`"));
        }
    }

    #[test]
    fn keep_sorted_modifier_without_keep_sorted_returns_an_error() {
        for (name, value) in [
            ("keep-sorted-pattern", "id: (?P<value>\\d+)"),
            ("keep-sorted-format", "numeric"),
        ] {
            let error = Attributes::from_tag([(name.into(), value.into())]).unwrap_err();

            assert_eq!(error.to_string(), format!("`{name}` without `keep-sorted`"));
        }
    }

    #[test]
    fn line_count_pattern_without_line_count_returns_an_error() {
        let error =
            Attributes::from_tag([("line-count-pattern".into(), "\\w+".into())]).unwrap_err();

        assert_eq!(
            error.to_string(),
            "`line-count-pattern` without `line-count`"
        );
    }

    #[test]
    fn same_as_mode_or_format_without_same_as_returns_an_error() {
        for (name, value) in [("same-as-mode", "set"), ("same-as-format", "flat")] {
            let error = Attributes::from_tag([(name.into(), value.into())]).unwrap_err();

            assert_eq!(
                error.to_string(),
                format!("`{name}` without `same-as` or `same-as-paths`")
            );
        }
    }

    #[test]
    fn same_as_pattern_without_same_as_or_name_returns_an_error() {
        let error = Attributes::from_tag([("same-as-pattern".into(), "\\d+".into())]).unwrap_err();

        assert_eq!(
            error.to_string(),
            "`same-as-pattern` without `same-as`, `same-as-paths` or `name`"
        );
    }

    #[test]
    fn same_as_format_for_unknown_group_without_same_as_reports_missing_main_first() {
        let error =
            Attributes::from_tag([("same-as-format".into(), "foo:numeric".into())]).unwrap_err();

        assert_eq!(
            error.to_string(),
            "`same-as-format` without `same-as` or `same-as-paths`"
        );
    }

    #[test]
    fn same_as_format_for_unknown_group_with_same_as_returns_an_error() {
        let error = Attributes::from_tag([
            ("same-as".into(), ":b".into()),
            ("same-as-format".into(), "foo:numeric".into()),
        ])
        .unwrap_err();

        assert_eq!(
            error.to_string(),
            "same-as-format refers to group \"foo\", which same-as-pattern doesn't have"
        );
    }
}

/// Test helpers for building [`Attributes`].
#[cfg(test)]
pub(crate) mod test_utils {
    use super::*;

    /// The attributes `pairs`, each with a text value, as a tag gives them.
    ///
    /// # Panics
    /// Panics if no attribute has one of the names.
    pub(crate) fn text_attributes(pairs: &[(&str, &str)]) -> Attributes {
        Attributes::from_tag(
            pairs
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string())),
        )
        .expect("known attribute names")
    }

    /// An [`Attributes`] value with every field set to a valid sample.
    pub(crate) fn sample_all_attributes() -> Attributes {
        let refs = |raw| TargetReference::parse_separated(raw).expect("valid sample reference");
        let re = |raw| parse_regex(raw).expect("valid sample regex");
        // Struct literal without `..Default::default()` fails to compile when a new attribute
        // field is added until that field is included here.
        // Every value differs from the others, so a test can tell which field a value came from.
        Attributes {
            affects: Some(refs("affected.py:target")),
            check_ai: Some("must be valid".to_string()),
            check_ai_pattern: Some(re("ai")),
            check_lua: Some("check.lua".to_string()),
            check_lua_pattern: Some(re("lua")),
            check_lua_timeout: Some(Duration::from_secs(10)),
            keep_sorted: Some(SortOrder::Asc),
            keep_sorted_format: Some(SortFormat::Lexicographic),
            keep_sorted_pattern: Some(re("sorted")),
            keep_unique: Some(None),
            line_count: Some(Constraint::parse(">0").expect("valid sample constraint")),
            line_count_pattern: Some(re("count")),
            line_pattern: Some(re("allowed")),
            line_pattern_forbidden: Some(re("forbidden")),
            name: Some("sample".to_string()),
            same_as: Some(refs("same.py:target")),
            same_as_format: Some(Formats::parse("numeric").expect("valid sample format")),
            same_as_mode: Some(Mode::Set),
            same_as_paths: Some(TargetPaths::parse("*.py").expect("valid sample glob")),
            same_as_pattern: Some(re("same")),
            severity: Some(Severity::Error),
        }
    }

    /// Every attribute paired with a valid sample [`AttributeEntry`], in [`AttributeName`] order.
    pub(crate) fn sample_entries() -> Vec<(AttributeName, AttributeEntry)> {
        sample_all_attributes()
            .values()
            .map(|(name, value)| {
                let entry = match value {
                    AttributeValue::Text(text) => AttributeEntry::Text(text),
                    AttributeValue::List(list) => AttributeEntry::List(list),
                };
                (name, entry)
            })
            .collect()
    }
}
