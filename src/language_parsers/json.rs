use crate::language_parsers::{
    LanguageParser, LanguageParserImpl, SLASH_MARKERS, c_style_comments_parser, members, node_text,
    tree_sitter_parser,
};
use crate::symbol_path::SymbolPath;
use crate::symbols::{ScalarValue, Symbol, Symbols, SymbolsParser, ensure_no_syntax_error};
use anyhow::{Context, bail};
use tree_sitter::Node;

/// Returns a [`LanguageParser`] for JSON.
pub(super) fn parser() -> anyhow::Result<impl LanguageParser> {
    let comments_parser = c_style_comments_parser(
        &tree_sitter_json::LANGUAGE.into(),
        &["comment"],
        SLASH_MARKERS,
    )?;
    Ok(LanguageParserImpl::new(comments_parser).with_symbols(JsonSymbolsParser::new()?))
}

/// Finds the symbols of a JSON file by walking its syntax tree.
///
/// Every key and every array item is a symbol. A string is decoded, and any other scalar is kept
/// as written. An object or an array has no value.
struct JsonSymbolsParser {
    parser: tree_sitter::Parser,
}

impl JsonSymbolsParser {
    /// # Errors
    /// Returns an error if the JSON grammar cannot be loaded.
    fn new() -> anyhow::Result<Self> {
        let parser = tree_sitter_parser(&tree_sitter_json::LANGUAGE.into())?;
        Ok(Self { parser })
    }
}

impl SymbolsParser for JsonSymbolsParser {
    /// # Errors
    /// Returns an error if the file has a syntax error, or a string that is not valid JSON.
    fn parse(&mut self, source: &str) -> anyhow::Result<Symbols> {
        let tree = self
            .parser
            .parse(source, None)
            .context("failed to parse syntax tree")?;
        ensure_no_syntax_error(&tree, source)?;
        Ok(Walk::symbols_in(source, tree.root_node())?.into())
    }
}

