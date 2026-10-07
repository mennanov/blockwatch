use crate::Position;
use crate::language_parsers::{
    CommentsParser, LanguageParser, LanguageParserImpl, python_style_comments_parser,
};
use crate::symbol_path::SymbolPath;
use crate::symbols::{ScalarValue, Symbol, SymbolsParser};
use anyhow::{Context, anyhow};
use std::ops::Range;
use toml_edit::{InlineTable, Item, Key, Table, Value};

/// Returns a [`LanguageParser`] for Toml.
pub(super) fn parser() -> anyhow::Result<impl LanguageParser> {
    Ok(LanguageParserImpl::new(comments_parser()?).with_symbols(TomlSymbolsParser))
}

/// The symbols of `source`, a TOML document, in document order.
///
/// # Errors
/// Returns an error if `source` is not valid TOML.
pub(crate) fn parse_symbols(source: &str) -> anyhow::Result<Vec<Symbol>> {
    TomlSymbolsParser.parse(source)
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let toml_language = tree_sitter_toml_ng::LANGUAGE.into();
    let parser = python_style_comments_parser(&toml_language, "comment");
    Ok(parser)
}

/// A [`SymbolsParser`] that derives the symbols of a TOML file from TOML's own data model, at the
/// paths TOML gives them.
///
/// Every key is a symbol, and so is every table, array of tables and array member. A table covers
/// every place it is written: the section under its own header, the lines its dotted keys are
/// on, and the tables inside it. A string value is decoded, and any other scalar is taken as
/// written.
struct TomlSymbolsParser;

impl SymbolsParser for TomlSymbolsParser {
    /// # Errors
    /// Returns an error if `source` is not valid TOML, a duplicate key included. The error shows
    /// the position of the problem.
    fn parse(&mut self, source: &str) -> anyhow::Result<Vec<Symbol>> {
        // What a key means depends on the headers above it, not on how the syntax tree nests:
        // `[fruits.physical]` belongs to the last `[[fruits]]` before it. So the symbols come from
        // the document `toml_edit` builds, not from a tree-sitter query.
        let document = toml_edit::Document::parse(source)
            .map_err(|error| toml_error_as_error(source, &error))?;
        Walk::symbols_in(source, document.as_table())
    }
}

/// The error for a `source` that `toml_edit` rejected with `error`.
fn toml_error_as_error(source: &str, error: &toml_edit::TomlError) -> anyhow::Error {
    match error.span() {
        Some(span) => {
            let position = Position::from_byte_offset(source, span.start);
            anyhow!(
                "file is not valid TOML at line {}, column {}: {}",
                position.line,
                position.character,
                error.message(),
            )
        }
        None => anyhow!("file is not valid TOML: {}", error.message()),
    }
}

/// What a walk of a TOML document has derived so far.
struct Walk<'s> {
    source: &'s str,
    /// The keys from the root of the document to the item the walk is at.
    path: Vec<String>,
    symbols: Vec<Symbol>,
}

/// Where the contents of a table are written.
#[derive(Default)]
struct Placement {
    /// The lines in the section of the nearest header above: the table's own pairs, and those
    /// written with dotted keys. Each ends after its line break.
    in_section: Vec<Range<usize>>,
    /// The sections of the headers inside the table.
    sections: Vec<Range<usize>>,
}

impl Placement {
    fn into_ranges(self) -> Vec<Range<usize>> {
        merged(self.in_section.into_iter().chain(self.sections).collect())
    }
}

impl<'s> Walk<'s> {
    /// The symbols of every key in `table`, the root table of a document in `source`, in document
    /// order.
    ///
    /// # Errors
    /// Returns an error if `toml_edit` gives no position for an element of the document.
    fn symbols_in(source: &'s str, table: &Table) -> anyhow::Result<Vec<Symbol>> {
        let mut walk = Walk {
            source,
            path: Vec::new(),
            symbols: Vec::new(),
        };
        // The document itself is not a symbol, so where its keys are written is not needed.
        walk.add_key_symbols(table)?;
        let mut symbols = walk.symbols;
        // The walk visits one table at a time, so the entries of an array of tables come together
        // even when other tables are written between them. A stable sort keeps a table ahead of a
        // key written at the same place.
        symbols.sort_by_key(|symbol| symbol.def_byte_ranges[0].start);
        Ok(symbols)
    }

