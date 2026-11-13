//! Blocks that the config file declares around a symbol, instead of tags in a comment.

use crate::LineIndex;
use crate::blocks::{Block, BlockSeverity, BlockWithContext, Content, Declaration};
use crate::diff_parser::LineChanges;
use crate::repo_path::RepoPath;
use crate::symbol_path::SymbolPath;
use crate::symbols::{ResolveError, Symbols};
use anyhow::{anyhow, bail};
use regex::{Captures, Regex, RegexBuilder};
use std::collections::HashMap;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::path::PathBuf;

/// A block that a `[[block]]` entry of the config file declares in one file, instead of tags in a
/// comment. It is a block of that file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct VirtualBlock {
    /// The file the block is in.
    pub(crate) file: RepoPath,
    /// How to find the block in `file`.
    pub(crate) locator: Locator,
    /// The attributes, with the same names and values a tag gives them.
    pub(crate) attributes: HashMap<String, String>,
    /// The entry of the config file that declares the block.
    pub(crate) entry: ConfigEntry,
    /// Whether the diff touches `entry`.
    pub(crate) is_entry_modified: bool,
    /// The block's severity, from its `severity` attribute.
    pub(crate) severity: BlockSeverity,
    /// Condition a file matched by a glob `target` must pass to receive this block.
    pub(crate) target_if: Option<TargetIf>,
}

/// Condition that a file matched by a glob target must pass to receive a [`VirtualBlock`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TargetIf {
    /// Symbol paths that must resolve in the file.
    pub(crate) symbols: Vec<SymbolPath>,
    /// Regexes that must each match the file at least once.
    pub(crate) patterns: Vec<TargetPattern>,
}

/// How to find a [`VirtualBlock`] in its file.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Locator {
    /// The symbol at this path.
    Symbol(SymbolPath),
    /// The text that the entry's `target-pattern` matches.
    Pattern(TargetPattern),
    /// The whole file.
    File,
}

/// A compiled regex of a config entry that runs on the whole file: its `target-pattern`, or an
/// item of its `target-if.patterns`. Two are equal when their regexes are written the same.
#[derive(Clone, Debug)]
pub(crate) struct TargetPattern(Regex);

impl TargetPattern {
    /// Compiles `pattern`.
    ///
    /// # Errors
    /// Returns an error if `pattern` is not a valid regex.
    pub(crate) fn new(pattern: &str) -> anyhow::Result<Self> {
        // Every other pattern runs on one line at a time, without its line end, so its `^` and `$`
        // match at the ends of a line, and `\r\n` counts as one line end. They keep that meaning
        // here, where the pattern runs on the whole file.
        let regex = RegexBuilder::new(pattern)
            .multi_line(true)
            .crlf(true)
            .build()?;
        Ok(Self(regex))
    }

    /// Whether the pattern has a group named `value`.
    pub(crate) fn has_value_group(&self) -> bool {
        self.0.capture_names().any(|name| name == Some("value"))
    }

    /// The byte ranges of every match of the pattern in `source`, in order.
    fn matches<'a>(&'a self, source: &'a str) -> impl Iterator<Item = Range<usize>> + 'a {
        self.0.find_iter(source).map(|found| found.range())
    }

    /// Finds the one place in `source` that the pattern matches. Returns the byte range of the
    /// whole match, and of the `value` group or else the whole match.
    ///
    /// # Errors
    /// Returns an error when the pattern matches no text in `source`, or more than once, or when
    /// it has a `value` group that is not in the match.
    fn find_once(&self, source: &str) -> anyhow::Result<(Range<usize>, Range<usize>)> {
        let matches: Vec<Captures> = self.0.captures_iter(source).collect();
        let captures = match matches.as_slice() {
            [captures] => captures,
            [] => bail!("target-pattern matches no text"),
            several => {
                // A broad pattern can match every line of a long file. A few lines are enough to
                // find the mistake.
                let index = LineIndex::new(source);
                let lines: Vec<String> = several
                    .iter()
                    .take(5)
                    .map(|captures| {
                        index
                            .position(captures.get_match().start())
                            .line
                            .to_string()
                    })
                    .collect();
                let more = match several.len() - lines.len() {
                    0 => String::new(),
                    hidden => format!(" and {hidden} more"),
                };
                bail!(
                    "target-pattern must match once, but matches at lines {}{more}",
                    lines.join(", ")
                );
            }
        };
        let whole_match = captures.get_match().range();
        // As in the `*-pattern` attributes, a `value` group picks the text that counts. Those skip a
        // match without the group, but this match is the block, so it can't be skipped.
        let value = match captures.name("value") {
            Some(value) => value.range(),
            None if self.has_value_group() => bail!(
                "target-pattern matches at line {}, but its value group does not",
                LineIndex::new(source).position(whole_match.start).line
            ),
            None => whole_match.clone(),
        };
        Ok((whole_match, value))
    }
}

