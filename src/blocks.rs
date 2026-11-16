//! The `Block` type and the other types that describe the blocks of a file.

use crate::Position;
use crate::diff_parser::{self, LineChange, LineChangeKind, LineChanges};
use crate::repo_path::RepoPath;
use crate::target_reference::TargetReference;
use crate::validators::{
    Constraint, Formats, Mode, SortFormat, SortOrder, TargetPaths, Validator, parse_keyword_value,
    parse_timeout,
};
use crate::virtual_blocks::ConfigEntry;
use anyhow::{Context, anyhow, bail};
use regex::Regex;
use serde::Serialize;
use serde_repr::Serialize_repr;
use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::ops::Range;
use std::time::Duration;
use strum::IntoEnumIterator;
use strum_macros::{Display, EnumIter, EnumString, IntoStaticStr};

const UNNAMED_BLOCK_LABEL: &str = "(unnamed)";

/// Every attribute name a block tag or `[[block]]` entry can hold.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Display, EnumString, IntoStaticStr, EnumIter,
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
}

/* <block name="attribute-names" same-as="src/skill.md:validator-reference"
same-as-format="identifier" same-as-pattern='^\s*(?P<value>[a-z_]+):'> */
/// The attributes of a block. Only `affects` and `same-as` can hold a list.
#[derive(Debug, PartialEq, Eq, Clone, Default, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct Attributes {
    // <block keep-sorted keep-sorted-pattern="^\s*(?P<value>[a-z_]+):">
    #[serde(skip_serializing_if = "Option::is_none")]
    affects: Option<Parsed<Vec<TargetReference>, AttributeValue>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    check_ai: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    check_ai_pattern: Option<Parsed<Regex>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    check_lua: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    check_lua_pattern: Option<Parsed<Regex>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    check_lua_timeout: Option<Parsed<Duration>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    keep_sorted: Option<Parsed<SortOrder>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    keep_sorted_format: Option<Parsed<SortFormat>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    keep_sorted_pattern: Option<Parsed<Option<Regex>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    keep_unique: Option<Parsed<Option<Regex>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    line_count: Option<Parsed<Constraint>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    line_count_pattern: Option<Parsed<Regex>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    line_pattern: Option<Parsed<Regex>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    line_pattern_forbidden: Option<Parsed<Regex>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    same_as: Option<Parsed<Vec<TargetReference>, AttributeValue>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    same_as_format: Option<Parsed<Formats>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    same_as_mode: Option<Parsed<Mode>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    same_as_paths: Option<Parsed<TargetPaths>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    same_as_pattern: Option<Parsed<Regex>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    severity: Option<Parsed<BlockSeverity>>,
    // </block>
}
// </block>

impl Attributes {
    /// The attributes of a start tag, from `values`: each attribute's name and its value.
    ///
    /// # Errors
    /// Returns an error that quotes a name that no attribute has, such as
    /// ``unrecognized attribute `keep-sortd` ``.
    pub(crate) fn from_tag(values: HashMap<String, String>) -> anyhow::Result<Self> {
        Self::from_entries(
            values
                .into_iter()
                .map(|(name, value)| (name, AttributeEntry::Text(value))),
        )
    }

