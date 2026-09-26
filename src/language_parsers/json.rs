use crate::language_parsers::{LanguageParser, LanguageParserImpl, c_style_comments_parser};
use crate::symbols::QuerySymbolsParser;
use anyhow::Context;

/// Returns a [`LanguageParser`] for JSON.
pub(super) fn parser() -> anyhow::Result<impl LanguageParser> {
    let language = tree_sitter_json::LANGUAGE.into();
    let comments_parser = c_style_comments_parser(&language, "comment");
    let symbols_parser = QuerySymbolsParser::new(&language, include_str!("json.scm"), decode_node)?;
    Ok(LanguageParserImpl::new(comments_parser).with_symbols(symbols_parser))
}

/// The text a captured JSON node denotes: a string literal unquoted and unescaped, and any other
/// node its source text.
///
/// # Errors
/// Returns an error if a string literal is not valid JSON, e.g. because it holds a raw tab.
fn decode_node(node: &tree_sitter::Node, source: &str) -> anyhow::Result<String> {
    let raw = &source[node.byte_range()];
    match node.kind() {
        "string" => serde_json::from_str::<String>(raw)
            .with_context(|| format!("failed to unescape JSON string: {raw}")),
        _ => Ok(raw.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks::Block;
    use std::collections::HashMap;

    /// The symbols `source` derives, as `(path, value)` pairs in the order they were derived.
    fn derived_symbols(source: &str) -> anyhow::Result<Vec<(String, Option<String>)>> {
        Ok(parser()?
            .parse_symbols(source)?
            .into_iter()
            .map(|symbol| (symbol.path.to_string(), symbol.value))
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
