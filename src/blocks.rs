//! The `Block` type and the other types that describe the blocks of a file.

use crate::Position;
use crate::diff_parser::{self, LineChange, LineChangeKind, LineChanges};
use crate::repo_path::RepoPath;
use crate::validators::Validator;
use crate::virtual_blocks::ConfigEntry;
use anyhow::{Context, anyhow, bail};
use serde::Serialize;
use serde_repr::Serialize_repr;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::ops::Range;
use std::str::FromStr;
use strum::IntoEnumIterator;
use strum_macros::EnumString;

const UNNAMED_BLOCK_LABEL: &str = "(unnamed)";

/// A block: a part of a file that its attributes set rules for.
///
/// A pair of tags in the file's comments declares a block. So does a `[[block]]` entry of the
/// config file, around a symbol of the file. That is a virtual block.
#[derive(Debug, PartialEq, Eq, Clone)]
pub(crate) struct Block {
    /// Optional attributes in the `block` tag, or in the `[[block]]` entry of a virtual block.
    /// Their names are what selects the validators that will check this block (`affects`,
    /// `keep-sorted`, …).
    pub(crate) attributes: HashMap<String, String>,
    /// Block's start tag position range, half-open: it starts at the `<` symbol and ends one column
    /// past the `>` symbol. A virtual block has no tag, so it has the first line of the symbol's
    /// definition.
    pub(crate) start_tag_position_range: Range<Position>,
    /// The block's content.
    pub(crate) content: Content,
    /// Where the block's attributes are written.
    pub(crate) declaration: Declaration,
    /// How serious a violation of this block is. It is the `severity` attribute, or
    /// [`BlockSeverity::Error`] if the block has none.
    pub(crate) severity: BlockSeverity,
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

#[cfg(test)]
impl BlockKey {
    /// The line the block starts on.
    pub(crate) fn start_line(&self) -> usize {
        self.start.line
    }
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
    /// [`validate_attributes`] and [`BlockSeverity::from_attributes`].
    pub(crate) fn from_tags(
        attributes: HashMap<String, String>,
        start_tag_position_range: Range<Position>,
        content: Content,
    ) -> anyhow::Result<Self> {
        let severity = validate_attributes(&attributes)
            .and_then(|()| BlockSeverity::from_attributes(&attributes))
            .map_err(|error| {
                // The tag parser does not know the file, so the message cannot show it.
                let name = attributes
                    .get("name")
                    .map_or(UNNAMED_BLOCK_LABEL, String::as_str);
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
            severity,
        })
    }

    /// Returns the optional value of the `name` attribute for this block.
    pub(crate) fn name(&self) -> Option<&str> {
        self.attributes.get("name").map(String::as_str)
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
            config_line: match &self.declaration {
                Declaration::Tags => None,
                Declaration::ConfigEntry(entry) => Some(entry.line),
            },
        }
    }
}

/// Block's severity.
///
/// Mirrors [LSP DiagnosticSeverity](https://github.com/microsoft/vscode-languageserver-node/blob/3412a17149850f445bf35b4ad71148cfe5f8411e/types/src/main.ts#L614)
#[derive(Clone, Copy, Serialize_repr, EnumString, Debug, PartialEq, Eq)]
#[strum(ascii_case_insensitive)]
#[repr(u8)]
/* <block name="block-severity" same-as="docs/validators/README.md:severity-levels"
same-as-format="identifier" same-as-pattern="^(?P<value>[A-Z][a-z]+) = \d"> */
pub(crate) enum BlockSeverity {
    /// The default. A violation at this level makes the run exit non-zero, failing a hook or CI.
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
    /// The severity that `attributes` give a block: its `severity` attribute, or
    /// [`BlockSeverity::Error`] if it has none.
    ///
    /// # Errors
    /// Returns an error that quotes the value if it is not a severity.
    pub(crate) fn from_attributes(attributes: &HashMap<String, String>) -> anyhow::Result<Self> {
        match attributes.get("severity") {
            Some(value) => {
                Self::from_str(value).map_err(|_| anyhow!("unrecognized severity value `{value}`"))
            }
            None => Ok(Self::Error),
        }
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
    /// A map that is sorted, because the attributes of a block are not.
    attributes: BTreeMap<&'a str, &'a str>,
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
            attributes: block
                .attributes
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str()))
                .collect(),
            checks: None,
            column: block.start_tag_position_range.start.character,
            config_line: match &block.declaration {
                Declaration::Tags => None,
                Declaration::ConfigEntry(entry) => Some(entry.line),
            },
            is_content_modified: block_with_context.is_content_modified,
            line: block.start_tag_position_range.start.line,
            name: block.name_display(),
        }
    }
}