    /// Builds attributes from normalized name-value `entries`.
    ///
    /// Parses each attribute's text or list into its domain value. It does not check rules between
    /// attributes, such as `keep-sorted-format` without `keep-sorted`; call
    /// [`validate_attributes`] for those.
    ///
    /// # Errors
    /// Returns an error for an unrecognized attribute name, duplicate names, a wrong value type
    /// (such as a list passed to a non-reference attribute), an empty list on a reference
    /// attribute, or an invalid attribute value.
    pub(crate) fn from_entries(
        entries: impl IntoIterator<Item = (String, AttributeEntry)>,
    ) -> anyhow::Result<Self> {
        let mut entries: Vec<(String, AttributeEntry)> = entries.into_iter().collect();
        // Sort by name so multiple errors report in the same order regardless of map iteration.
        entries.sort_by(|(left, _), (right, _)| left.cmp(right));

        // Resolve every attribute name first so a typo beats an invalid value on another field.
        let named: Vec<(AttributeName, AttributeEntry)> = entries
            .into_iter()
            .map(|(raw_name, entry)| {
                let name = raw_name
                    .parse()
                    .map_err(|_| anyhow!("unrecognized attribute `{raw_name}`"))?;
                Ok((name, entry))
            })
            .collect::<anyhow::Result<_>>()?;

        if let Some(pair) = named.windows(2).find(|pair| pair[0].0 == pair[1].0) {
            bail!("duplicate attribute `{}`", pair[0].0);
        }

        let mut attributes = Self::default();
        for (name, entry) in named {
            attributes.set_entry(name, entry)?;
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
                set_parsed(&mut self.keep_sorted, name, entry, SortOrder::parse)
            }
            AttributeName::KeepSortedFormat => {
                set_parsed(&mut self.keep_sorted_format, name, entry, SortFormat::parse)
            }
            AttributeName::KeepSortedPattern => set_parsed(
                &mut self.keep_sorted_pattern,
                name,
                entry,
                parse_optional_regex,
            ),
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
            AttributeName::Severity => {
                set_parsed(&mut self.severity, name, entry, BlockSeverity::parse)
            }
        }
    }

    /// The `affects` target references, or `None` when not set.
    pub(crate) fn affects(&self) -> Option<&[TargetReference]> {
        self.affects.as_ref().map(|parsed| parsed.value.as_slice())
    }

    /// The `check-ai` attribute, or `None` when not set.
    pub(crate) fn check_ai(&self) -> Option<&str> {
        self.check_ai.as_deref()
    }

    /// The `check-ai-pattern` regex, or `None` when not set.
    pub(crate) fn check_ai_pattern(&self) -> Option<&Regex> {
        self.check_ai_pattern.as_ref().map(|parsed| &parsed.value)
    }

    /// The `check-lua` attribute, or `None` when not set.
    pub(crate) fn check_lua(&self) -> Option<&str> {
        self.check_lua.as_deref()
    }

    /// The `check-lua-pattern` regex, or `None` when not set.
    pub(crate) fn check_lua_pattern(&self) -> Option<&Regex> {
        self.check_lua_pattern.as_ref().map(|parsed| &parsed.value)
    }

    /// The `check-lua-timeout` duration, or `None` when not set.
    pub(crate) fn check_lua_timeout(&self) -> Option<Duration> {
        self.check_lua_timeout.as_ref().map(|parsed| parsed.value)
    }

    /// The `keep-sorted` sort direction, or `None` when not set.
    pub(crate) fn keep_sorted(&self) -> Option<SortOrder> {
        self.keep_sorted.as_ref().map(|parsed| parsed.value)
    }

    /// The `keep-sorted-format` comparison mode, or `None` when not set.
    pub(crate) fn keep_sorted_format(&self) -> Option<SortFormat> {
        self.keep_sorted_format.as_ref().map(|parsed| parsed.value)
    }

    /// The `keep-sorted-pattern` attribute when set (`Some(None)` when empty), or `None` when omitted.
    pub(crate) fn keep_sorted_pattern(&self) -> Option<Option<&Regex>> {
        self.keep_sorted_pattern
            .as_ref()
            .map(|parsed| parsed.value.as_ref())
    }

    /// The `keep-unique` attribute when set (`Some(None)` when empty), or `None` when omitted.
    pub(crate) fn keep_unique(&self) -> Option<Option<&Regex>> {
        self.keep_unique
            .as_ref()
            .map(|parsed| parsed.value.as_ref())
    }

    /// The `line-count` constraint, or `None` when not set.
    pub(crate) fn line_count(&self) -> Option<Constraint> {
        self.line_count.as_ref().map(|parsed| parsed.value)
    }

    /// The `line-count-pattern` regex, or `None` when not set.
    pub(crate) fn line_count_pattern(&self) -> Option<&Regex> {
        self.line_count_pattern.as_ref().map(|parsed| &parsed.value)
    }

    /// The `line-pattern` regex, or `None` when not set.
    pub(crate) fn line_pattern(&self) -> Option<&Regex> {
        self.line_pattern.as_ref().map(|parsed| &parsed.value)
    }

    /// The `line-pattern-forbidden` regex, or `None` when not set.
    pub(crate) fn line_pattern_forbidden(&self) -> Option<&Regex> {
        self.line_pattern_forbidden
            .as_ref()
            .map(|parsed| &parsed.value)
    }

    /// The `name` attribute, or `None` when not set.
    pub(crate) fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// The `same-as` target references, or `None` when not set.
    pub(crate) fn same_as(&self) -> Option<&[TargetReference]> {
        self.same_as.as_ref().map(|parsed| parsed.value.as_slice())
    }

    /// The `same-as-format` rules, or `None` when not set.
    pub(crate) fn same_as_format(&self) -> Option<&Formats> {
        self.same_as_format.as_ref().map(|parsed| &parsed.value)
    }

    /// The `same-as-mode` comparison mode, or `None` when not set.
    pub(crate) fn same_as_mode(&self) -> Option<Mode> {
        self.same_as_mode.as_ref().map(|parsed| parsed.value)
    }

    /// The `same-as-paths` target glob, or `None` when not set.
    pub(crate) fn same_as_paths(&self) -> Option<&TargetPaths> {
        self.same_as_paths.as_ref().map(|parsed| &parsed.value)
    }

    /// The `same-as-pattern` regex, or `None` when not set.
    pub(crate) fn same_as_pattern(&self) -> Option<&Regex> {
        self.same_as_pattern.as_ref().map(|parsed| &parsed.value)
    }

    /// How serious a violation of this block is: its `severity` attribute, or
    /// [`BlockSeverity::Error`] when not set.
    pub(crate) fn severity(&self) -> BlockSeverity {
        self.severity
            .as_ref()
            .map_or(BlockSeverity::default(), |parsed| parsed.value)
    }

    /// Every set attribute paired with its raw value, in alphabetical order of name.
    pub(crate) fn raw_values(&self) -> Vec<(AttributeName, AttributeValue)> {
        fn text_value(
            attr: AttributeName,
            value: Option<&String>,
        ) -> Option<(AttributeName, AttributeValue)> {
            value.map(|text| (attr, AttributeValue::Text(text.clone())))
        }
        fn parsed_text<T>(
            attr: AttributeName,
            value: Option<&Parsed<T>>,
        ) -> Option<(AttributeName, AttributeValue)> {
            value.map(|parsed| (attr, AttributeValue::Text(parsed.raw.clone())))
        }
        fn parsed_ref(
            attr: AttributeName,
            value: Option<&Parsed<Vec<TargetReference>, AttributeValue>>,
        ) -> Option<(AttributeName, AttributeValue)> {
            value.map(|parsed| (attr, parsed.raw.clone()))
        }

        let Self {
            affects,
            check_ai,
            check_ai_pattern,
            check_lua,
            check_lua_pattern,
            check_lua_timeout,
            keep_sorted,
            keep_sorted_format,
            keep_sorted_pattern,
            keep_unique,
            line_count,
            line_count_pattern,
            line_pattern,
            line_pattern_forbidden,
            name,
            same_as,
            same_as_format,
            same_as_mode,
            same_as_paths,
            same_as_pattern,
            severity,
        } = self;

        [
            parsed_ref(AttributeName::Affects, affects.as_ref()),
            text_value(AttributeName::CheckAi, check_ai.as_ref()),
            parsed_text(AttributeName::CheckAiPattern, check_ai_pattern.as_ref()),
            text_value(AttributeName::CheckLua, check_lua.as_ref()),
            parsed_text(AttributeName::CheckLuaPattern, check_lua_pattern.as_ref()),
            parsed_text(AttributeName::CheckLuaTimeout, check_lua_timeout.as_ref()),
            parsed_text(AttributeName::KeepSorted, keep_sorted.as_ref()),
            parsed_text(AttributeName::KeepSortedFormat, keep_sorted_format.as_ref()),
            parsed_text(
                AttributeName::KeepSortedPattern,
                keep_sorted_pattern.as_ref(),
            ),
            parsed_text(AttributeName::KeepUnique, keep_unique.as_ref()),
            parsed_text(AttributeName::LineCount, line_count.as_ref()),
            parsed_text(AttributeName::LineCountPattern, line_count_pattern.as_ref()),
            parsed_text(AttributeName::LinePattern, line_pattern.as_ref()),
            parsed_text(
                AttributeName::LinePatternForbidden,
                line_pattern_forbidden.as_ref(),
            ),
            text_value(AttributeName::Name, name.as_ref()),
            parsed_ref(AttributeName::SameAs, same_as.as_ref()),
            parsed_text(AttributeName::SameAsFormat, same_as_format.as_ref()),
            parsed_text(AttributeName::SameAsMode, same_as_mode.as_ref()),
            parsed_text(AttributeName::SameAsPaths, same_as_paths.as_ref()),
            parsed_text(AttributeName::SameAsPattern, same_as_pattern.as_ref()),
            parsed_text(AttributeName::Severity, severity.as_ref()),
        ]
        .into_iter()
        .flatten()
        .collect()
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

/// Extracts a non-blank text value, failing with ``invalid `{name}``` when empty or spaces only.
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

/// Parses a text attribute with `parse` and stores both its raw text and parsed value.
fn set_parsed<T>(
    slot: &mut Option<Parsed<T>>,
    name: AttributeName,
    entry: AttributeEntry,
    parse: fn(&str) -> anyhow::Result<T>,
) -> anyhow::Result<()> {
    let raw = text_entry(name, entry)?;
    // Format `{error:#}` inline because `Block::from_tags` formats attribute errors with `{error}`.
    let value = parse(&raw).map_err(|error| anyhow!("invalid `{name}`: {error:#}"))?;
    *slot = Some(Parsed { raw, value });
    Ok(())
}

/// Parses `affects` or `same-as` from a text value or a non-empty list of entries.
fn set_refs(
    slot: &mut Option<Parsed<Vec<TargetReference>, AttributeValue>>,
    name: AttributeName,
    entry: AttributeEntry,
) -> anyhow::Result<()> {
    let raw = match entry {
        AttributeEntry::Text(text) => AttributeValue::Text(text),
        AttributeEntry::List(entries) if entries.is_empty() => {
            bail!("`{name}` must list at least one reference");
        }
        AttributeEntry::List(entries) => AttributeValue::List(entries),
        AttributeEntry::InvalidType => bail!("{}", wrong_type_message(name)),
    };
    let value = raw
        .parse_references()
        .map_err(|error| anyhow!("invalid `{name}`: {error:#}"))?;
    *slot = Some(Parsed { raw, value });
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

/// Compiles an optional attribute regex (`keep-sorted-pattern` or `keep-unique`), returning `None`
/// when `raw` is empty.
fn parse_optional_regex(raw: &str) -> anyhow::Result<Option<Regex>> {
    if raw.is_empty() {
        Ok(None)
    } else {
        Ok(Some(parse_regex(raw)?))
    }
}

/// An attribute's raw value paired with its parsed domain value.
///
/// Equality and serialization use `raw`, so `Attributes` compares equal when the author wrote the
/// same text and `blockwatch list` outputs that text verbatim.
#[derive(Debug, Clone, Serialize)]
#[serde(transparent)]
struct Parsed<T, R = String> {
    raw: R,
    #[serde(skip)]
    value: T,
}

impl<T, R: PartialEq> PartialEq for Parsed<T, R> {
    fn eq(&self, other: &Self) -> bool {
        self.raw == other.raw
    }
}

impl<T, R: Eq> Eq for Parsed<T, R> {}

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

/// Formats the config type error for `name`, quoting the accepted TOML value shapes.
fn wrong_type_message(name: AttributeName) -> String {
    format!(
        "`{name}` must be a string, `true` or an integer. `affects` and `same-as` can also be a \
         list of strings"
    )
}

/// The raw value of a set attribute.
#[derive(Debug, PartialEq, Eq, Clone, Serialize)]
#[serde(untagged)]
pub(crate) enum AttributeValue {
    /// A tag's value, or a string, `true` or an integer in a `[[block]]` entry.
    Text(String),
    /// A list in a `[[block]]` entry, with at least one entry.
    List(Vec<String>),
}

impl AttributeValue {
    /// Parses the target references carried by this attribute value.
    fn parse_references(&self) -> anyhow::Result<Vec<TargetReference>> {
        match self {
            Self::Text(text) => TargetReference::parse_separated(text),
            Self::List(entries) => TargetReference::parse_list(entries),
        }
    }
}

/// A block: a part of a file that its attributes set rules for.
///
/// A pair of tags in the file's comments declares a block. So does a `[[block]]` entry of the
/// config file, around a symbol of the file. That is a virtual block.
#[derive(Debug, PartialEq, Eq, Clone)]
pub(crate) struct Block {
    /// Optional attributes in the `block` tag, or in the `[[block]]` entry of a virtual block.
    /// Their names are what selects the validators that will check this block (`affects`,
    /// `keep-sorted`, …).
    pub(crate) attributes: Attributes,
    /// Block's start tag position range, half-open: it starts at the `<` symbol and ends one column
    /// past the `>` symbol. A virtual block has no tag, so it has the first line of the symbol's
    /// definition.
    pub(crate) start_tag_position_range: Range<Position>,
    /// The block's content.
    pub(crate) content: Content,
    /// Where the block's attributes are written.
    pub(crate) declaration: Declaration,
}

/// The content of a [`Block`]: its text, and where the block's file writes it.
#[derive(Debug, PartialEq, Eq, Clone)]
pub(crate) struct Content {
    /// The content's text.
    text: ContentText,
    /// Where the content is written in the block's file. For a block with tags, it runs from the
    /// end of the comment with the start tag to the start of the comment with the end tag. For a
    /// virtual block, it is where the symbol's value or definition is written.
    positions: Range<Position>,
}

/// The text of a block's [`Content`].
#[derive(Debug, PartialEq, Eq, Clone)]
enum ContentText {
    /// The source text in this byte range: the lines between the block's tags, or the part of the
    /// file that a virtual block wraps when it is written as is, such as a JSON object or a
    /// Markdown section.
    Source(Range<usize>),
    /// The decoded value of the scalar that a virtual block wraps. Its quotes and escapes are
    /// gone, so it is not a part of the source text.
    Decoded(String),
}

/// Where the attributes of a [`Block`] are written.
#[derive(Debug, PartialEq, Eq, Clone)]
pub(crate) enum Declaration {
    /// In the block's start tag, at its `start_tag_position_range`.
    Tags,
    /// In a `[[block]]` entry of the config file.
    ConfigEntry(ConfigEntry),
}

impl Declaration {
    /// The line of the `[[block]]` entry that declared the block, or `None` when declared by tags.
    fn config_line(&self) -> Option<usize> {
        match self {
            Self::Tags => None,
            Self::ConfigEntry(entry) => Some(entry.line),
        }
    }
}

/// A block and its file, written the way messages refer to the block. [`Block::label`] makes it.
pub(crate) struct BlockLabel<'a> {
    file: &'a RepoPath,
    block: &'a Block,
}

impl fmt::Display for BlockLabel<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{} at {}",
            self.file,
            self.block.name_display(),
            self.block.declared_at()
        )
    }
}