/// A member of an object or an array, whose symbol the walk still has to add.
enum Member<'tree> {
    /// A pair of an object, such as `"a": 1`.
    Pair(Node<'tree>),
    /// An item of an array, with its index.
    Item(Node<'tree>, usize),
}

/// The state of a walk over a JSON document.
struct Walk<'s, 'tree> {
    source: &'s str,
    /// The path to the last member walked. Before each member, the walk cuts it back to the path
    /// to the value that holds the member.
    path: Vec<String>,
    symbols: Vec<Symbol>,
    /// The members that still need symbols, the next one last. Each comes with the length of the
    /// path to the value that holds it.
    pending: Vec<(usize, Member<'tree>)>,
}

impl<'s, 'tree> Walk<'s, 'tree> {
    /// The symbols of everything inside `document`, the root of the syntax tree of `source`, in
    /// document order.
    ///
    /// # Errors
    /// Returns an error if a string is not valid JSON. The error is about the first such string.
    fn symbols_in(source: &'s str, document: Node<'tree>) -> anyhow::Result<Vec<Symbol>> {
        let mut walk = Walk {
            source,
            path: Vec::new(),
            symbols: Vec::new(),
            pending: Vec::new(),
        };
        // The grammar reads several values one after the other, and a path can't say which one it
        // means, so their members all start at the root. The stack takes the last member first,
        // so the values go on it in reverse.
        for value in members(document).into_iter().rev() {
            walk.push_members(value);
        }
        // The walk keeps its own stack, as deeply nested values could overflow the call stack.
        while let Some((depth, member)) = walk.pending.pop() {
            // Members come off the stack in document order. So every member walked since the
            // value that holds `member` is inside that value, and the path still starts with the
            // path to that value.
            walk.path.truncate(depth);
            walk.add_member_symbol(member)?;
        }
        Ok(walk.symbols)
    }

    /// Puts the members of `value`, the value at the current path, on `pending`. A scalar has
    /// none.
    fn push_members(&mut self, value: Node<'tree>) {
        let depth = self.path.len();
        let first = self.pending.len();
        let members = members(value).into_iter();
        match value.kind() {
            "object" => {
                self.pending
                    .extend(members.map(|pair| (depth, Member::Pair(pair))));
            }
            "array" => {
                let items = members.enumerate();
                self.pending
                    .extend(items.map(|(index, item)| (depth, Member::Item(item, index))));
            }
            _ => {}
        }
        // The stack takes the last member first, so the members go on it in reverse.
        self.pending[first..].reverse();
    }

    /// Adds the symbol of `member`. The current path is the path to the value that holds it.
    /// The members of its value go to `pending`.
    ///
    /// # Errors
    /// Returns an error if its key or its value is a string that is not valid JSON.
    fn add_member_symbol(&mut self, member: Member<'tree>) -> anyhow::Result<()> {
        let (written, value) = match member {
            Member::Pair(pair) => {
                let key = pair
                    .child_by_field_name("key")
                    .context("unexpected JSON pair without a key")?;
                let value = pair
                    .child_by_field_name("value")
                    .context("unexpected JSON pair without a value")?;
                self.path.push(decode_node(&key, self.source)?);
                (pair, value)
            }
            Member::Item(item, index) => {
                self.path.push(index.to_string());
                (item, item)
            }
        };
        let scalar = match value.kind() {
            "object" | "array" => None,
            "string" | "number" | "true" | "false" | "null" => Some(ScalarValue {
                text: decode_node(&value, self.source)?,
                byte_range: value.byte_range(),
            }),
            other => bail!("unexpected {other} in a JSON value"),
        };
        self.symbols.push(Symbol {
            path: SymbolPath::from_segments(self.path.clone()),
            def_byte_ranges: vec![written.byte_range()],
            value: scalar,
        });
        self.push_members(value);
        Ok(())
    }
}

/// The text that `node`, a key or a scalar, stands for: a string without its quotes and with its
/// escapes decoded, and any other scalar as written.
///
/// # Errors
/// Returns an error if a string is not valid JSON, e.g. because it holds a raw tab.
fn decode_node(node: &Node, source: &str) -> anyhow::Result<String> {
    let raw = node_text(node, source);
    match node.kind() {
        "string" => serde_json::from_str::<String>(raw)
            .with_context(|| format!("failed to unescape JSON string: {raw}")),
        _ => Ok(raw.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Position;
    use crate::blocks::Block;
    use std::collections::HashMap;
    use std::ops::Range;

    /// The symbols `source` derives, as `(path, value)` pairs in the order they were derived.
    fn derived_symbols(source: &str) -> anyhow::Result<Vec<(String, Option<String>)>> {
        Ok(parser()?
            .parse_symbols(source)?
            .symbols
            .into_iter()
            .map(|symbol| {
                let value = symbol.value.map(|value| value.text);
                (symbol.path.to_string(), value)
            })
            .collect())
    }

    /// The same pairs, written as string literals for readability.
    fn expected_symbols(
        symbols: impl IntoIterator<Item = (&'static str, Option<&'static str>)>,
    ) -> Vec<(String, Option<String>)> {
        symbols
            .into_iter()
            .map(|(path, value)| (path.to_string(), value.map(str::to_string)))
            .collect()
    }

    #[test]
    fn source_with_block_tags_parse_blocks_returns_blocks() -> anyhow::Result<()> {
        let mut parser = parser()?;

        let blocks: Vec<Block> = parser
            .parse_blocks(
                r#"{
  // <block name="test">
  "key": "value"
  // </block>
}
"#,
            )
            .collect::<anyhow::Result<Vec<_>>>()?;

        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].attributes.get("name").unwrap(), "test");
        Ok(())
    }

    #[test]
    fn start_tag_wrapped_over_longer_markers_parse_blocks_returns_block() -> anyhow::Result<()> {
        let mut parser = parser()?;

        let blocks: Vec<Block> = parser
            .parse_blocks(
                r#"{
  //// <block name="test"
  //! keep-sorted="asc">
  "key": "value"
  // </block>
}
"#,
            )
            .collect::<anyhow::Result<Vec<_>>>()?;

        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].attributes.get("keep-sorted").unwrap(), "asc");
        Ok(())
    }

    #[test]
    fn document_with_every_json_shape_parse_symbols_returns_every_symbol_in_order()
    -> anyhow::Result<()> {
        let source = r#"{
            "string": "a\nb\t\"c\"",
            "number": 42,
            "true": true,
            "false": false,
            "null": null,
            "a/b~c": "escaped key",
            "object": {"nested": "value"},
            "scalars": [
                // a comment takes no index
                "first",
                "second"
            ],
            "containers": [{"key": "value"}, ["item"]]
        }"#;

        // <block affects="docs/symbols.md:json-paths, src/skill.md#/blockwatch/Symbols/JSON">
        assert_eq!(
            derived_symbols(source)?,
            expected_symbols([
                ("/string", Some("a\nb\t\"c\"")),
                ("/number", Some("42")),
                ("/true", Some("true")),
                ("/false", Some("false")),
                ("/null", Some("null")),
                ("/a~1b~0c", Some("escaped key")),
                ("/object", None),
                ("/object/nested", Some("value")),
                ("/scalars", None),
                ("/scalars/0", Some("first")),
                ("/scalars/1", Some("second")),
                ("/containers", None),
                ("/containers/0", None),
                ("/containers/0/key", Some("value")),
                ("/containers/1", None),
                ("/containers/1/0", Some("item")),
            ])
        );
        // </block>
        Ok(())
    }

    #[test]
    fn duplicate_keys_parse_symbols_returns_one_symbol_each() -> anyhow::Result<()> {
        assert_eq!(
            derived_symbols(r#"{"v": 1, "v": 2}"#)?,
            expected_symbols([("/v", Some("1")), ("/v", Some("2"))])
        );
        Ok(())
    }

    #[test]
    fn top_level_array_parse_symbols_returns_symbols_indexed_from_zero() -> anyhow::Result<()> {
        assert_eq!(
            derived_symbols(r#"["a", "b"]"#)?,
            expected_symbols([("/0", Some("a")), ("/1", Some("b"))])
        );
        Ok(())
    }

    #[test]
    fn empty_key_in_array_item_parse_symbols_returns_an_empty_last_segment() -> anyhow::Result<()> {
        assert_eq!(
            derived_symbols(r#"[{"": 1}]"#)?,
            expected_symbols([("/0", None), ("/0/", Some("1"))])
        );
        Ok(())
    }

    #[test]
    fn object_and_array_values_parse_symbols_returns_symbols_without_a_value() -> anyhow::Result<()>
    {
        assert_eq!(
            derived_symbols(r#"{"object": {}, "array": []}"#)?,
            expected_symbols([("/object", None), ("/array", None)])
        );
        Ok(())
    }

    #[test]
    fn scalar_values_parse_symbols_returns_correct_byte_range() -> anyhow::Result<()> {
        let source = r#"{"string": "a\nb", "number": 42, "array": ["item"]}"#;

        let byte_range: Vec<(String, Option<&str>)> = parser()?
            .parse_symbols(source)?
            .symbols
            .into_iter()
            .map(|symbol| {
                let written = symbol.value.map(|value| &source[value.byte_range]);
                (symbol.path.to_string(), written)
            })
            .collect();

        assert_eq!(
            byte_range,
            vec![
                ("/string".to_string(), Some(r#""a\nb""#)),
                ("/number".to_string(), Some("42")),
                ("/array".to_string(), None),
                ("/array/0".to_string(), Some(r#""item""#)),
            ]
        );
        Ok(())
    }

    #[test]
    #[allow(clippy::single_range_in_vec_init)]
    fn multibyte_source_parse_symbols_returns_byte_ranges_and_character_columns()
    -> anyhow::Result<()> {
        // A key covers its whole pair, and an item only itself.
        let source = r#"{
  "é": [1, 22]
}"#;

        type PathAndRanges = (String, Vec<Range<usize>>, Vec<Range<Position>>);
        let ranges: Vec<PathAndRanges> = parser()?
            .parse_symbols(source)?
            .symbols
            .iter()
            .map(|symbol| {
                (
                    symbol.path.to_string(),
                    symbol.def_byte_ranges.clone(),
                    symbol.position_ranges(source),
                )
            })
            .collect();

        assert_eq!(
            ranges,
            vec![
                (
                    "/é".to_string(),
                    vec![4..17],
                    vec![Position::new(2, 3)..Position::new(2, 15)]
                ),
                (
                    "/é/0".to_string(),
                    vec![11..12],
                    vec![Position::new(2, 9)..Position::new(2, 10)]
                ),
                (
                    "/é/1".to_string(),
                    vec![14..16],
                    vec![Position::new(2, 12)..Position::new(2, 14)]
                ),
            ]
        );
        Ok(())
    }

    #[test]
    fn several_top_level_values_parse_symbols_returns_their_members_in_order() -> anyhow::Result<()>
    {
        assert_eq!(
            derived_symbols(r#"{"a": 1} [2] {"a": 3}"#)?,
            expected_symbols([("/a", Some("1")), ("/0", Some("2")), ("/a", Some("3"))])
        );
        Ok(())
    }

    #[test]
    fn deeply_nested_arrays_parse_symbols_does_not_overflow_stack() -> anyhow::Result<()> {
        // Each level is an object and an array, so the paths grow with the square of the depth.
        // This depth is enough to overflow a recursive walk.
        let depth = 1_000;
        let source = format!("{}1{}", r#"{"a": ["#.repeat(depth), "]}".repeat(depth));

        let symbols = parser()?.parse_symbols(&source)?;

        assert_eq!(symbols.symbols.len(), 2 * depth);
        Ok(())
    }

    #[test]
    fn top_level_scalar_parse_symbols_returns_no_symbols() -> anyhow::Result<()> {
        assert_eq!(derived_symbols("42")?, vec![]);
        Ok(())
    }

    #[test]
    fn keys_with_escape_sequences_parse_symbols_returns_decoded_paths() -> anyhow::Result<()> {
        let source = r#"{
            "hello\nworld": "newline",
            "escaped\"quote": "quote",
            "\u0041\u0042": "unicode",
            "": "empty"
        }"#;

        let symbol_map: HashMap<String, Option<String>> =
            derived_symbols(source)?.into_iter().collect();

        assert_eq!(
            symbol_map.get("/hello%0Aworld"),
            Some(&Some("newline".to_string()))
        );
        assert_eq!(
            symbol_map.get("/escaped%22quote"),
            Some(&Some("quote".to_string()))
        );
        assert_eq!(symbol_map.get("/AB"), Some(&Some("unicode".to_string())));
        assert_eq!(symbol_map.get("/"), Some(&Some("empty".to_string())));

        Ok(())
    }

    #[test]
    fn key_with_raw_tab_parse_symbols_returns_error() {
        let err = derived_symbols("{\"a\tb\": 1}").unwrap_err();

        assert!(
            err.to_string()
                .contains("failed to unescape JSON string: \"a\tb\""),
            "unexpected error message: {err}"
        );
    }

    #[test]
    fn value_with_raw_tab_parse_symbols_returns_error() {
        let err = derived_symbols("{\"key\": \"a\tb\"}").unwrap_err();

        assert!(
            err.to_string()
                .contains("failed to unescape JSON string: \"a\tb\""),
            "unexpected error message: {err}"
        );
    }
}
