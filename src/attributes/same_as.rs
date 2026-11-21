//! Attribute values for `same-as-format`, `same-as-mode`, and `same-as-paths`.

use super::parse_keyword_value;
use crate::fs::{FileSystem, PathsGlob};
use anyhow::{anyhow, bail};
use regex::Regex;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt;
use strum_macros::{Display, EnumIter, EnumString};

/// The group of a whole match, or of a line read without a pattern. It is the name of the group that
/// picks the value in every `*-pattern` attribute, so `(?P<value>\d+)` and `\d+` give equal records.
pub(crate) const WHOLE_GROUP: &str = "value";

/// The groups of every record that `pattern` gives: its named groups, or [`WHOLE_GROUP`] when it
/// has none or when there is no pattern.
fn pattern_groups(pattern: Option<&Regex>) -> Vec<&str> {
    let groups: Vec<&str> = match pattern {
        Some(regex) => regex.capture_names().flatten().collect(),
        None => Vec::new(),
    };
    if groups.is_empty() {
        vec![WHOLE_GROUP]
    } else {
        groups
    }
}

/// How the values of records are canonicalized before comparison.
#[derive(Clone, Copy, Debug, PartialEq, Eq, EnumString, EnumIter, Display)]
#[strum(ascii_case_insensitive, serialize_all = "lowercase")]
pub(crate) enum Format {
    /// Compare values as extracted (the default). The attribute has no keyword for it: leaving the
    /// attribute out, or a group out of it, does this.
    #[strum(disabled)]
    Verbatim,
    /// Parse each value as a number so different spellings (`"10"`, `"10.0"`) compare equal.
    Numeric,
    /// Split each value into words so naming styles (`"PreExistingFailure"`,
    /// `"pre-existing-failure"`) compare equal.
    Identifier,
    /// Keep only the lower-cased letters and digits of each value, so names whose words break in
    /// different places (`"DarkRed"`, `"darkred"`) compare equal.
    Flat,
}

/// The format of each group of a block's records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Formats(FormatsKind);

#[derive(Debug, Clone, PartialEq, Eq)]
enum FormatsKind {
    /// One format for every group.
    All(Format),
    /// A format for some groups, by group name. Every other group is compared as written, which
    /// is the strictest comparison, so a group left out can't hide a difference.
    ByGroup(BTreeMap<String, Format>),
}

impl Formats {
    /// Compares every group as written, the default when `same-as-format` is not set.
    ///
    /// Don't format it: [`Format::Verbatim`] has no keyword, so strum's `Display` panics on it.
    /// A parsed `same-as-format` never holds it, so formatting the attribute is safe.
    pub(crate) const VERBATIM: Self = Self(FormatsKind::All(Format::Verbatim));

    /// Parses `raw` as a single `same-as-format` keyword or comma-separated `group:format` pairs.
    pub(super) fn parse(raw: &str) -> anyhow::Result<Self> {
        let trimmed = raw.trim();
        if !trimmed.contains(':') {
            return Ok(Self(FormatsKind::All(parse_keyword_value(trimmed)?)));
        }
        let mut entries = BTreeMap::new();
        for entry in trimmed.split(',') {
            let (group, format) = entry.split_once(':').ok_or_else(|| {
                anyhow!(
                    "\"{raw}\" (expected one format, or group:format entries such as \
                     value:numeric)"
                )
            })?;
            let group = group.trim();
            let format = format.trim();
            if group.is_empty() {
                bail!("\"{raw}\" has an empty group name before ':'");
            }
            if entries
                .insert(group.to_string(), parse_keyword_value(format)?)
                .is_some()
            {
                bail!("duplicate format for group \"{group}\"");
            }
        }
        Ok(Self(FormatsKind::ByGroup(entries)))
    }

    /// The format of `group`.
    pub(crate) fn of(&self, group: &str) -> Format {
        match &self.0 {
            FormatsKind::All(format) => *format,
            FormatsKind::ByGroup(formats) => {
                formats.get(group).copied().unwrap_or(Format::Verbatim)
            }
        }
    }

    /// Checks that every named group in a per-group `same-as-format` exists in `pattern`.
    ///
    /// # Errors
    /// Returns an error when a group name in `same-as-format` is absent from `pattern`.
    pub(super) fn validate_groups(&self, pattern: Option<&Regex>) -> anyhow::Result<()> {
        let groups_map = match &self.0 {
            // One format for every group refers to no group by name.
            FormatsKind::All(_) => return Ok(()),
            FormatsKind::ByGroup(groups_map) => groups_map,
        };
        let groups = pattern_groups(pattern);
        for group in groups_map.keys() {
            // A typo in a group would otherwise compare that group as written, with no sign that
            // its format is lost.
            if !groups.iter().any(|&known| known == group) {
                bail!(
                    "same-as-format refers to group \"{group}\", which same-as-pattern doesn't have"
                );
            }
        }
        Ok(())
    }
}

impl fmt::Display for Formats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            FormatsKind::All(format) => write!(f, "{format}"),
            FormatsKind::ByGroup(groups) => {
                for (index, (group, format)) in groups.iter().enumerate() {
                    if index > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{group}:{format}")?;
                }
                Ok(())
            }
        }
    }
}

/// How the records of the two sides are compared.
#[derive(Clone, Copy, Debug, PartialEq, Eq, EnumString, EnumIter, Display, Serialize)]
#[strum(ascii_case_insensitive, serialize_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub(crate) enum Mode {
    /// Order- and duplicate-insensitive set equality (the default with a `same-as-pattern`).
    Set,
    /// Order-sensitive list equality (the default without a `same-as-pattern`).
    Sequence,
    /// Exactly one value per side, compared directly.
    Single,
    /// Directional containment: every value in this block must also appear in the target.
    Subset,
}

impl Mode {
    /// Parses `raw` as a comparison mode after trimming whitespace.
    pub(super) fn parse(raw: &str) -> anyhow::Result<Self> {
        parse_keyword_value(raw.trim())
    }
}

/// The target of a `same-as-paths` attribute: files or directories matching a glob.
#[derive(Debug, Clone)]
pub(crate) struct TargetPaths {
    /// How the author wrote the glob, including any trailing `/` for directory matching.
    glob: String,
    matcher: PathsGlob,
    directories: bool,
}

impl TargetPaths {
    /// Parses `raw` as a `same-as-paths` glob, trimming whitespace and rejecting empty patterns.
    pub(super) fn parse(raw: &str) -> anyhow::Result<Self> {
        let trimmed = raw.trim();
        // The glob is written the way the block writes its paths, so a directory ends with `/`.
        let (pattern, directories) = match trimmed.strip_suffix('/') {
            Some(directory_glob) => (directory_glob, true),
            None => (trimmed, false),
        };
        if pattern.is_empty() {
            bail!("glob must not be empty");
        }
        let matcher = PathsGlob::new(pattern)?;
        Ok(Self {
            glob: trimmed.to_string(),
            matcher,
            directories,
        })
    }

    /// The glob text as written in the attribute.
    pub(crate) fn as_str(&self) -> &str {
        &self.glob
    }

    /// The repository files or directories that match this glob, sorted.
    pub(crate) fn matches(&self, file_system: &impl FileSystem) -> anyhow::Result<Vec<String>> {
        if self.directories {
            self.matcher.matching_directories(file_system)
        } else {
            Ok(self
                .matcher
                .matching_files(file_system)?
                .into_iter()
                .map(|path| path.to_string())
                .collect())
        }
    }
}