/// What tells a block apart from the other blocks of its file.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct BlockKey {
    /// Where the block starts. Two blocks with tags never start at the same place.
    start: Position,
    /// The config line of a virtual block, or `None` for a block with tags. Two virtual blocks
    /// can start at the same place, such as TOML's `/a` and `/a/b` in `a.b = 1`, but they never
    /// share a config line.
    config_line: Option<usize>,
}

/// What tells a named block apart from every other block in the repository: `FILE:BLOCK_NAME`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BlockAddress {
    file: RepoPath,
    name: String,
}

impl BlockAddress {
    /// The address of the block called `name` in `file`.
    pub(crate) fn new(file: RepoPath, name: String) -> Self {
        Self { file, name }
    }

    /// Parses `FILE:BLOCK_NAME`.
    ///
    /// Returns an error for any other address, such as `FILE` alone, an empty segment, or a file
    /// outside the repository.
    pub(crate) fn parse(address: &str) -> anyhow::Result<Self> {
        // `FILE` alone could select every block in a file, but a path argument or `--ignore`
        // already does that.
        match address.split(':').collect::<Vec<_>>().as_slice() {
            [file, name] if !file.is_empty() && !name.is_empty() => Ok(Self {
                file: RepoPath::from_reference(file)
                    .with_context(|| format!("invalid file in the block address \"{address}\""))?,
                name: name.to_string(),
            }),
            _ => bail!("expected FILE:BLOCK_NAME, got \"{address}\""),
        }
    }

    /// The file the block is in.
    pub(crate) fn file(&self) -> &RepoPath {
        &self.file
    }
}

impl fmt::Display for BlockAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.file, self.name)
    }
}

impl PartialOrd for Block {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Block {
    fn cmp(&self, other: &Self) -> Ordering {
        self.start_tag_position_range
            .start
            .cmp(&other.start_tag_position_range.start)
    }
}

impl Content {
    /// Content whose text is the bytes in `byte_range` of the block's file. `positions` are where
    /// those bytes are.
    pub(crate) fn source(byte_range: Range<usize>, positions: Range<Position>) -> Self {
        Self {
            text: ContentText::Source(byte_range),
            positions,
        }
    }

    /// Content whose text is `text`, the decoded value of a scalar. `positions` are where the
    /// scalar is written in the block's file.
    pub(crate) fn decoded(text: String, positions: Range<Position>) -> Self {
        Self {
            text: ContentText::Decoded(text),
            positions,
        }
    }

    /// Describes the content for JSON. `source` must be the text of the block's file.
    pub(crate) fn listing<'a>(&'a self, source: &'a str) -> ContentListing<'a> {
        ContentListing {
            start: self.positions.start,
            end: self.positions.end,
            text: self.text(source),
        }
    }

    /// Returns the content's text. `source` must be the text of the block's file.
    pub(crate) fn text<'a>(&'a self, source: &'a str) -> &'a str {
        match &self.text {
            #[expect(
                clippy::string_slice,
                reason = "a comment, a symbol or a match sets `range`"
            )]
            ContentText::Source(range) => &source[range.clone()],
            ContentText::Decoded(text) => text,
        }
    }

    /// Maps a position inside the content onto the position in the block's file.
    ///
    /// `line_idx` is the 0-based index of a line of [`Self::text`],
    /// `column_offset` is the 0-based offset within that line.
    pub(crate) fn position(&self, line_idx: usize, column_offset: usize) -> Position {
        match self.text {
            ContentText::Source(_) => {
                let line_start_column = if line_idx == 0 {
                    self.positions.start.character
                } else {
                    1
                };
                Position::new(
                    self.positions.start.line + line_idx,
                    line_start_column + column_offset,
                )
            }
            // Quotes, escapes and joined lines make a decoded value differ from the source text,
            // so a position inside it has no exact place in the source. The start of the value is
            // the closest one.
            ContentText::Decoded(_) => self.positions.start,
        }
    }

    /// Whether any of the `line_changes` touch where the content is written.
    pub(super) fn intersects_any(&self, line_changes: &LineChanges) -> bool {
        line_changes
            .on_lines(self.positions.start.line, self.positions.end.line)
            .any(|line_change| self.intersects(line_change))
    }

    /// Whether `line_change`, which has to fall on one of the lines the content spans, touches
    /// where the content is written.
    fn intersects(&self, line_change: &LineChange) -> bool {
        match &line_change.kind {
            LineChangeKind::Deleted => {
                let point = diff_parser::deletion_point(line_change.line);
                // The end is included even though the range excludes it: a gap sitting exactly at
                // the range's exclusive end is where the range's last character used to be. A
                // whole-line deletion never lands there, since its gap is always at column 1, so
                // this bound only bites if deletion points ever get finer-grained than a line.
                self.positions.start <= point && point <= self.positions.end
            }
            // An insertion moves the line breaks around the line, not just the characters on it,
            // so there is nothing to compare column by column. Every position on the line counts
            // as changed.
            LineChangeKind::Added => true,
            LineChangeKind::Modified(ranges) => {
                let start_col = if line_change.line == self.positions.start.line {
                    self.positions.start.character
                } else {
                    1
                };
                let end_col = if line_change.line < self.positions.end.line {
                    usize::MAX
                } else {
                    self.positions.end.character
                };

                diff_parser::intersects_columns(ranges, start_col, end_col)
            }
        }
    }
}