// A `Regex` can't be compared or hashed, so the text of the pattern stands in for it.
impl PartialEq for TargetPattern {
    fn eq(&self, other: &Self) -> bool {
        self.0.as_str() == other.0.as_str()
    }
}

impl Eq for TargetPattern {}

impl Hash for TargetPattern {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.as_str().hash(state);
    }
}

/// A `[[block]]` entry of the config file. It shows as `line 7 of "blockwatch.toml"`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ConfigEntry {
    /// The config file.
    pub(crate) file: PathBuf,
    /// The line of the entry's `[[block]]` header.
    pub(crate) line: usize,
    /// The entry's position in the config file's `block` array, starting at 0. Inline tables can
    /// put two entries on one line, so only this tells them apart.
    pub(crate) index: usize,
}

impl ConfigEntry {
    /// The first line of an error about the entry, such as
    /// `invalid block at line 7 of "blockwatch.toml"`.
    pub(crate) fn error_context(&self) -> String {
        format!("invalid block at {self}")
    }
}

impl fmt::Display for ConfigEntry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "line {} of \"{}\"",
            self.line,
            self.file.display()
        )
    }
}

impl VirtualBlock {
    /// Finds the part of the file this entry wraps, and returns the block for it. Returns `None`
    /// when the file fails the block's `target-if`: a symbol it lists is missing, or a pattern it
    /// lists matches no text. A symbol that matches several elements still counts as present.
    ///
    /// - `symbols` are all the symbols of the file. Only a block that wraps a symbol, or whose
    ///   `target-if` lists symbols, reads them.
    /// - `source` is the text of the file, such as `package.json`.
    /// - `line_changes` are the diff's changes to that file.
    ///
    /// The block's content counts as changed when the diff touches the symbol with its key, or
    /// the text that the pattern matches, or any part of a whole file. A change elsewhere in the
    /// file does not count. Its start tag counts as changed when the diff touches its entry in the
    /// config file, the definition of a `target-if` symbol, or the text a `target-if` pattern
    /// matches.
    ///
    /// # Errors
    /// Returns an error when:
    /// - the file has no symbol with this path, or two of them;
    /// - the symbol is a TOML table written in several places, such as `[package]` and
    ///   `[package.metadata]`;
    /// - the pattern matches no text, or more than once;
    /// - a `target-if` symbol can't be read.
    ///
    /// The error shows the line of the entry in the config file.
    pub(crate) fn resolve(
        &self,
        symbols: &Symbols,
        source: &str,
        line_changes: &LineChanges,
    ) -> anyhow::Result<Option<BlockWithContext>> {
        // A file that fails `target-if` may lack the target, so `target-if` is checked first.
        let target_if_ranges = match self.target_if_ranges(symbols, source)? {
            Some(ranges) => ranges,
            None => return Ok(None),
        };
        let lines = LineIndex::new(source);
        let (definition, content) = match &self.locator {
            Locator::Symbol(path) => self.symbol_definition(path, symbols, source, &lines)?,
            Locator::Pattern(pattern) => self.pattern_match(pattern, source, &lines)?,
            Locator::File => {
                let file = 0..source.len();
                let content = Content::source(file.clone(), lines.range(&file));
                (file, content)
            }
        };
        let definition_positions = lines.range(&definition);
        // A violation of the whole block is shown on this range. Its first line is enough to find
        // the block, and it keeps a violation from covering a whole object or file.
        #[expect(
            clippy::string_slice,
            reason = "`definition` is a symbol, a match or the file"
        )]
        let first_line_end = source[definition.clone()]
            .find('\n')
            .map_or(definition.end, |offset| definition.start + offset);
        let start_tag_position_range = definition_positions.start..lines.position(first_line_end);
        Ok(Some(BlockWithContext {
            block: Block {
                attributes: self.attributes.clone(),
                start_tag_position_range,
                content,
                declaration: Declaration::ConfigEntry(self.entry.clone()),
                severity: self.severity,
            },
            is_content_modified: match self.locator {
                Locator::Symbol(_) | Locator::Pattern(_) => {
                    line_changes.touches(&definition_positions)
                }
                Locator::File => !line_changes.is_empty(),
            },
            // The block's attributes are written in its entry in the config file, so the entry
            // plays the part of the start tag. What `target-if` checks decides whether the file
            // gets the block at all, so a change to it counts the same.
            is_start_tag_modified: self.is_entry_modified
                || target_if_ranges
                    .iter()
                    .any(|range| line_changes.touches(&lines.range(range))),
        }))
    }

    /// Finds the symbol at `path` among `symbols`, the symbols of `source`, which `lines` indexes.
    /// Returns the byte range of its definition, key included, and the block's content.
    ///
    /// # Errors
    /// Returns an error when `symbols` have no symbol at `path`, or two of them, or when the
    /// symbol is written in several places.
    fn symbol_definition(
        &self,
        path: &SymbolPath,
        symbols: &Symbols,
        source: &str,
        lines: &LineIndex,
    ) -> anyhow::Result<(Range<usize>, Content)> {
        let symbol = symbols
            .resolve(path)
            .map_err(|error| self.unresolved(anyhow!(error.reason(source))))?;
        let definition = match symbol.def_byte_ranges.as_slice() {
            [definition] => definition,
            // The block's content would have gaps.
            _ => {
                return Err(anyhow!(
                    "target {} is written in several places; wrap one of its keys instead",
                    self.target()
                )
                .context(self.entry.error_context()));
            }
        };
        // The content is what a reference to the symbol reads, so that the two always agree.
        Ok((definition.clone(), symbol.content(source, lines)))
    }

    /// Finds the one place in `source` that `pattern` matches, with `lines` indexing `source`.
    /// Returns the byte range of the whole match, and the block's content.
    ///
    /// # Errors
    /// Returns an error when `pattern` matches no text in `source`, or more than once.
    fn pattern_match(
        &self,
        pattern: &TargetPattern,
        source: &str,
        lines: &LineIndex,
    ) -> anyhow::Result<(Range<usize>, Content)> {
        let (whole_match, value) = pattern
            .find_once(source)
            .map_err(|error| self.unresolved(error))?;
        Ok((
            whole_match,
            Content::source(value.clone(), lines.range(&value)),
        ))
    }

    /// The error for a target that does not resolve because of `cause`. It shows the target and
    /// the config line that declares the block.
    pub(crate) fn unresolved(&self, cause: anyhow::Error) -> anyhow::Error {
        cause
            .context(format!("target {} does not resolve", self.target()))
            .context(self.entry.error_context())
    }

    /// The byte ranges in `source`, with `symbols`, of what the block's `target-if` checks: the
    /// definition of each symbol, and each match of each pattern. Returns `None` when the file
    /// fails `target-if`, and no ranges for a block without `target-if`. A symbol that matches
    /// several elements gives the range of each one.
    ///
    /// # Errors
    /// Returns an error if a symbol's definition can't be read.
    fn target_if_ranges(
        &self,
        symbols: &Symbols,
        source: &str,
    ) -> anyhow::Result<Option<Vec<Range<usize>>>> {
        let target_if = match &self.target_if {
            Some(target_if) => target_if,
            None => return Ok(Some(Vec::new())),
        };
        let mut ranges = Vec::new();
        for pattern in &target_if.patterns {
            let len = ranges.len();
            ranges.extend(pattern.matches(source));
            if ranges.len() == len {
                return Ok(None);
            }
        }
        for path in &target_if.symbols {
            let matches = match symbols.resolve(path) {
                Ok(symbol) => vec![symbol],
                Err(ResolveError::AmbiguousSymbol { candidates }) => candidates,
                Err(ResolveError::AmbiguousFilter { keys, .. }) => keys,
                Err(ResolveError::NotFound { .. }) => return Ok(None),
                Err(ResolveError::Unreadable { reason }) => {
                    return Err(self.unresolved(anyhow!("{reason}")));
                }
            };
            ranges.extend(
                matches
                    .iter()
                    .flat_map(|symbol| symbol.def_byte_ranges.iter().cloned()),
            );
        }
        Ok(Some(ranges))
    }

    /// The block's target, as the config file writes it: `file#/path`, or `file` when the block
    /// wraps a pattern or the whole file.
    pub(crate) fn target(&self) -> String {
        match &self.locator {
            Locator::Symbol(path) => format!("{}#{path}", self.file),
            Locator::Pattern(_) | Locator::File => self.file.to_string(),
        }
    }
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;
    use crate::Position;
    use crate::diff_parser::{LineChange, LineChangeKind};
    use crate::language_parsers::LanguageParsers;
    use crate::test_utils::{virtual_block, virtual_block_with_target_if};
    use std::path::Path;

    /// The block that a [`virtual_block`] around `path` in `file` declares in `source`, the text of
    /// `file`, when the diff makes `line_changes` to it.
    fn resolved(
        file: &str,
        path: &str,
        source: &str,
        line_changes: &[LineChange],
    ) -> anyhow::Result<BlockWithContext> {
        let block = virtual_block(&format!("{file}#{path}"), &[])?.resolve(
            &symbols_of(file, source)?,
            source,
            &line_changes.iter().cloned().collect(),
        )?;
        Ok(block.expect("a block without target-if is always found"))
    }

    /// The symbols of `source`, the text of `file`.
    fn symbols_of(file: &str, source: &str) -> anyhow::Result<Symbols> {
        LanguageParsers::new()
            .parser_for(Path::new(file))
            .expect("the file has a parser")
            .lock()
            .expect("a thread panicked while it held the parser")
            .parse_symbols(source)
    }

    /// Whether `source`, the text of `file`, gets a block from a [`virtual_block_with_target_if`] that
    /// lists `target_if_symbols` and `target_if_patterns`.
    fn gets_block(
        file: &str,
        source: &str,
        target_if_symbols: &[&str],
        target_if_patterns: &[&str],
    ) -> anyhow::Result<bool> {
        let block = virtual_block_with_target_if(file, &[], target_if_symbols, target_if_patterns)?
            .resolve(&symbols_of(file, source)?, source, &LineChanges::default())?;
        Ok(block.is_some())
    }

    /// The block that a [`virtual_block`] in `notes.txt`, found by `locator`, declares in
    /// `source`, the text of `notes.txt`, when the diff makes `line_changes` to it.
    fn resolved_in_text(
        locator: Locator,
        source: &str,
        line_changes: &[LineChange],
    ) -> anyhow::Result<BlockWithContext> {
        let mut block = virtual_block("notes.txt", &[])?;
        block.locator = locator;
        let block = block.resolve(
            &Symbols::default(),
            source,
            &line_changes.iter().cloned().collect(),
        )?;
        Ok(block.expect("a block without target-if is always found"))
    }

    /// What a `target-pattern` of `pattern` wraps.
    fn pattern(pattern: &str) -> Locator {
        Locator::Pattern(TargetPattern::new(pattern).expect("the pattern is a valid regex"))
    }

    #[test]
    fn scalar_target_resolve_returns_its_decoded_value_as_content() -> anyhow::Result<()> {
        let source = r#"{
  "version": "1.2.3"
}
"#;

        let block = resolved("package.json", "/version", source, &[])?.block;

        assert_eq!(block.content.text(source), "1.2.3");
        // Every position in the value is where the value starts, just after the key.
        assert_eq!(block.content.position(0, 2), Position::new(2, 14));
        assert_eq!(
            block.start_tag_position_range,
            Position::new(2, 3)..Position::new(2, 21)
        );
        Ok(())
    }

    #[test]
    fn container_target_resolve_returns_its_definition_as_content() -> anyhow::Result<()> {
        let source = r#"{
  "keywords": [
    "b",
    "a"
  ]
}
"#;

        let block = resolved("package.json", "/keywords", source, &[])?.block;

        assert_eq!(
            block.content.text(source),
            r#""keywords": [
    "b",
    "a"
  ]"#
        );
        assert_eq!(block.content.position(2, 4), Position::new(4, 5));
        // Only the first line of the definition, as a tag is usually one line.
        assert_eq!(
            block.start_tag_position_range,
            Position::new(2, 3)..Position::new(2, 16)
        );
        Ok(())
    }

    #[test]
    fn section_target_resolve_returns_its_body_with_the_position_of_each_line() -> anyhow::Result<()>
    {
        let source = r"# Colors

## Names

- blue
- red
- green
";

        let block = resolved("colors.md", "/Colors/Names", source, &[])?.block;

        assert_eq!(block.content.text(source), "- blue\n- red\n- green");
        // The body is written as is, so a line of it has its own position, unlike a decoded value.
        let position = block.content.position(2, 2);
        let line = source
            .lines()
            .nth(position.line - 1)
            .expect("a line of the source");
        let text: String = line.chars().skip(position.character - 1).collect();
        assert_eq!(text, "green");
        assert_eq!(
            block.start_tag_position_range,
            Position::new(3, 1)..Position::new(3, 9)
        );
        Ok(())
    }

    #[test]
    fn pattern_target_resolve_returns_the_match_as_content() -> anyhow::Result<()> {
        let source = "CREATE TABLE a (
  id INT
);
CREATE TABLE b (
  id INT,
  name TEXT
);
";

        // `^` matches at the start of any line, not only of the file.
        let block = resolved_in_text(pattern(r"(?s)^CREATE TABLE b .*?^\);"), source, &[])?.block;

        assert_eq!(
            block.content.text(source),
            "CREATE TABLE b (
  id INT,
  name TEXT
);"
        );
        assert_eq!(block.content.position(2, 2), Position::new(6, 3));
        assert_eq!(
            block.start_tag_position_range,
            Position::new(4, 1)..Position::new(4, 17)
        );
        Ok(())
    }

    #[test]
    fn crlf_line_end_resolve_counts_as_one_line_end() -> anyhow::Result<()> {
        // `$` matches before the `\r`, as in the patterns that run on one line at a time.
        let source = "TIMEOUT = 5\r\nMODEL = a\r\n";

        let block = resolved_in_text(pattern("^MODEL = .*$"), source, &[])?.block;
        assert_eq!(block.content.text(source), "MODEL = a");

        let yaml = "name: a\r\ndriver: postgres\r\n";
        assert!(gets_block("a.yml", yaml, &[], &["^driver: postgres$"])?);
        Ok(())
    }

    #[test]
    fn pattern_with_a_value_group_resolve_returns_the_group_as_content() -> anyhow::Result<()> {
        let source = r#"MODEL = "model-b"  # the default
"#;

        let block = resolved_in_text(pattern(r#"^MODEL = "(?P<value>[^"]+)""#), source, &[])?.block;

        assert_eq!(block.content.text(source), "model-b");
        assert_eq!(block.content.position(0, 0), Position::new(1, 10));
        // The whole match, as a symbol's range includes its key.
        assert_eq!(
            block.start_tag_position_range,
            Position::new(1, 1)..Position::new(1, 18)
        );
        Ok(())
    }

    #[test]
    fn whole_file_target_resolve_returns_the_file_as_content() -> anyhow::Result<()> {
        let source = "a\nb\n";

        let block = resolved_in_text(Locator::File, source, &[])?.block;

        assert_eq!(block.content.text(source), source);
        assert_eq!(
            block.start_tag_position_range,
            Position::new(1, 1)..Position::new(1, 2)
        );
        Ok(())
    }

    #[test]
    fn severity_attribute_resolve_returns_a_block_with_that_severity() -> anyhow::Result<()> {
        let block = virtual_block("notes.txt", &[("severity", "warning")])?
            .resolve(&Symbols::default(), "a\n", &LineChanges::default())?
            .expect("a block without target-if is always found")
            .block;

        assert_eq!(block.severity, BlockSeverity::Warning);
        Ok(())
    }

    #[test]
    fn diff_touching_the_definition_resolve_marks_the_content_modified() -> anyhow::Result<()> {
        let source = r#"{
  "name": "app",
  "version": "1.2.3"
}
"#;
        // Columns 3 to 11 of line 3 are the key, before the value starts.
        let key_modified = LineChange {
            line: 3,
            kind: LineChangeKind::Modified(vec![2..11]),
        };
        let line_above_added = LineChange {
            line: 2,
            kind: LineChangeKind::Added,
        };

        let modified = |line_change| -> anyhow::Result<bool> {
            Ok(resolved("package.json", "/version", source, &[line_change])?.is_content_modified)
        };

        assert!(modified(key_modified)?);
        assert!(!modified(line_above_added)?);
        Ok(())
    }

    #[test]
    fn diff_touching_the_match_resolve_marks_the_content_modified() -> anyhow::Result<()> {
        let source = r#"TIMEOUT = 5
MODEL = "model-b"  # the default
"#;
        // Columns 1 to 5 of line 2 are `MODEL`, before the value group.
        let name_modified = LineChange {
            line: 2,
            kind: LineChangeKind::Modified(vec![0..5]),
        };
        // Columns 20 to 32 of line 2 are the comment, after the match.
        let comment_modified = LineChange {
            line: 2,
            kind: LineChangeKind::Modified(vec![19..32]),
        };
        let line_above_modified = LineChange {
            line: 1,
            kind: LineChangeKind::Modified(vec![0..7]),
        };

        let modified = |line_change| -> anyhow::Result<bool> {
            Ok(resolved_in_text(
                pattern(r#"^MODEL = "(?P<value>[^"]+)""#),
                source,
                &[line_change],
            )?
            .is_content_modified)
        };

        assert!(modified(name_modified)?);
        assert!(!modified(comment_modified)?);
        assert!(!modified(line_above_modified)?);
        Ok(())
    }

    #[test]
    fn diff_deleting_the_first_line_of_a_whole_file_resolve_marks_it_modified() -> anyhow::Result<()>
    {
        let first_line_deleted = LineChange {
            line: 1,
            kind: LineChangeKind::Deleted,
        };

        let block = resolved_in_text(Locator::File, "b\nc\n", &[first_line_deleted])?;

        assert!(block.is_content_modified);
        Ok(())
    }

    #[test]
    fn missing_symbol_resolve_returns_error_with_hints() {
        let error = resolved("package.json", "/versoin", r#"{"version": "1"}"#, &[]).unwrap_err();

        assert_eq!(
            format!("{error:#}"),
            "invalid block at line 7 of \"blockwatch.toml\": target package.json#/versoin does \
             not resolve: symbol not found; did you mean: /version"
        );
    }

    #[test]
    fn table_written_in_several_places_resolve_returns_error() {
        let source = r#"[package]
name = "a"

[other]
x = 1

[package.metadata]
y = 2
"#;

        let error = resolved("Cargo.toml", "/package", source, &[]).unwrap_err();

        assert_eq!(
            format!("{error:#}"),
            "invalid block at line 7 of \"blockwatch.toml\": target Cargo.toml#/package is \
             written in several places; wrap one of its keys instead"
        );
    }

    #[test]
    fn pattern_matching_nothing_resolve_returns_error() {
        let error = resolved_in_text(pattern("^MODEL ="), "TIMEOUT = 5\n", &[]).unwrap_err();

        assert_eq!(
            format!("{error:#}"),
            "invalid block at line 7 of \"blockwatch.toml\": target notes.txt does not resolve: \
             target-pattern matches no text"
        );
    }

    #[test]
    fn pattern_matching_twice_resolve_returns_error_with_the_start_lines() {
        let source = r#"MODELS = [
  "a",
]
TIMEOUT = 5
MODELS = [
  "b",
]
"#;

        let error = resolved_in_text(pattern(r"(?s)^MODELS = \[.*?^\]"), source, &[]).unwrap_err();

        // The lines where the matches start.
        assert_eq!(
            format!("{error:#}"),
            "invalid block at line 7 of \"blockwatch.toml\": target notes.txt does not resolve: \
             target-pattern must match once, but matches at lines 1, 5"
        );
    }

    #[test]
    fn pattern_matching_more_than_five_times_resolve_returns_error_with_the_first_five_lines() {
        for (source, lines) in [
            ("x\nx\nx\nx\nx\n", "1, 2, 3, 4, 5"),
            ("x\nx\nx\nx\nx\nx\nx\n", "1, 2, 3, 4, 5 and 2 more"),
        ] {
            let error = resolved_in_text(pattern("^x"), source, &[]).unwrap_err();

            assert_eq!(
                format!("{error:#}"),
                format!(
                    "invalid block at line 7 of \"blockwatch.toml\": target notes.txt does not \
                     resolve: target-pattern must match once, but matches at lines {lines}"
                )
            );
        }
    }

    #[test]
    fn pattern_with_a_value_group_not_in_the_match_resolve_returns_error() {
        // `none` matches the branch without the `value` group.
        let error = resolved_in_text(
            pattern(r#"^MODEL = (?:none|"(?P<value>[^"]+)")"#),
            "TIMEOUT = 5\nMODEL = none\n",
            &[],
        )
        .unwrap_err();

        assert_eq!(
            format!("{error:#}"),
            "invalid block at line 7 of \"blockwatch.toml\": target notes.txt does not resolve: \
             target-pattern matches at line 2, but its value group does not"
        );
    }

    #[test]
    fn target_patterns_with_different_text_are_not_equal() -> anyhow::Result<()> {
        // `a+` and `aa*` match the same text, but only the text of a pattern is compared.
        assert_ne!(TargetPattern::new("a+")?, TargetPattern::new("aa*")?);
        Ok(())
    }

    #[test]
    fn target_if_with_present_symbols_resolve_returns_a_block() -> anyhow::Result<()> {
        let yaml = "flag: true\nextra: 1\n";

        assert!(gets_block("a.yml", yaml, &["#/flag"], &[])?);
        assert!(gets_block("a.yml", yaml, &["#/flag", "#/extra"], &[])?);
        Ok(())
    }

    #[test]
    fn target_if_with_a_missing_symbol_resolve_returns_no_block() -> anyhow::Result<()> {
        let yaml = "extra: 1\n";

        assert!(!gets_block("a.yml", yaml, &["#/flag"], &[])?);
        assert!(!gets_block("a.yml", yaml, &["#/extra", "#/flag"], &[])?);
        Ok(())
    }

    #[test]
    fn target_if_with_patterns_resolve_returns_a_block_only_when_each_one_matches()
    -> anyhow::Result<()> {
        // `^` and `$` match at the ends of each line, so a pattern can match a line in the middle.
        let yaml = "name: a\ndriver: postgres\nport: 1\n";
        let rows: [(&[&str], &[&str], bool); 8] = [
            (&[], &["^driver: postgres$"], true),
            (&[], &["^driver: mysql$"], false),
            (&[], &["^name: a$", "^driver: postgres$"], true),
            (&[], &["^name: a$", "^driver: mysql$"], false),
            // A pattern that matches several times still passes.
            (&[], &["^[a-z]+:"], true),
            // A file must pass both `symbols` and `patterns`.
            (&["#/name"], &["^driver: postgres$"], true),
            (&["#/name"], &["^driver: mysql$"], false),
            (&["#/missing"], &["^driver: postgres$"], false),
        ];

        for (target_if_symbols, target_if_patterns, expected) in rows {
            assert_eq!(
                gets_block("a.yml", yaml, target_if_symbols, target_if_patterns)?,
                expected,
                "{target_if_symbols:?} {target_if_patterns:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn target_if_with_ambiguous_symbol_or_filter_resolve_returns_a_block() -> anyhow::Result<()> {
        // Markdown lets a heading appear twice, so `/A` resolves to `AmbiguousSymbol`.
        let markdown = "# A\none\n# A\ntwo\n";
        match symbols_of("doc.md", markdown)?.resolve(&SymbolPath::parse("/A")?) {
            Err(ResolveError::AmbiguousSymbol { .. }) => {}
            other => panic!("expected AmbiguousSymbol, got {other:?}"),
        }
        assert!(gets_block("doc.md", markdown, &["#/A"], &[])?);

        // Two items with the same `name` make `~[name=a]` resolve to `AmbiguousFilter`.
        let yaml = "items:\n  - name: a\n  - name: a\n";
        match symbols_of("a.yml", yaml)?.resolve(&SymbolPath::parse("/items/~[name=a]")?) {
            Err(ResolveError::AmbiguousFilter { .. }) => {}
            other => panic!("expected AmbiguousFilter, got {other:?}"),
        }
        assert!(gets_block("a.yml", yaml, &["#/items/~[name=a]"], &[])?);
        Ok(())
    }

    #[test]
    fn target_if_with_unreadable_symbol_resolve_returns_error() -> anyhow::Result<()> {
        let symbols = Symbols::with_unreadable(
            Vec::new(),
            HashMap::from([(SymbolPath::parse("/flag")?, "cannot read".to_string())]),
        );

        let error = virtual_block_with_target_if("a.yml", &[], &["#/flag"], &[])?
            .resolve(&symbols, "flag: true\n", &LineChanges::default())
            .unwrap_err();

        assert_eq!(
            format!("{error:#}"),
            "invalid block at line 7 of \"blockwatch.toml\": target a.yml does not resolve: \
             cannot read"
        );
        Ok(())
    }

    #[test]
    fn diff_touching_a_target_if_symbol_resolve_marks_the_start_tag_modified() -> anyhow::Result<()>
    {
        // A change to a `target-if` symbol can give a file the block or take it away, so it
        // counts like a change to the entry. The content stays unchanged.
        let block_for =
            |target: &str, target_if_symbol: &str, source: &str, line_changed: usize| {
                let file = target.split_once('#').map_or(target, |(file, _)| file);
                let line_change = LineChange {
                    line: line_changed,
                    kind: LineChangeKind::Added,
                };
                let block = virtual_block_with_target_if(target, &[], &[target_if_symbol], &[])?
                    .resolve(
                        &symbols_of(file, source)?,
                        source,
                        &[line_change].into_iter().collect(),
                    )?
                    .expect("the file passes target-if");
                anyhow::Ok(block)
            };
        let yaml = "flag: true\nother: 1\nvalue: 2\n";
        // `/A` matches both `# A` sections.
        let markdown = "# A\none\n# A\ntwo\n# B\nthree\n";
        // `~[name=a]` matches both items.
        let items = "items:\n  - name: a\n  - name: a\nvalue: 1\n";
        // `[package]` is written in two places.
        let toml = "[package]\nname = \"a\"\n\n[other]\nx = 1\n\n[package.metadata]\ny = 2\n";

        // Each row: the target, the `target-if` symbol, the file, the line the diff adds, and
        // whether the start tag and the content count as changed.
        for (
            target,
            target_if_symbol,
            source,
            line_changed,
            (start_tag_modified, content_modified),
        ) in [
            ("a.yml#/value", "#/flag", yaml, 1, (true, false)),
            ("a.yml#/value", "#/flag", yaml, 2, (false, false)),
            ("a.yml#/value", "#/flag", yaml, 3, (false, true)),
            ("doc.md#/B", "#/A", markdown, 3, (true, false)),
            ("a.yml#/value", "#/items/~[name=a]", items, 3, (true, false)),
            ("Cargo.toml#/other/x", "#/package", toml, 7, (true, false)),
        ] {
            let block = block_for(target, target_if_symbol, source, line_changed)?;
            let row = format!("{target} with {target_if_symbol}, line {line_changed}");
            assert_eq!(block.is_start_tag_modified, start_tag_modified, "{row}");
            assert_eq!(block.is_content_modified, content_modified, "{row}");
        }
        Ok(())
    }

    #[test]
    fn diff_touching_a_target_if_pattern_match_resolve_marks_the_start_tag_modified()
    -> anyhow::Result<()> {
        // A change to the text a `target-if` pattern matches can give a file the block or take it
        // away, so it counts like a change to the entry. The content stays unchanged.
        let source = "mode = on\nother = 1\nvalue = 2\nmode = on\n";
        let block_for = |line_changed: usize| {
            let mut block = virtual_block_with_target_if("notes.txt", &[], &[], &["^mode = on$"])?;
            block.locator = pattern("^value = .*");
            let line_change = LineChange {
                line: line_changed,
                kind: LineChangeKind::Added,
            };
            let block = block
                .resolve(
                    &Symbols::default(),
                    source,
                    &[line_change].into_iter().collect(),
                )?
                .expect("the file passes target-if");
            anyhow::Ok(block)
        };

        // Each row: the line the diff adds, and whether the start tag and the content count as
        // changed.
        for (line_changed, (start_tag_modified, content_modified)) in [
            (1, (true, false)),
            (2, (false, false)),
            (3, (false, true)),
            // Every match counts, not only the first.
            (4, (true, false)),
        ] {
            let block = block_for(line_changed)?;
            let row = format!("line {line_changed}");
            assert_eq!(block.is_start_tag_modified, start_tag_modified, "{row}");
            assert_eq!(block.is_content_modified, content_modified, "{row}");
        }
        Ok(())
    }
}
