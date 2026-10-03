use crate::Position;
use crate::blocks::{Block, BlockWithContext, Content, ContentText, Declaration};
use crate::diff_parser::{self, LineChange};
use crate::repo_path::RepoPath;
use crate::symbol_path::SymbolPath;
use crate::symbols::{Symbol, resolve};
use anyhow::anyhow;
use std::collections::HashMap;
use std::fmt;
use std::ops::Range;
use std::path::PathBuf;

/// A block that an entry of the config file declares around a symbol, instead of tags in a
/// comment. It is a block of the file the symbol is in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VirtualBlock {
    /// The file the symbol is in.
    pub(crate) file: RepoPath,
    /// The symbol's path in `file`.
    pub(crate) path: SymbolPath,
    /// The attributes, with the same names and values a tag gives them.
    pub(crate) attributes: HashMap<String, String>,
    /// The entry of the config file that declares the block.
    pub(crate) entry: ConfigEntry,
}

/// A `[[block]]` entry of the config file. It shows as `line 7 of "blockwatch.toml"`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigEntry {
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
    /// Finds the symbol this entry wraps, and returns the block for it.
    ///
    /// - `source` is the text of the file the symbol is in, such as `package.json`.
    /// - `symbols` are all the symbols of that file.
    /// - `line_changes` are the diff's changes to that file.
    ///
    /// The block counts as changed when the diff touches the symbol, its key included. A change
    /// elsewhere in the file does not count.
    ///
    /// # Errors
    /// Returns an error when:
    /// - the file has no symbol with this path, or two of them;
    /// - the symbol is a TOML table written in several places, such as `[package]` and
    ///   `[package.metadata]`.
    ///
    /// The error shows the line of the entry in the config file.
    pub(crate) fn resolve(
        &self,
        symbols: &[Symbol],
        source: &str,
        line_changes: &[LineChange],
    ) -> anyhow::Result<BlockWithContext> {
        let symbol = resolve(symbols, &self.path)
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
        let definition_positions = position_range(source, definition);
        // A violation of the whole block is shown on this range. Its first line is enough to find
        // the symbol, and it keeps a violation from covering a whole object.
        let first_line_end = source[definition.clone()]
            .find('\n')
            .map_or(definition.end, |offset| definition.start + offset);
        let start_tag_position_range =
            definition_positions.start.clone()..Position::from_byte_offset(source, first_line_end);
        // The content is what a reference to the symbol reads, so that the two always agree.
        let content = match &symbol.value {
            Some(value) => Content {
                text: ContentText::Decoded(value.text.clone()),
                positions: position_range(source, &value.byte_range),
            },
            None => Content {
                text: ContentText::Source(definition.clone()),
                positions: definition_positions.clone(),
            },
        };
        Ok(BlockWithContext {
            block: Block {
                attributes: self.attributes.clone(),
                start_tag_position_range,
                content,
                declaration: Declaration::ConfigEntry(self.entry.clone()),
            },
            is_content_modified: diff_parser::range_intersects_any(
                &definition_positions,
                line_changes,
            ),
            // The block's attributes are written in the config file, not in this one.
            is_start_tag_modified: false,
        })
    }

    /// The error for a target that does not resolve because of `cause`. It shows the target and
    /// the config line that declares the block.
    pub(crate) fn unresolved(&self, cause: anyhow::Error) -> anyhow::Error {
        cause
            .context(format!("target {} does not resolve", self.target()))
            .context(self.entry.error_context())
    }

    /// The block's target, as the config file writes it: `file#/path`.
    pub(crate) fn target(&self) -> String {
        format!("{}#{}", self.file, self.path)
    }
}

/// The positions in `source` of the bytes in `range`.
fn position_range(source: &str, range: &Range<usize>) -> Range<Position> {
    Position::from_byte_offset(source, range.start)..Position::from_byte_offset(source, range.end)
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;
    use crate::diff_parser::LineChangeKind;
    use crate::language_parsers::language_parsers;
    use crate::test_utils::virtual_block;
    use std::ffi::OsString;
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
        let symbols = language_parsers()?[&OsString::from(extension)]
            .lock()
            .expect("no active locks")
            .parse_symbols(source)?;
        virtual_block(&format!("{file}#{path}"), &[])?.resolve(&symbols, source, line_changes)
    }

    #[test]
    fn scalar_target_resolve_returns_its_decoded_value_as_content() -> anyhow::Result<()> {
        let source = "{\n  \"version\": \"1.2.3\"\n}\n";

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
        let source = "{\n  \"keywords\": [\n    \"b\",\n    \"a\"\n  ]\n}\n";

        let block = resolved("package.json", "/keywords", source, &[])?.block;

        assert_eq!(
            block.content.text(source),
            "\"keywords\": [\n    \"b\",\n    \"a\"\n  ]"
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
    fn diff_touching_the_definition_resolve_marks_the_content_modified() -> anyhow::Result<()> {
        let source = "{\n  \"name\": \"app\",\n  \"version\": \"1.2.3\"\n}\n";
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
        let source = "[package]\nname = \"a\"\n\n[other]\nx = 1\n\n[package.metadata]\ny = 2\n";

        let error = resolved("Cargo.toml", "/package", source, &[]).unwrap_err();

        assert_eq!(
            format!("{error:#}"),
            "invalid block at line 7 of \"blockwatch.toml\": target Cargo.toml#/package is \
             written in several places; wrap one of its keys instead"
        );
    }
}