impl Block {
    /// Builds the block that a pair of tags declares, with the start tag at
    /// `start_tag_position_range`.
    ///
    /// # Errors
    /// Returns an error that shows the block's name and line if its attributes are not valid; see
    /// [`validate_attributes`].
    pub(crate) fn from_tags(
        attributes: HashMap<String, String>,
        start_tag_position_range: Range<Position>,
        content: Content,
    ) -> anyhow::Result<Self> {
        let name = attributes
            .get("name")
            .map_or(UNNAMED_BLOCK_LABEL, String::as_str)
            .to_string();
        let attributes = Attributes::from_tag(attributes)
            .and_then(|attributes| {
                validate_attributes(&attributes)?;
                Ok(attributes)
            })
            .map_err(|error| {
                // The tag parser does not know the file, so the message cannot show it.
                anyhow!(
                    "block {name} at line {} contains {error}",
                    start_tag_position_range.start.line
                )
            })?;
        Ok(Self {
            attributes,
            start_tag_position_range,
            content,
            declaration: Declaration::Tags,
        })
    }

    /// Returns the optional value of the `name` attribute for this block.
    pub(crate) fn name(&self) -> Option<&str> {
        self.attributes.name()
    }

    /// Returns the block's name if present, otherwise a human-friendly placeholder label.
    pub(crate) fn name_display(&self) -> &str {
        self.name().unwrap_or(UNNAMED_BLOCK_LABEL)
    }

    /// The address of this block, which is in `file`: `FILE:BLOCK_NAME`. `None` for an unnamed
    /// block.
    pub(crate) fn address(&self, file: &RepoPath) -> Option<BlockAddress> {
        Some(BlockAddress::new(file.clone(), self.name()?.to_string()))
    }

    /// Where the block's attributes are written, as messages show it: `line 3` for a block with
    /// tags, or `line 7 of "blockwatch.toml"` for a virtual block.
    pub(crate) fn declared_at(&self) -> String {
        match &self.declaration {
            // A message shows the block's file before this, so the line is enough.
            Declaration::Tags => format!("line {}", self.start_tag_position_range.start.line),
            Declaration::ConfigEntry(entry) => entry.to_string(),
        }
    }

    /// How a message refers to this block, which is in `file`: `FILE:NAME at WHERE`, such as
    /// `src/a.rs:limits at line 3`.
    pub(crate) fn label<'a>(&'a self, file: &'a RepoPath) -> BlockLabel<'a> {
        BlockLabel { file, block: self }
    }

    /// What tells the block apart from the other blocks of its file.
    pub(crate) fn key(&self) -> BlockKey {
        BlockKey {
            start: self.start_tag_position_range.start,
            config_line: self.declaration.config_line(),
        }
    }
}

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
pub(crate) enum BlockSeverity {
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

impl BlockSeverity {
    /// Parses `raw` as a severity level after trimming surrounding whitespace.
    pub(crate) fn parse(raw: &str) -> anyhow::Result<Self> {
        parse_keyword_value(raw.trim())
    }
}

/// Represents a source field with its corresponding modified blocks.
#[derive(Debug)]
pub(crate) struct FileBlocks {
    /// Source file contents.
    pub(crate) file_content: String,
    /// Blocks to be validated.
    pub(crate) blocks_with_context: Vec<BlockWithContext>,
}

impl FileBlocks {
    pub(super) fn is_empty(&self) -> bool {
        self.blocks_with_context.is_empty()
    }
}

/// Represents a block with its corresponding validation context.
#[derive(Debug, Clone)]
pub(crate) struct BlockWithContext {
    /// The block itself, as parsed from the source comment or the config file.
    pub(crate) block: Block,
    /// Whether the block's start tag is modified (computed from the input diff).
    pub(crate) is_start_tag_modified: bool,
    /// Whether the content of the block is modified (computed from the input diff). Validators such
    /// as `affects` fire only for blocks whose content actually changed.
    pub(crate) is_content_modified: bool,
}

// <block name="block-listing" affects="docs/cli.md:list-output-example">
/// A block, as it is written in JSON.
///
/// serde writes the keys in the order the fields are declared, and the output keeps its keys
/// sorted. So declare the fields in alphabetical order.
#[derive(Serialize, Debug)]
pub(crate) struct BlockListing<'a> {
    attributes: &'a Attributes,
    /// The validators that checked the block. `None` leaves the key out.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) checks: Option<BTreeSet<Validator>>,
    column: usize,
    /// The line of the `[[block]]` entry that declares a virtual block. `None` for a block with
    /// tags.
    #[serde(skip_serializing_if = "Option::is_none")]
    config_line: Option<usize>,
    is_content_modified: bool,
    line: usize,
    name: &'a str,
}
// </block>

/// The content of a block, as it is written in JSON: its text, and where the block's file writes
/// it.
#[derive(Serialize, Debug)]
pub(crate) struct ContentListing<'a> {
    /// The position of the content's first character.
    start: Position,
    /// The first position after the content.
    end: Position,
    /// The text the validators read. For a virtual block around a quoted or escaped value, it is
    /// the decoded value.
    text: &'a str,
}

impl<'a> From<&'a BlockWithContext> for BlockListing<'a> {
    /// Describes the block, with no `checks`.
    fn from(block_with_context: &'a BlockWithContext) -> Self {
        let block = &block_with_context.block;
        Self {
            attributes: &block.attributes,
            checks: None,
            column: block.start_tag_position_range.start.character,
            config_line: block.declaration.config_line(),
            is_content_modified: block_with_context.is_content_modified,
            line: block.start_tag_position_range.start.line,
            name: block.name_display(),
        }
    }
}

/// Checks that each validator accepts the attributes it reads (see
/// [`Validator::check_attributes`]).
///
/// # Errors
/// Returns an error that quotes the first attribute found to be wrong, such as
/// ``"`keep-sorted-pattern` without `keep-sorted`"``.
pub(crate) fn validate_attributes(attributes: &Attributes) -> anyhow::Result<()> {
    // A block can be in a file that the run reads only to find a target, and no validator runs on
    // such a block. Every parsed block passes here.
    Validator::iter().try_for_each(|validator| validator.check_attributes(attributes))
}

#[cfg(test)]
mod validate_attributes_tests {
    use crate::blocks::{
        AttributeName,
        test_utils::{sample_all_attributes, sample_attributes},
        validate_attributes,
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
        for (name, attributes) in sample_attributes() {
            let result = validate_attributes(&attributes);

            assert_eq!(
                result.is_ok(),
                needs_no_other
                    .iter()
                    .any(|allowed| allowed.as_str() == name),
                "{name}"
            );
        }
    }

    #[test]
    fn known_names_are_in_alphabetical_order() {
        // JSON output relies on field declaration order matching alphabetical order.
        let names: Vec<String> = sample_attributes()
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert!(names.is_sorted(), "{names:?}");
    }

    #[test]
    fn attribute_names_match_attributes_fields_in_order() {
        let enum_names: Vec<&str> = AttributeName::iter().map(AttributeName::as_str).collect();
        let raw_names: Vec<&str> = sample_all_attributes()
            .raw_values()
            .into_iter()
            .map(|(name, _)| name.as_str())
            .collect();
        assert_eq!(enum_names, raw_names);
    }
}