    /// Adds a symbol at the current path, and returns where it is in `symbols`. The ranges can be
    /// left empty, and set once the symbol's contents have been walked.
    fn add_symbol(
        &mut self,
        def_byte_ranges: Vec<Range<usize>>,
        value: Option<ScalarValue>,
    ) -> usize {
        self.symbols.push(Symbol {
            path: SymbolPath::from_segments(self.path.clone()),
            def_byte_ranges,
            value,
        });
        self.symbols.len() - 1
    }

    /// Adds a symbol for every key in `table`, and returns where its contents are written.
    fn add_key_symbols(&mut self, table: &Table) -> anyhow::Result<Placement> {
        let mut placement = Placement::default();
        for (name, item) in table.iter() {
            self.path.push(name.to_string());
            match item {
                Item::None => {}
                Item::Value(value) => {
                    let start = require_span(table.key(name).and_then(Key::span))?.start;
                    for range in self.add_value_symbol(start, value)? {
                        placement
                            .in_section
                            .push(self.extended_past_line_break(range));
                    }
                }
                Item::Table(inner) => {
                    let inner = self.add_table_symbol(inner)?;
                    placement.in_section.extend(inner.in_section);
                    placement.sections.extend(inner.sections);
                }
                Item::ArrayOfTables(array) => {
                    let at = self.add_symbol(Vec::new(), None);
                    let mut ranges = Vec::new();
                    for (index, entry) in array.iter().enumerate() {
                        self.path.push(index.to_string());
                        ranges.extend(self.add_table_symbol(entry)?.into_ranges());
                        self.path.pop();
                    }
                    self.symbols[at].def_byte_ranges = merged(ranges.clone());
                    placement.sections.extend(ranges);
                }
            }
            self.path.pop();
        }
        Ok(placement)
    }

    /// Adds a symbol for `table` at the current path, and one for every key in it. Returns where
    /// the table is written.
    fn add_table_symbol(&mut self, table: &Table) -> anyhow::Result<Placement> {
        let at = self.add_symbol(Vec::new(), None);
        let contents = self.add_key_symbols(table)?;
        // A table made only by a dotted key, or only by a longer header, has no section of its
        // own. Its pairs are written in the section of the header above it.
        let placement = if table.is_dotted() || table.is_implicit() {
            contents
        } else {
            let header = require_span(table.span())?;
            let end = contents.in_section.iter().map(|range| range.end).fold(
                self.extended_past_line_break(header.clone()).end,
                usize::max,
            );
            let section = header.start..end;
            let mut sections = vec![section];
            sections.extend(contents.sections);
            Placement {
                in_section: Vec::new(),
                sections,
            }
        };
        self.symbols[at].def_byte_ranges = merged(
            placement
                .in_section
                .iter()
                .chain(&placement.sections)
                .cloned()
                .collect(),
        );
        Ok(placement)
    }

    /// Adds a symbol for `value` at the current path, written from `start`, and one for every
    /// member of `value`. Returns where the symbol is written.
    fn add_value_symbol(
        &mut self,
        start: usize,
        value: &Value,
    ) -> anyhow::Result<Vec<Range<usize>>> {
        match value {
            Value::String(string) => {
                let written = require_span(value.span())?;
                let range = start..written.end;
                let scalar = ScalarValue {
                    text: string.value().clone(),
                    byte_range: written,
                };
                self.add_symbol(vec![range.clone()], Some(scalar));
                Ok(vec![range])
            }
            Value::Integer(_) | Value::Float(_) | Value::Boolean(_) | Value::Datetime(_) => {
                let written = require_span(value.span())?;
                let range = start..written.end;
                let scalar = ScalarValue {
                    text: self.source[written.clone()].to_string(),
                    byte_range: written,
                };
                self.add_symbol(vec![range.clone()], Some(scalar));
                Ok(vec![range])
            }
            Value::Array(array) => {
                let range = start..require_span(value.span())?.end;
                self.add_symbol(vec![range.clone()], None);
                for (index, member) in array.iter().enumerate() {
                    self.path.push(index.to_string());
                    self.add_value_symbol(require_span(member.span())?.start, member)?;
                    self.path.pop();
                }
                Ok(vec![range])
            }
            Value::InlineTable(table) => {
                let at = self.add_symbol(Vec::new(), None);
                let members = self.add_inline_key_symbols(table)?;
                // A dotted key inside an inline table, such as `y` in `{ y.z = 1 }`, is a table
                // written only where its members are.
                let ranges = if table.is_dotted() {
                    merged(members)
                } else {
                    let range = start..require_span(value.span())?.end;
                    vec![range]
                };
                self.symbols[at].def_byte_ranges = ranges.clone();
                Ok(ranges)
            }
        }
    }

