//! Blocks that the config file declares around a symbol, instead of tags in a comment.

use crate::LineIndex;
use crate::blocks::{Block, BlockSeverity, BlockWithContext, Content, Declaration};
use crate::diff_parser::LineChanges;
use crate::repo_path::RepoPath;
use crate::symbol_path::SymbolPath;
use crate::symbols::{Symbol, resolve};
use anyhow::{Context, anyhow, bail};
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

/// The compiled `target-pattern` of a config entry. Two are equal when their regexes are written
/// the same.
#[derive(Clone, Debug)]
pub(crate) struct TargetPattern(Regex);

impl TargetPattern {
    /// Compiles `pattern`, the `target-pattern` of a config entry.
    ///
    /// # Errors
    /// Returns an error if `pattern` is not a valid regex.
    pub(crate) fn new(pattern: &str) -> anyhow::Result<Self> {
        // Every other pattern runs on one line at a time, so its `^` and `$` match at the ends of
        // a line. They keep that meaning here, where the pattern runs on the whole file.
        let regex = RegexBuilder::new(pattern)
            .multi_line(true)
            .build()
            .context("invalid `target-pattern`")?;
        Ok(Self(regex))
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
            None if self.0.capture_names().any(|name| name == Some("value")) => bail!(
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ConfigEntry {
    /// The config file.
    pub(crate) file: PathBuf,
    /// The line of the entry's `[[block]]` header.
    pub(crate) line: usize,
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
    /// Finds the part of the file this entry wraps, and returns the block for it.
    ///
    /// - `symbols` are all the symbols of the file. Only a block that wraps a symbol reads them.
    /// - `source` is the text of the file, such as `package.json`.
    /// - `line_changes` are the diff's changes to that file.
    ///
    /// The block's content counts as changed when the diff touches the symbol with its key, or
    /// the text that the pattern matches, or any part of a whole file. A change elsewhere in the
    /// file does not count. Its start tag counts as changed when the diff touches its entry in the
    /// config file.
    ///
    /// # Errors
    /// Returns an error when:
    /// - the file has no symbol with this path, or two of them;
    /// - the symbol is a TOML table written in several places, such as `[package]` and
    ///   `[package.metadata]`;
    /// - the pattern matches no text, or more than once.
    ///
    /// The error shows the line of the entry in the config file.
    pub(crate) fn resolve(
        &self,
        symbols: &[Symbol],
        source: &str,
        line_changes: &LineChanges,
    ) -> anyhow::Result<BlockWithContext> {
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
        Ok(BlockWithContext {
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
            // plays the part of the start tag.
            is_start_tag_modified: self.is_entry_modified,
        })
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
        symbols: &[Symbol],
        source: &str,
        lines: &LineIndex,
    ) -> anyhow::Result<(Range<usize>, Content)> {
        let symbol = resolve(symbols, path)
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
    use crate::language_parsers::language_parsers;
    use crate::test_utils::virtual_block;
    use std::path::Path;

    /// The block that a [`virtual_block`] around `path` in `file` declares in `source`, the text of
    /// `file`, when the diff makes `line_changes` to it.
    fn resolved(
        file: &str,
        path: &str,
        source: &str,
        line_changes: &[LineChange],
    ) -> anyhow::Result<BlockWithContext> {
        let extension = Path::new(file)
            .extension()
            .expect("the file has an extension");
        let symbols = language_parsers()?
            .for_extension(extension)
            .expect("the extension has a parser")
            .lock()
            .expect("a thread panicked while it held the parser")
            .parse_symbols(source)?;
        virtual_block(&format!("{file}#{path}"), &[])?.resolve(
            &symbols,
            source,
            &line_changes.iter().cloned().collect(),
        )
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
        block.resolve(&[], source, &line_changes.iter().cloned().collect())
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
            .resolve(&[], "a\n", &LineChanges::default())?
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
}