#[cfg(test)]
mod attributes_tests {
    use crate::blocks::{AttributeEntry, Attributes};
    use std::collections::HashMap;

    #[test]
    fn unrecognized_name_from_tag_returns_error() {
        let error = Attributes::from_tag(HashMap::from([("keep-sortd".into(), "".into())]))
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
    fn unrecognized_name_with_another_invalid_attribute_reports_unrecognized_name() {
        for pairs in [
            [("aaa-unknown", ""), ("line-count", "bad")],
            [("zzz-unknown", ""), ("affects", "file.rs:")],
            [("line-count", "bad"), ("zzz-unknown", "")],
        ] {
            let error = Attributes::from_entries(
                pairs
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), AttributeEntry::Text(v.to_string()))),
            )
            .unwrap_err()
            .to_string();

            assert!(error.contains("unrecognized attribute"), "{error}");
        }
    }

    #[test]
    fn duplicate_attribute_with_invalid_second_value_reports_duplicate() {
        let error = Attributes::from_entries([
            ("line-count".into(), AttributeEntry::Text(">0".into())),
            ("line-count".into(), AttributeEntry::Text("bad".into())),
        ])
        .unwrap_err()
        .to_string();

        assert_eq!(error, "duplicate attribute `line-count`");
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
        let error = Attributes::from_tag(HashMap::from([("affects".into(), "file.rs:".into())]))
            .unwrap_err();

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
                let error =
                    Attributes::from_tag(HashMap::from([(name.into(), value.into())])).unwrap_err();
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
            "line-count-pattern",
            "line-pattern",
            "line-pattern-forbidden",
            "same-as-pattern",
        ] {
            let error =
                Attributes::from_tag(HashMap::from([(name.into(), "".into())])).unwrap_err();
            assert!(
                format!("{error:#}").starts_with(&format!("invalid `{name}`:")),
                "{name}: {error:#}"
            );

            // A single space regex is valid for matching literal spaces.
            assert!(Attributes::from_tag(HashMap::from([(name.into(), " ".into())])).is_ok());
        }
    }

    #[test]
    fn empty_same_as_paths_from_tag_returns_error() {
        for value in ["", "   ", "/", " / "] {
            let error =
                Attributes::from_tag(HashMap::from([("same-as-paths".into(), value.into())]))
                    .unwrap_err();
            assert!(
                format!("{error:#}").starts_with("invalid `same-as-paths`:"),
                "{value:?}: {error:#}"
            );
        }
    }

    #[test]
    fn empty_and_blank_optional_values_from_tag_return_defaults() {
        let attrs = Attributes::from_tag(HashMap::from([
            ("keep-sorted".into(), "  ".into()),
            ("keep-sorted-format".into(), "  ".into()),
            ("keep-sorted-pattern".into(), "".into()),
            ("keep-unique".into(), "".into()),
        ]))
        .unwrap();

        assert_eq!(attrs.keep_sorted(), Some(Default::default()));
        assert_eq!(attrs.keep_sorted_format(), Some(Default::default()));
        assert!(matches!(attrs.keep_sorted_pattern(), Some(None)));
        assert!(matches!(attrs.keep_unique(), Some(None)));
    }

    #[test]
    fn parsed_attributes_with_different_raw_text_are_not_equal() {
        let lower = crate::test_utils::text_attributes(&[("keep-sorted", "asc")]);
        let same_lower = crate::test_utils::text_attributes(&[("keep-sorted", "asc")]);
        let upper = crate::test_utils::text_attributes(&[("keep-sorted", "ASC")]);
        let desc = crate::test_utils::text_attributes(&[("keep-sorted", "desc")]);

        assert_eq!(lower, same_lower);
        assert_ne!(lower, upper);
        assert_ne!(lower, desc);
    }

    #[test]
    fn pattern_with_an_escaped_line_break_compiles() -> anyhow::Result<()> {
        assert!(super::parse_regex(r"a\nb")?.is_match("a\nb"));
        Ok(())
    }
}

#[cfg(test)]
mod block_severity_tests {
    use crate::blocks::{AttributeName, AttributeValue, Attributes, BlockSeverity};
    use crate::test_utils::text_attributes;
    use std::collections::HashMap;

    #[test]
    fn block_with_valid_severity_attribute_returns_correct_severity() {
        let attributes = text_attributes(&[("severity", "warning")]);

        assert_eq!(attributes.severity(), BlockSeverity::Warning);
    }

    #[test]
    fn block_with_mixed_case_severity_attribute_returns_correct_severity() {
        let attributes = text_attributes(&[("severity", "InFo")]);

        assert_eq!(attributes.severity(), BlockSeverity::Info);
        assert_eq!(
            attributes.raw_values(),
            vec![(
                AttributeName::Severity,
                AttributeValue::Text("InFo".to_string()),
            )]
        );
    }

    #[test]
    fn block_without_severity_attribute_returns_error_severity() {
        assert_eq!(Attributes::default().severity(), BlockSeverity::Error);
    }

    #[test]
    fn block_with_trimmed_severity_attribute_returns_correct_severity() {
        let attributes = text_attributes(&[("severity", " warning ")]);

        assert_eq!(attributes.severity(), BlockSeverity::Warning);
    }

    #[test]
    fn block_with_invalid_severity_attribute_returns_error() {
        let error = Attributes::from_tag(HashMap::from([("severity".into(), "warn".into())]))
            .unwrap_err()
            .to_string();

        assert_eq!(
            error,
            "invalid `severity`: \"warn\" (expected error, warning, info or hint)"
        );
    }
}

#[cfg(test)]
mod block_address_tests {
    use crate::Position;
    use crate::blocks::{Block, BlockAddress, Content, Declaration};
    use crate::repo_path::RepoPath;
    use crate::test_utils::text_attributes;

    fn repo_path(path: &str) -> RepoPath {
        RepoPath::from_reference(path).expect("a valid repository path")
    }

    /// Builds a contentless block with `attributes`.
    fn block_with_attributes(attributes: &[(&str, &str)]) -> Block {
        Block {
            attributes: text_attributes(attributes),
            start_tag_position_range: Position::new(0, 0)..Position::new(0, 0),
            content: Content::source(0..0, Position::new(0, 0)..Position::new(0, 0)),
            declaration: Declaration::Tags,
        }
    }

    #[test]
    fn named_block_address_is_its_file_and_name() {
        let block = block_with_attributes(&[("name", "fruits")]);

        let address = block
            .address(&repo_path("a.py"))
            .expect("a named block has an address");

        assert_eq!(address.to_string(), "a.py:fruits");
    }

    #[test]
    fn unnamed_block_has_no_address() {
        let block = block_with_attributes(&[("keep-sorted", "asc")]);

        assert_eq!(block.address(&repo_path("a.py")), None);
    }

    #[test]
    fn parsed_address_equals_the_address_of_that_block() -> anyhow::Result<()> {
        let address = BlockAddress::parse("./a.md:n")?;
        assert_eq!(
            address,
            BlockAddress::new(repo_path("a.md"), "n".to_string())
        );
        assert_ne!(
            address,
            BlockAddress::new(repo_path("a.md"), "other".to_string())
        );
        assert_ne!(
            address,
            BlockAddress::new(repo_path("b.md"), "n".to_string())
        );
        Ok(())
    }

    #[test]
    fn address_without_two_non_empty_segments_is_rejected() {
        for address in ["a.md", "a.md:n:keep-unique", "a.md:", ":n"] {
            let error = BlockAddress::parse(address).expect_err("the address must be rejected");
            assert!(
                format!("{error:#}").contains("expected FILE:BLOCK_NAME"),
                "{address}: {error:#}"
            );
        }
    }
}

#[cfg(test)]
mod block_listing_tests {
    use super::*;
    use std::path::PathBuf;