    /// Adds a symbol for every key in the inline table `table`, and returns where its members are
    /// written.
    fn add_inline_key_symbols(&mut self, table: &InlineTable) -> anyhow::Result<Vec<Range<usize>>> {
        let mut ranges = Vec::new();
        for (name, member) in table.iter() {
            self.path.push(name.to_string());
            let start = require_span(table.key(name).and_then(Key::span))?.start;
            ranges.extend(self.add_value_symbol(start, member)?);
            self.path.pop();
        }
        Ok(ranges)
    }

    /// `range` extended to just after the line break that ends its last line, or to the end of
    /// the source when no line break follows.
    fn extended_past_line_break(&self, range: Range<usize>) -> Range<usize> {
        // `range_intersects_any` places a deleted line at the start of the line after it. Ending
        // after the line break puts that point inside the range, so deleting a table's last pair
        // counts as touching the table.
        let end = self.source[range.end..]
            .find('\n')
            .map_or(self.source.len(), |offset| range.end + offset + 1);
        range.start..end
    }
}

/// The byte range `toml_edit` gives an element of a parsed document.
///
/// # Errors
/// Returns an error if it gives none, which it does only for an element it did not parse.
fn require_span(span: Option<Range<usize>>) -> anyhow::Result<Range<usize>> {
    span.context("toml_edit gave no position for an element of a parsed document")
}