const RECOGNIZED_ATTRIBUTES: &[&str] = &[
    /* <block keep-sorted same-as="src/skill.md:validator-reference"
    same-as-pattern='"(?P<value>[a-z-]+)"'> */
    "affects",
    "check-ai",
    "check-ai-pattern",
    "check-lua",
    "check-lua-pattern",
    "check-lua-timeout",
    "keep-sorted",
    "keep-sorted-format",
    "keep-sorted-pattern",
    "keep-unique",
    "line-count",
    "line-count-pattern",
    "line-pattern",
    "line-pattern-forbidden",
    "name",
    "same-as",
    "same-as-format",
    "same-as-mode",
    "same-as-paths",
    "same-as-pattern",
    "severity",
    // </block>
];

/// Checks that each attribute of a block is recognized, and that each validator accepts the
/// attributes it reads (see [`Validator::check_attributes`]). It does not check the values: see
/// [`BlockSeverity::from_attributes`] for `severity`.
///
/// # Errors
/// Returns an error that quotes the first attribute found to be wrong, such as
/// ``unrecognized attribute `keep-sortd` ``.
pub(crate) fn validate_attributes(attributes: &HashMap<String, String>) -> anyhow::Result<()> {
    if let Some(name) = attributes
        .keys()
        .find(|name| !RECOGNIZED_ATTRIBUTES.contains(&name.as_str()))
    {
        bail!("unrecognized attribute `{name}`");
    }
    // A block can be in a file that the run reads only to find a target, and no validator runs on
    // such a block. Every parsed block passes here.
    Validator::iter().try_for_each(|validator| validator.check_attributes(attributes))
}

#[cfg(test)]
mod validate_attributes_tests {
    use crate::blocks::{RECOGNIZED_ATTRIBUTES, validate_attributes};
    use crate::validators::Validator;
    use std::collections::HashMap;
    use strum::IntoEnumIterator;

    #[test]
    fn single_attribute_returns_an_error_only_when_it_needs_another() {
        // A new attribute must either work alone or stop the run alone. Otherwise a block with
        // only that attribute would pass without being checked.
        let needs_no_other: Vec<&str> = Validator::iter()
            .map(Validator::as_str)
            .chain([
                "line-pattern-forbidden",
                "name",
                "same-as-paths",
                "severity",
            ])
            .collect();
        for name in RECOGNIZED_ATTRIBUTES {
            let attributes = HashMap::from([(name.to_string(), String::new())]);

            let result = validate_attributes(&attributes);

            assert_eq!(result.is_ok(), needs_no_other.contains(name), "{name}");
        }
    }
}

#[cfg(test)]
mod block_severity_from_str_tests {
    use crate::blocks::BlockSeverity;
    use std::collections::HashMap;

    /// Attributes holding only a `severity`, to test how that attribute is parsed.
    fn attributes_with_severity(severity: &str) -> HashMap<String, String> {
        HashMap::from([("severity".into(), severity.into())])
    }

    #[test]
    fn block_with_valid_severity_attribute_returns_correct_severity() {
        let severity = BlockSeverity::from_attributes(&attributes_with_severity("warning"));

        assert_eq!(severity.unwrap(), BlockSeverity::Warning);
    }