    fn block_with_context(attributes: Attributes, declaration: Declaration) -> BlockWithContext {
        BlockWithContext {
            block: Block {
                attributes,
                start_tag_position_range: Position::new(2, 4)..Position::new(2, 30),
                content: Content::source(0..0, Position::new(3, 1)..Position::new(3, 1)),
                declaration,
            },
            is_start_tag_modified: false,
            is_content_modified: true,
        }
    }

    #[test]
    fn virtual_block_with_checks_is_written_with_its_keys_sorted() -> anyhow::Result<()> {
        let attributes = Attributes::from_entries([
            ("name".into(), AttributeEntry::Text("n".into())),
            ("line-count".into(), AttributeEntry::Text("<=3".into())),
            ("keep-sorted".into(), AttributeEntry::Text("asc".into())),
            (
                "affects".into(),
                AttributeEntry::List(vec!["a.md:b".into(), "c.md".into()]),
            ),
        ])?;
        let block = block_with_context(
            attributes,
            Declaration::ConfigEntry(ConfigEntry {
                file: PathBuf::from("blockwatch.toml"),
                line: 7,
                index: 0,
            }),
        );
        let mut listing = BlockListing::from(&block);
        listing.checks = Some(BTreeSet::from([Validator::KeepSorted]));

        assert_eq!(
            serde_json::to_string(&listing)?,
            r#"{"attributes":{"affects":["a.md:b","c.md"],"keep-sorted":"asc","line-count":"<=3","name":"n"},"checks":["keep-sorted"],"column":4,"config_line":7,"is_content_modified":true,"line":2,"name":"n"}"#
        );
        Ok(())
    }

    #[test]
    fn unnamed_block_with_tags_is_written_without_checks_and_config_line() -> anyhow::Result<()> {
        let block = block_with_context(
            crate::test_utils::text_attributes(&[("keep-sorted", "asc")]),
            Declaration::Tags,
        );

        assert_eq!(
            serde_json::to_string(&BlockListing::from(&block))?,
            r#"{"attributes":{"keep-sorted":"asc"},"column":4,"is_content_modified":true,"line":2,"name":"(unnamed)"}"#
        );
        Ok(())
    }
}

#[cfg(test)]
mod supported_languages_tests {
    use std::collections::HashMap;

    use crate::blocks::*;
    use crate::diff_parser::DiffChanges;
    use crate::fs::test_utils::FakeFileSystem;
    use crate::language_parsers::LanguageParsers;
    use crate::scan::{Scan, ScanMode};