/// `ranges` in order, with the ones that overlap or touch joined into one.
fn merged(mut ranges: Vec<Range<usize>>) -> Vec<Range<usize>> {
    ranges.sort_by_key(|range| range.start);
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
    for range in ranges {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            Some(_) | None => merged.push(range),
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Position, language_parsers::Comment};

    #[test]
    fn source_with_comments_parse_returns_every_comment() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser()?;

        let blocks: Vec<Comment> = comments_parser
            .parse(
                r#"
# This is a TOML file
title = "TOML Example" # Inline comment
[owner]
# Owner's details
name = "Tom Preston-Werner" # Another inline comment
dob = 1979-05-27T07:32:00-08:00 # Date of birth with comment
# End of file
"#,
            )
            .collect();

        assert_eq!(
            blocks,
            vec![
                Comment {
                    position_range: Position::new(2, 1)..Position::new(2, 22),
                    source_range: 1..22,
                    comment_text: "  This is a TOML file".to_string()
                },
                Comment {
                    position_range: Position::new(3, 24)..Position::new(3, 40),
                    source_range: 46..62,
                    comment_text: "  Inline comment".to_string()
                },
                Comment {
                    position_range: Position::new(5, 1)..Position::new(5, 18),
                    source_range: 71..88,
                    comment_text: "  Owner's details".to_string()
                },
                Comment {
                    position_range: Position::new(6, 29)..Position::new(6, 53),
                    source_range: 117..141,
                    comment_text: "  Another inline comment".to_string()
                },
                Comment {
                    position_range: Position::new(7, 33)..Position::new(7, 61),
                    source_range: 174..202,
                    comment_text: "  Date of birth with comment".to_string()
                },
                Comment {
                    position_range: Position::new(8, 1)..Position::new(8, 14),
                    source_range: 203..216,
                    comment_text: "  End of file".to_string()
                }
            ]
        );

        Ok(())
    }

    /// A symbol as `(path, value, the text of each of its ranges)`.
    type DerivedSymbol = (String, Option<String>, Vec<String>);

    /// The symbols `source` derives, in the order they were derived.
    fn derived_symbols(source: &str) -> anyhow::Result<Vec<DerivedSymbol>> {
        Ok(parser()?
            .parse_symbols(source)?
            .into_iter()
            .map(|symbol| {
                let texts = symbol
                    .def_byte_ranges
                    .iter()
                    .map(|range| source[range.clone()].to_string())
                    .collect();
                let value = symbol.value.map(|value| value.text);
                (symbol.path.to_string(), value, texts)
            })
            .collect())
    }

    /// The same symbols, written as string literals for readability.
    fn expected_symbols(symbols: &[(&str, Option<&str>, &[&str])]) -> Vec<DerivedSymbol> {
        symbols
            .iter()
            .map(|(path, value, texts)| {
                (
                    path.to_string(),
                    value.map(str::to_string),
                    texts.iter().map(|text| text.to_string()).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn document_with_every_toml_shape_parse_symbols_returns_every_symbol_in_order()
    -> anyhow::Result<()> {
        let source = r#"# A comment before any key
string = "a\tb"
integer = 1_000
float = 3.14
date = 1979-05-27
a.b = true
"quoted.key" = 1
inline = { x = 1, y.z = 2 }
array = [
  # A comment takes no index
  "first",
  [2, { k = "v" }],
]

[package]
name = "blockwatch"

[dependencies]
serde = "1"

[package.metadata.docs]
all-features = true

[[bin]]
name = "a"

[[test]]
name = "t"

[[bin]]
name = "b"

[[fruits]]
name = "apple"

[fruits.physical]
color = "red"

[[fruits.varieties]]
name = "red delicious"

[[fruits]]
name = "banana"

[[fruits.varieties]]
name = "plantain"
"#;
        let package = "[package]\nname = \"blockwatch\"\n";
        let docs = "[package.metadata.docs]\nall-features = true\n";
        let first_bin = "[[bin]]\nname = \"a\"\n";
        let second_bin = "[[bin]]\nname = \"b\"\n";
        let test = "[[test]]\nname = \"t\"\n";
        let apple = "[[fruits]]\nname = \"apple\"\n";
        let physical = "[fruits.physical]\ncolor = \"red\"\n";
        let red_delicious = "[[fruits.varieties]]\nname = \"red delicious\"\n";
        let banana = "[[fruits]]\nname = \"banana\"\n";
        let plantain = "[[fruits.varieties]]\nname = \"plantain\"\n";

        // <block affects="docs/symbols.md:toml-paths, src/skill.md:toml-paths">
        assert_eq!(
            derived_symbols(source)?,
            expected_symbols(&[
                ("/string", Some("a\tb"), &[r#"string = "a\tb""#]),
                ("/integer", Some("1_000"), &["integer = 1_000"]),
                ("/float", Some("3.14"), &["float = 3.14"]),
                ("/date", Some("1979-05-27"), &["date = 1979-05-27"]),
                ("/a", None, &["b = true\n"]),
                ("/a/b", Some("true"), &["b = true"]),
                ("/quoted.key", Some("1"), &[r#""quoted.key" = 1"#]),
                ("/inline", None, &["inline = { x = 1, y.z = 2 }"]),
                ("/inline/x", Some("1"), &["x = 1"]),
                ("/inline/y", None, &["z = 2"]),
                ("/inline/y/z", Some("2"), &["z = 2"]),
                (
                    "/array",
                    None,
                    &[
                        "array = [\n  # A comment takes no index\n  \"first\",\n  [2, { k = \"v\" }],\n]"
                    ],
                ),
                ("/array/0", Some("first"), &[r#""first""#]),
                ("/array/1", None, &[r#"[2, { k = "v" }]"#]),
                ("/array/1/0", Some("2"), &["2"]),
                ("/array/1/1", None, &[r#"{ k = "v" }"#]),
                ("/array/1/1/k", Some("v"), &[r#"k = "v""#]),
                ("/package", None, &[package, docs]),
                (
                    "/package/name",
                    Some("blockwatch"),
                    &[r#"name = "blockwatch""#]
                ),
                ("/dependencies", None, &["[dependencies]\nserde = \"1\"\n"]),
                ("/dependencies/serde", Some("1"), &[r#"serde = "1""#]),
                ("/package/metadata", None, &[docs]),
                ("/package/metadata/docs", None, &[docs]),
                (
                    "/package/metadata/docs/all-features",
                    Some("true"),
                    &["all-features = true"]
                ),
                ("/bin", None, &[first_bin, second_bin]),
                ("/bin/0", None, &[first_bin]),
                ("/bin/0/name", Some("a"), &[r#"name = "a""#]),
                ("/test", None, &[test]),
                ("/test/0", None, &[test]),
                ("/test/0/name", Some("t"), &[r#"name = "t""#]),
                ("/bin/1", None, &[second_bin]),
                ("/bin/1/name", Some("b"), &[r#"name = "b""#]),
                (
                    "/fruits",
                    None,
                    &[apple, physical, red_delicious, banana, plantain]
                ),
                ("/fruits/0", None, &[apple, physical, red_delicious]),
                ("/fruits/0/name", Some("apple"), &[r#"name = "apple""#]),
                ("/fruits/0/physical", None, &[physical]),
                (
                    "/fruits/0/physical/color",
                    Some("red"),
                    &[r#"color = "red""#]
                ),
                ("/fruits/0/varieties", None, &[red_delicious]),
                ("/fruits/0/varieties/0", None, &[red_delicious]),
                (
                    "/fruits/0/varieties/0/name",
                    Some("red delicious"),
                    &[r#"name = "red delicious""#]
                ),
                ("/fruits/1", None, &[banana, plantain]),
                ("/fruits/1/name", Some("banana"), &[r#"name = "banana""#]),
                ("/fruits/1/varieties", None, &[plantain]),
                ("/fruits/1/varieties/0", None, &[plantain]),
                (
                    "/fruits/1/varieties/0/name",
                    Some("plantain"),
                    &[r#"name = "plantain""#]
                ),
            ])
        );
        // </block>
        Ok(())
    }

    #[test]
    fn every_spelling_of_one_key_parse_symbols_derives_one_path() -> anyhow::Result<()> {
        let spellings = [
            "[a.b]\nc = 1\n",
            "a.b.c = 1\n",
            "a = { b = { c = 1 } }\n",
            "[a]\nb = { c = 1 }\n",
            "[a]\nb.c = 1\n",
            "[a.\"b\"]\nc = 1\n",
        ];

        for source in spellings {
            let symbols = derived_symbols(source)?;
            assert!(
                symbols
                    .iter()
                    .any(|(path, value, _)| path == "/a/b/c" && value.as_deref() == Some("1")),
                "{source:?} derives {symbols:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn quoted_key_with_a_dot_parse_symbols_keeps_it_one_segment() -> anyhow::Result<()> {
        let paths: Vec<String> = derived_symbols("[a]\n\"b.c\" = 1\n")?
            .into_iter()
            .map(|(path, _, _)| path)
            .collect();

        assert_eq!(paths, vec!["/a", "/a/b.c"]);
        Ok(())
    }

    #[test]
    fn dotted_table_with_a_sub_table_parse_symbols_keeps_its_sections_apart() -> anyhow::Result<()>
    {
        // `[other]` is written between the two places `fruit.apple` is written, and belongs to
        // neither.
        let source = r#"[fruit]
apple.color = "red"

[other]
x = 1

[fruit.apple.texture]
smooth = true
"#;
        let texture = "[fruit.apple.texture]\nsmooth = true\n";

        let symbols = derived_symbols(source)?;

        let ranges_of = |wanted: &str| {
            symbols
                .iter()
                .find(|(path, _, _)| path == wanted)
                .map(|(_, _, texts)| texts.clone())
        };
        assert_eq!(
            ranges_of("/fruit"),
            Some(vec![
                "[fruit]\napple.color = \"red\"\n".to_string(),
                texture.to_string()
            ])
        );
        assert_eq!(
            ranges_of("/fruit/apple"),
            Some(vec!["color = \"red\"\n".to_string(), texture.to_string()])
        );
        Ok(())
    }

    #[test]
    fn string_kinds_parse_symbols_returns_decoded_values() -> anyhow::Result<()> {
        let source = r#"basic = "tab\there \u00e9"
literal = 'C:\path'
multiline = """
first \
   second"""
multiline_literal = '''
raw \n'''
"#;

        let values: Vec<(String, Option<String>)> = derived_symbols(source)?
            .into_iter()
            .map(|(path, value, _)| (path, value))
            .collect();

        assert_eq!(
            values,
            vec![
                ("/basic".to_string(), Some("tab\there é".to_string())),
                ("/literal".to_string(), Some(r"C:\path".to_string())),
                ("/multiline".to_string(), Some("first second".to_string())),
                (
                    "/multiline_literal".to_string(),
                    Some(r"raw \n".to_string())
                ),
            ]
        );
        Ok(())
    }

    #[test]
    fn scalar_values_parse_symbols_returns_correct_byte_range() -> anyhow::Result<()> {
        let source = r#"string = "a\tb"
integer = 1_000
"#;

        let byte_range: Vec<(String, Option<&str>)> = parser()?
            .parse_symbols(source)?
            .into_iter()
            .map(|symbol| {
                let written = symbol.value.map(|value| &source[value.byte_range]);
                (symbol.path.to_string(), written)
            })
            .collect();

        assert_eq!(
            byte_range,
            vec![
                ("/string".to_string(), Some(r#""a\tb""#)),
                ("/integer".to_string(), Some("1_000")),
            ]
        );
        Ok(())
    }

    #[test]
    fn duplicate_key_parse_symbols_returns_error_with_its_position() -> anyhow::Result<()> {
        let err = parser()?.parse_symbols("v = 1\nv = 2\n").unwrap_err();

        assert_eq!(
            err.to_string(),
            "file is not valid TOML at line 2, column 1: duplicate key"
        );
        Ok(())
    }
}