    #[test]
    fn block_with_mixed_case_severity_attribute_returns_correct_severity() {
        let severity = BlockSeverity::from_attributes(&attributes_with_severity("InFo"));

        assert_eq!(severity.unwrap(), BlockSeverity::Info);
    }

    #[test]
    fn block_without_severity_attribute_returns_error_severity() {
        let severity = BlockSeverity::from_attributes(&HashMap::new());

        assert_eq!(severity.unwrap(), BlockSeverity::Error);
    }

    #[test]
    fn block_with_invalid_severity_attribute_returns_error() {
        let severity = BlockSeverity::from_attributes(&attributes_with_severity("warn"));

        assert!(severity.is_err());
    }
}

#[cfg(test)]
mod block_address_tests {
    use crate::Position;
    use crate::blocks::{Block, BlockAddress, BlockSeverity, Content, Declaration};
    use crate::repo_path::RepoPath;
    use std::collections::HashMap;

    fn repo_path(path: &str) -> RepoPath {
        RepoPath::from_reference(path).expect("a valid repository path")
    }

    /// Builds a contentless block with `attributes`.
    fn block_with_attributes(attributes: &[(&str, &str)]) -> Block {
        Block {
            attributes: attributes
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect::<HashMap<_, _>>(),
            start_tag_position_range: Position::new(0, 0)..Position::new(0, 0),
            content: Content::source(0..0, Position::new(0, 0)..Position::new(0, 0)),
            declaration: Declaration::Tags,
            severity: BlockSeverity::Error,
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

    fn block_with_context(
        attributes: &[(&str, &str)],
        declaration: Declaration,
    ) -> BlockWithContext {
        BlockWithContext {
            block: Block {
                attributes: attributes
                    .iter()
                    .map(|(name, value)| (name.to_string(), value.to_string()))
                    .collect(),
                start_tag_position_range: Position::new(2, 4)..Position::new(2, 30),
                content: Content::source(0..0, Position::new(3, 1)..Position::new(3, 1)),
                declaration,
                severity: BlockSeverity::Error,
            },
            is_start_tag_modified: false,
            is_content_modified: true,
        }
    }

    #[test]
    fn virtual_block_with_checks_is_written_with_its_keys_sorted() -> anyhow::Result<()> {
        let block = block_with_context(
            &[
                ("name", "n"),
                ("line-count", "<=3"),
                ("keep-sorted", "asc"),
                ("affects", "a.md:b"),
            ],
            Declaration::ConfigEntry(ConfigEntry {
                file: PathBuf::from("blockwatch.toml"),
                line: 7,
            }),
        );
        let mut listing = BlockListing::from(&block);
        listing.checks = Some(BTreeSet::from([Validator::KeepSorted]));

        assert_eq!(
            serde_json::to_string(&listing)?,
            r#"{"attributes":{"affects":"a.md:b","keep-sorted":"asc","line-count":"<=3","name":"n"},"checks":["keep-sorted"],"column":4,"config_line":7,"is_content_modified":true,"line":2,"name":"n"}"#
        );
        Ok(())
    }

    #[test]
    fn unnamed_block_with_tags_is_written_without_checks_and_config_line() -> anyhow::Result<()> {
        let block = block_with_context(&[("keep-sorted", "asc")], Declaration::Tags);

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
    use crate::fs::PathFilter;
    use crate::fs::test_utils::FakeFileSystem;
    use crate::language_parsers::language_parsers;
    use crate::scan::{Scan, ScanMode};

    // <block name="supported-extensions">
    #[test]
    fn all_language_extensions_are_supported() -> anyhow::Result<()> {
        let parsers = language_parsers()?;
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

        let blocks_by_file = Scan::new(&file_system, &PathFilter::allow_all(), &parsers, &[])
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