    // <block name="supported-extensions">
    #[test]
    fn all_language_extensions_are_supported() -> anyhow::Result<()> {
        let parsers = LanguageParsers::new();
        let files = HashMap::from([
            (
                "BUILD".to_string(),
                "# <block>\ncc_library(name = \"foo\")\n# </block>".to_string(),
            ),
            (
                "MODULE.bazel".to_string(),
                "# <block>\nmodule(name = \"m\")\n# </block>".to_string(),
            ),
            (
                "WORKSPACE".to_string(),
                "# <block>\nworkspace(name = \"w\")\n# </block>".to_string(),
            ),
            (
                "WORKSPACE.bzlmod".to_string(),
                "# <block>\n# migration stub\n# </block>".to_string(),
            ),
            (
                "CMakeLists.txt".to_string(),
                "# <block>\nadd_library(foo foo.c)\n# </block>".to_string(),
            ),
            (
                "cmake.cmake".to_string(),
                "#[[ <block> ]]\nset(X 1)\n# </block>".to_string(),
            ),
            (
                "bash.bash".to_string(),
                "# <block>\necho \"hello\"\n# </block>".to_string(),
            ),
            (
                ".bash_profile".to_string(),
                "# <block>\nsource ~/.bashrc\n# </block>".to_string(),
            ),
            (
                ".bashrc".to_string(),
                "# <block>\nexport EDITOR=vim\n# </block>".to_string(),
            ),
            (
                ".envrc".to_string(),
                "# <block>\nexport API_URL=http://localhost\n# </block>".to_string(),
            ),
            (
                "zsh.zsh".to_string(),
                "# <block>\necho \"hello\"\n# </block>".to_string(),
            ),
            (
                ".zshrc".to_string(),
                "# <block>\nexport PATH=\"$HOME/bin:$PATH\"\n# </block>".to_string(),
            ),
            (
                "bzl.bzl".to_string(),
                "# <block>\ndef my_macro():\n    pass\n# </block>".to_string(),
            ),
            (
                "c.c".to_string(),
                "/* <block> */\nint main() { return 0; }\n/* </block> */".to_string(),
            ),
            (
                "cc.cc".to_string(),
                "// <block>\nint main() { return 0; }\n// </block>".to_string(),
            ),
            (
                "cpp.cpp".to_string(),
                "// <block>\nint main() { return 0; }\n// </block>".to_string(),
            ),
            (
                "cxx.cxx".to_string(),
                "// <block>\nint main() { return 0; }\n// </block>".to_string(),
            ),
            (
                "hh.hh".to_string(),
                "// <block>\nvoid foo();\n// </block>".to_string(),
            ),
            (
                "hpp.hpp".to_string(),
                "// <block>\nclass Foo {};\n// </block>".to_string(),
            ),
            (
                "hxx.hxx".to_string(),
                "// <block>\nstruct Bar;\n// </block>".to_string(),
            ),
            (
                "cs.cs".to_string(),
                "// <block>\nclass Program { }\n// </block>".to_string(),
            ),
            (
                "css.css".to_string(),
                "/* <block> */\nbody { margin: 0; }\n/* </block> */".to_string(),
            ),
            (
                "Containerfile".to_string(),
                "# <block>\nFROM fedora\n# </block>".to_string(),
            ),
            (
                "containerfile".to_string(),
                "# <block>\nFROM centos\n# </block>".to_string(),
            ),
            (
                "Dockerfile".to_string(),
                "# <block>\nFROM alpine\n# </block>".to_string(),
            ),
            (
                "app.dockerfile".to_string(),
                "# <block>\nFROM debian\n# </block>".to_string(),
            ),
            (
                "dart.dart".to_string(),
                "// <block>\nvoid main() {}\n// </block>".to_string(),
            ),
            (
                "ex.ex".to_string(),
                "# <block>\ndefmodule Foo do\nend\n# </block>".to_string(),
            ),
            (
                "exs.exs".to_string(),
                "# <block>\nIO.puts(:hello)\n# </block>".to_string(),
            ),
            (
                "go.go".to_string(),
                "// <block>\nfunc main() {}\n// </block>".to_string(),
            ),
            (
                "go.mod".to_string(),
                "// <block>\nmodule example.com/m\n// </block>".to_string(),
            ),
            (
                "go.sum".to_string(),
                "// <block>\nexample.com/dep v1.0.0 h1:abc\n// </block>".to_string(),
            ),
            (
                "go.work".to_string(),
                "// <block>\nuse ./mod\n// </block>".to_string(),
            ),
            (
                "gql.gql".to_string(),
                "# <block>\ntype Mutation {\n  noop: Boolean\n}\n# </block>".to_string(),
            ),
            (
                "gradle.gradle".to_string(),
                "// <block>\nversion = '1.0'\n// </block>".to_string(),
            ),
            (
                "graphql.graphql".to_string(),
                "# <block>\ntype Query {\n  hello: String\n}\n# </block>".to_string(),
            ),
            (
                "graphqls.graphqls".to_string(),
                "# <block>\ntype Query {\n  ping: String\n}\n# </block>".to_string(),
            ),
            (
                "groovy.groovy".to_string(),
                "// <block>\ndef x = 1\n// </block>".to_string(),
            ),
            (
                "h.h".to_string(),
                "// <block>\nvoid foo();\n// </block>".to_string(),
            ),
            (
                "hcl.hcl".to_string(),
                "// <block>\nregion = \"eu-west-1\"\n// </block>".to_string(),
            ),
            (
                "tofu.tofu".to_string(),
                "# <block>\nregion = \"eu-central-1\"\n# </block>".to_string(),
            ),
            (
                "nomad.nomad".to_string(),
                "# <block>\ndatacenters = [\"dc1\"]\n# </block>".to_string(),
            ),
            (
                "htm.htm".to_string(),
                "<!-- <block> -->\n<div>Content</div>\n<!-- </block> -->".to_string(),
            ),
            (
                "html.html".to_string(),
                "<!-- <block> -->\n<p>Hello</p>\n<!-- </block> -->".to_string(),
            ),
            (
                "xhtml.xhtml".to_string(),
                "<!-- <block> -->\n<p>Hi</p>\n<!-- </block> -->".to_string(),
            ),
            (
                "java.java".to_string(),
                "// <block>\nclass App {}\n// </block>".to_string(),
            ),
            (
                "Jenkinsfile".to_string(),
                "// <block>\npipeline { }\n// </block>".to_string(),
            ),
            (
                "jenkinsfile".to_string(),
                "// <block>\nnode { }\n// </block>".to_string(),
            ),
            (
                "js.js".to_string(),
                "// <block>\nconst x = 1;\n// </block>".to_string(),
            ),
            (
                "cjs.cjs".to_string(),
                "// <block>\nmodule.exports = {};\n// </block>".to_string(),
            ),
            (
                "mjs.mjs".to_string(),
                "// <block>\nexport const x = 1;\n// </block>".to_string(),
            ),
            (
                "json.json".to_string(),
                "// <block>\n{\"key\": 1}\n// </block>".to_string(),
            ),
            (
                "jsonc.jsonc".to_string(),
                "// <block>\n{\"key\": 1}\n// </block>".to_string(),
            ),
            (
                "site.webmanifest".to_string(),
                "// <block>\n{\"name\": \"app\"}\n// </block>".to_string(),
            ),
            (
                "jsx.jsx".to_string(),
                "// <block>\nconst Comp = () => <div/>;\n// </block>".to_string(),
            ),
            (
                "kt.kt".to_string(),
                "// <block>\nfun main() {}\n// </block>".to_string(),
            ),
            (
                "kts.kts".to_string(),
                "// <block>\nplugins { }\n// </block>".to_string(),
            ),
            (
                "lean.lean".to_string(),
                "-- <block>\ndef x := 1\n-- </block>".to_string(),
            ),
            (
                "lua.lua".to_string(),
                "-- <block>\nlocal x = 1\n-- </block>".to_string(),
            ),
            (
                "app-1.0-1.rockspec".to_string(),
                "-- <block>\npackage = \"app\"\n-- </block>".to_string(),
            ),
            (
                "makefile".to_string(),
                "# <block>\nall:\n\t@echo \"hello\"\n# </block>".to_string(),
            ),
            (
                "Makefile".to_string(),
                "# <block>\nall:\n\t@echo \"hello\"\n# </block>".to_string(),
            ),
            (
                "markdown.markdown".to_string(),
                "<div>\n<!-- <block> -->\n# Title\n<!-- </block> -->\n</div>".to_string(),
            ),
            (
                "md.md".to_string(),
                "<div>\n<!-- <block> -->\n## Heading\n<!-- </block> -->\n</div>".to_string(),
            ),
            (
                "mdx.mdx".to_string(),
                "{/* <block> */}\n## Heading\n{/* </block> */}".to_string(),
            ),
            (
                "mk.mk".to_string(),
                "# <block>\nall:\n\t@echo \"hello\"\n# </block>".to_string(),
            ),
            (
                "GNUmakefile".to_string(),
                "# <block>\nall:\n\t@echo \"hello\"\n# </block>".to_string(),
            ),
            (
                "mak.mak".to_string(),
                "# <block>\nall:\n\t@echo \"hello\"\n# </block>".to_string(),
            ),
            (
                "nix.nix".to_string(),
                "# <block>\n{ pkgs = null; }\n# </block>".to_string(),
            ),
            (
                "ml.ml".to_string(),
                "(* <block> *)\nlet x = 1\n(* </block> *)".to_string(),
            ),
            (
                "mli.mli".to_string(),
                "(* <block> *)\nval x : int\n(* </block> *)".to_string(),
            ),
            (
                "php.php".to_string(),
                "<?php\n# <block>\necho 'hello';\n# </block>\n?>".to_string(),
            ),
            (
                "phtml.phtml".to_string(),
                "<?php\n# <block>\necho 'world';\n# </block>\n?>".to_string(),
            ),
            (
                "proto.proto".to_string(),
                "// <block>\nsyntax = \"proto3\";\n// </block>".to_string(),
            ),
            (
                "py.py".to_string(),
                "# <block>\ndef main():\n    pass\n# </block>".to_string(),
            ),
            (
                "pyi.pyi".to_string(),
                "# <block>\ndef foo() -> None: pass\n# </block>".to_string(),
            ),
            (
                "pyw.pyw".to_string(),
                "# <block>\nimport tkinter\n# </block>".to_string(),
            ),
            (
                "rb.rb".to_string(),
                "# <block>\ndef hello\n  puts 'world'\nend\n# </block>".to_string(),
            ),
            (
                "Brewfile".to_string(),
                "# <block>\nbrew 'git'\n# </block>".to_string(),
            ),
            (
                "Fastfile".to_string(),
                "# <block>\nlane :beta do\nend\n# </block>".to_string(),
            ),
            (
                "Gemfile".to_string(),
                "# <block>\ngem 'rails'\n# </block>".to_string(),
            ),
            (
                "Podfile".to_string(),
                "# <block>\npod 'Alamofire'\n# </block>".to_string(),
            ),
            (
                "Rakefile".to_string(),
                "# <block>\ntask :default\n# </block>".to_string(),
            ),
            (
                "Vagrantfile".to_string(),
                "# <block>\nVagrant.configure('2') do |config|\nend\n# </block>".to_string(),
            ),
            (
                "index.atom.builder".to_string(),
                "# <block>\nxml.feed\n# </block>".to_string(),
            ),
            (
                "app.gemspec".to_string(),
                "# <block>\nGem::Specification.new\n# </block>".to_string(),
            ),
            (
                "show.json.jbuilder".to_string(),
                "# <block>\njson.name @user.name\n# </block>".to_string(),
            ),
            (
                "app.podspec".to_string(),
                "# <block>\nPod::Spec.new\n# </block>".to_string(),
            ),
            (
                "rake.rake".to_string(),
                "# <block>\ntask :build\n# </block>".to_string(),
            ),
            (
                "config.ru".to_string(),
                "# <block>\nrun App\n# </block>".to_string(),
            ),
            (
                "rs.rs".to_string(),
                r#"/* <block> */fn a() {}/* </block> */"#.to_string(),
            ),
            (
                "sbt.sbt".to_string(),
                "// <block>\nname := \"app\"\n// </block>".to_string(),
            ),
            (
                "scala.scala".to_string(),
                "// <block>\nval x = 1\n// </block>".to_string(),
            ),
            (
                "sh.sh".to_string(),
                "# <block>\necho \"hello\"\n# </block>".to_string(),
            ),
            (
                "sql.sql".to_string(),
                "-- <block>\nSELECT * FROM users;\n-- </block>".to_string(),
            ),
            (
                "star.star".to_string(),
                "# <block>\nx = 42\n# </block>".to_string(),
            ),
            (
                "BUCK".to_string(),
                "# <block>\ncxx_library(name = \"foo\")\n# </block>".to_string(),
            ),
            (
                "Tiltfile".to_string(),
                "# <block>\nk8s_yaml('app.yaml')\n# </block>".to_string(),
            ),
            (
                "swift.swift".to_string(),
                "// <block>\nfunc main() {}\n// </block>".to_string(),
            ),
            (
                "tf.tf".to_string(),
                "# <block>\nregion = \"us-east-1\"\n# </block>".to_string(),
            ),
            (
                "tfvars.tfvars".to_string(),
                "# <block>\nregion = \"us-west-2\"\n# </block>".to_string(),
            ),
            (
                "toml.toml".to_string(),
                "# <block>\nname = \"test\"\n# </block>".to_string(),
            ),
            (
                "Pipfile".to_string(),
                "# <block>\n[packages]\nrequests = \"*\"\n# </block>".to_string(),
            ),
            (
                "ts.ts".to_string(),
                "// <block>\nconst x: number = 1;\n// </block>".to_string(),
            ),
            (
                "tsx.tsx".to_string(),
                "// <block>\nconst C = () => <div/>;\n// </block>".to_string(),
            ),
            (
                "typescript.d.ts".to_string(),
                "// <block>\ndeclare const x: number;\n// </block>".to_string(),
            ),
            (
                "cts.cts".to_string(),
                "// <block>\nconst x: number = 1;\n// </block>".to_string(),
            ),
            (
                "mts.mts".to_string(),
                "// <block>\nexport const x: number = 1;\n// </block>".to_string(),
            ),
            (
                "xml.xml".to_string(),
                "<!-- <block> -->\n<root/>\n<!-- </block> -->".to_string(),
            ),
            (
                "App.xaml".to_string(),
                "<!-- <block> -->\n<Application/>\n<!-- </block> -->".to_string(),
            ),
            (
                "Directory.Build.props".to_string(),
                "<!-- <block> -->\n<Project/>\n<!-- </block> -->".to_string(),
            ),
            (
                "Directory.Build.targets".to_string(),
                "<!-- <block> -->\n<Project/>\n<!-- </block> -->".to_string(),
            ),
            (
                "app.csproj".to_string(),
                "<!-- <block> -->\n<Project/>\n<!-- </block> -->".to_string(),
            ),
            (
                "app.fsproj".to_string(),
                "<!-- <block> -->\n<Project/>\n<!-- </block> -->".to_string(),
            ),
            (
                "app.vbproj".to_string(),
                "<!-- <block> -->\n<Project/>\n<!-- </block> -->".to_string(),
            ),
            (
                "app.vcxproj".to_string(),
                "<!-- <block> -->\n<Project/>\n<!-- </block> -->".to_string(),
            ),
            (
                "schema.xsd".to_string(),
                "<!-- <block> -->\n<xs:schema/>\n<!-- </block> -->".to_string(),
            ),
            (
                "style.xsl".to_string(),
                "<!-- <block> -->\n<xsl:stylesheet/>\n<!-- </block> -->".to_string(),
            ),
            (
                "style.xslt".to_string(),
                "<!-- <block> -->\n<xsl:stylesheet/>\n<!-- </block> -->".to_string(),
            ),
            (
                "yaml.yaml".to_string(),
                "# <block>\nkey: value\n# </block>".to_string(),
            ),
            (
                "yml.yml".to_string(),
                "# <block>\nname: test\n# </block>".to_string(),
            ),
            (
                "CITATION.cff".to_string(),
                "# <block>\nversion: 1.0.0\n# </block>".to_string(),
            ),
        ]);
        let file_system = FakeFileSystem::new(files.clone());

        let blocks_by_file = Scan::new(
            &file_system,
            &crate::fs::test_utils::path_filter_all(),
            &parsers,
            &[],
        )
        .parse_blocks(&DiffChanges::default(), ScanMode::All)?
        .blocks;

        for file_name in files.keys() {
            assert!(
                !blocks_by_file
                    .get(&RepoPath::from_reference(file_name)?)
                    .unwrap_or_else(|| panic!("No blocks found for file {file_name}"))
                    .blocks_with_context
                    .is_empty(),
                "File {file_name} should have blocks",
            );
        }
        Ok(())
    }
    // </block>
}

/// Test helpers for building and inspecting [`Attributes`].
#[cfg(test)]
pub(crate) mod test_utils {
    use super::*;
    use serde::Deserializer;
    use serde::de::{IgnoredAny, MapAccess, Visitor};

    /// An [`Attributes`] value with every field set to a valid sample.
    pub(crate) fn sample_all_attributes() -> Attributes {
        fn parsed<T>(raw: &str, parse: fn(&str) -> anyhow::Result<T>) -> Parsed<T> {
            let value = parse(raw).expect("valid sample attribute");
            Parsed {
                raw: raw.to_string(),
                value,
            }
        }
        fn parsed_ref(raw: &str) -> Parsed<Vec<TargetReference>, AttributeValue> {
            let raw = AttributeValue::Text(raw.to_string());
            let value = raw.parse_references().expect("valid sample reference");
            Parsed { raw, value }
        }
        // Struct literal without `..Default::default()` fails to compile when a new attribute
        // field is added until that field is included here.
        Attributes {
            affects: Some(parsed_ref("other.py:target")),
            check_ai: Some("must be valid".to_string()),
            check_ai_pattern: Some(parsed(".", parse_regex)),
            check_lua: Some("check.lua".to_string()),
            check_lua_pattern: Some(parsed(".", parse_regex)),
            check_lua_timeout: Some(parsed("10", parse_timeout)),
            keep_sorted: Some(parsed("", SortOrder::parse)),
            keep_sorted_format: Some(parsed("", SortFormat::parse)),
            keep_sorted_pattern: Some(parsed("", parse_optional_regex)),
            keep_unique: Some(parsed("", parse_optional_regex)),
            line_count: Some(parsed(">0", Constraint::parse)),
            line_count_pattern: Some(parsed(".", parse_regex)),
            line_pattern: Some(parsed(".", parse_regex)),
            line_pattern_forbidden: Some(parsed("forbidden", parse_regex)),
            name: Some("sample".to_string()),
            same_as: Some(parsed_ref("other.py:target")),
            same_as_format: Some(parsed("numeric", Formats::parse)),
            same_as_mode: Some(parsed("set", Mode::parse)),
            same_as_paths: Some(parsed("*.py", TargetPaths::parse)),
            same_as_pattern: Some(parsed(".", parse_regex)),
            severity: Some(parsed("error", BlockSeverity::parse)),
        }
    }

    /// Every attribute paired with a single-attribute [`Attributes`] value, in [`Attributes`]
    /// serialization order.
    pub(crate) fn sample_attributes() -> Vec<(String, Attributes)> {
        struct KeyOrder(Vec<String>);

        impl<'de> serde::Deserialize<'de> for KeyOrder {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                struct KeysVisitor;

                impl<'de> Visitor<'de> for KeysVisitor {
                    type Value = KeyOrder;

                    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                        formatter.write_str("a JSON object of attributes")
                    }

                    fn visit_map<M>(self, mut map: M) -> Result<KeyOrder, M::Error>
                    where
                        M: MapAccess<'de>,
                    {
                        let mut keys = Vec::new();
                        while let Some((key, _)) = map.next_entry::<String, IgnoredAny>()? {
                            keys.push(key);
                        }
                        Ok(KeyOrder(keys))
                    }
                }

                deserializer.deserialize_map(KeysVisitor)
            }
        }

        let all = sample_all_attributes();
        let json = serde_json::to_string(&all).expect("Attributes serializes to JSON");
        // Parse the JSON stream instead of `serde_json::Value` (which sorts keys via `BTreeMap`
        // unless `preserve_order` is enabled) so field-declaration order is actually verified.
        let KeyOrder(keys) =
            serde_json::from_str(&json).expect("sample attributes serialize as a JSON object");
        let by_name: HashMap<&'static str, AttributeEntry> = all
            .raw_values()
            .into_iter()
            .map(|(name, value)| {
                let entry = match value {
                    AttributeValue::Text(text) => AttributeEntry::Text(text),
                    AttributeValue::List(list) => AttributeEntry::List(list),
                };
                (name.as_str(), entry)
            })
            .collect();
        keys.into_iter()
            .map(|name| {
                let entry = by_name[name.as_str()].clone();
                let single = Attributes::from_entries([(name.clone(), entry)])
                    .expect("valid sample attribute");
                (name, single)
            })
            .collect()
    }
}
