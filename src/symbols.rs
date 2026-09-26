use crate::Position;
use crate::character_column_at;
use crate::symbol_path::SymbolPath;
use anyhow::{Context, bail, ensure};
use std::collections::HashMap;
use std::ops::Range;
use tree_sitter::StreamingIterator;

/// An addressable symbol definition derived from a syntax tree using a language query.
///
/// Contains the derived rooted [`SymbolPath`], the byte range of the definition node, and the
/// unquoted scalar value if the symbol represents a scalar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol {
    /// The symbol path of this symbol (e.g. `/dependencies/inngest`).
    pub path: SymbolPath,
    /// Byte range of the definition node (`@def` or `@item`) in the source.
    pub def_byte_range: Range<usize>,
    /// The unquoted scalar value, or `None` if the symbol represents a container or composite node.
    pub value: Option<String>,
}

impl Symbol {
    fn new(node: &tree_sitter::Node, path: &[String], value: Option<String>) -> Self {
        // A path is empty only when the query gives a `@def` no `@name`, and `from_segments`
        // panics on that. An `@item` always gets its index, so it cannot be the cause.
        Self {
            path: SymbolPath::from_segments(path.to_vec()),
            def_byte_range: node.byte_range(),
            value,
        }
    }

    /// The 1-based line and character range of the definition in `source`, which must be the
    /// text the symbol was derived from.
    ///
    /// Takes time proportional to the definition's offset in `source`, so it suits the symbol a
    /// reference resolves to rather than every symbol a file derives.
    ///
    /// # Panics
    /// Panics if the definition's byte range does not fit `source`.
    pub fn position_range(&self, source: &str) -> Range<Position> {
        position_at(source, self.def_byte_range.start)..position_at(source, self.def_byte_range.end)
    }

    /// The symbol's value, or the text of its definition in `source` when it has none, as an
    /// object does. `source` must be the text the symbol was derived from.
    ///
    /// # Panics
    /// Panics if the definition's byte range does not fit `source`.
    pub fn value_or_definition<'s>(&'s self, source: &'s str) -> &'s str {
        match &self.value {
            Some(value) => value,
            None => &source[self.def_byte_range.clone()],
        }
    }
}

/// Why a symbol path did not resolve to exactly one symbol.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ResolveError<'a> {
    /// No symbol has the path. `hints` holds the paths that resemble it, the most alike first.
    NotFound { hints: Vec<&'a SymbolPath> },
    /// Several symbols have the path. `candidates` holds every one of them, in document order.
    Ambiguous { candidates: Vec<&'a Symbol> },
}

/// The most hints a path that does not resolve gets.
const MAX_HINTS: usize = 3;

/// How alike a path has to be to the one that does not resolve to be a hint, as a normalized edit
/// distance from 0 (nothing in common) to 1 (identical).
const MIN_HINT_SIMILARITY: f64 = 0.6;

/// Finds a matching `Symbol` in `symbols` for the given `path`.
///
/// When no symbol has `path`, the hints are the paths that resemble it: those at least 60% alike
/// by edit distance, and those that end in the same segment.
///
/// # Errors
/// Returns [`ResolveError::NotFound`] when no symbol matches `path`, and
/// [`ResolveError::Ambiguous`] when several do.
pub(crate) fn resolve<'a>(
    symbols: &'a [Symbol],
    path: &SymbolPath,
) -> Result<&'a Symbol, ResolveError<'a>> {
    let candidates: Vec<&Symbol> = symbols
        .iter()
        .filter(|symbol| symbol.path == *path)
        .collect();
    match candidates.len() {
        0 => Err(ResolveError::NotFound {
            hints: hints(symbols, path),
        }),
        1 => Ok(candidates[0]),
        _ => Err(ResolveError::Ambiguous { candidates }),
    }
}

/// The paths in `symbols` that resemble `path`.
fn hints<'a>(symbols: &'a [Symbol], path: &SymbolPath) -> Vec<&'a SymbolPath> {
    // The paths are compared as written, escapes included, so that a `/` left unescaped in a key
    // reads as a small difference.
    let written = path.to_string();
    let mut alike: Vec<(f64, &SymbolPath)> = symbols
        .iter()
        .map(|symbol| {
            let similarity = strsim::normalized_levenshtein(&written, &symbol.path.to_string());
            (similarity, &symbol.path)
        })
        .filter(|(similarity, candidate)| {
            *similarity >= MIN_HINT_SIMILARITY
                || candidate.segments().last() == path.segments().last()
        })
        .collect();
    alike.sort_by(|(a, _), (b, _)| b.total_cmp(a));
    alike
        .into_iter()
        .take(MAX_HINTS)
        .map(|(_, candidate)| candidate)
        .collect()
}

/// Derives the addressable symbols of one language's source files.
pub(crate) trait SymbolsParser: Send + Sync {
    /// Derives all the addressable symbols in `source`, in document order. Two symbols can share a
    /// path, when the source defines the same name twice.
    ///
    /// # Errors
    /// Returns an error if the symbols of `source` cannot be derived reliably, such as when it has
    /// a syntax error. The error mentions the position of the problem when there is one.
    fn parse(&mut self, source: &str) -> anyhow::Result<Vec<Symbol>>;
}

/// The semantic role a query capture plays in deriving a symbol path or value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CaptureRole {
    /// An addressable definition node contributing a region.
    Def,
    /// An addressable sequence member contributing an index.
    Item,
    /// A non-addressable scope contributing path segments to descendants.
    Container,
    /// A name fragment contributing one or more path segments.
    Name,
    /// A scalar node contributing a value.
    Value,
    /// A document boundary in multi-document files.
    Document,
}

/// Turns a captured `@name` or `@value` node into the text it denotes in `source`, following the
/// language's own quoting and escaping rules.
///
/// # Errors
/// Returns an error if the node's text is not a valid spelling in the language.
pub(crate) type NodeDecoder = fn(&tree_sitter::Node, &str) -> anyhow::Result<String>;

/// A [`SymbolsParser`] driven by a tree-sitter query that declares what is addressable in one
/// language.
///
/// A symbol's path is made of the names along its node's ancestors in the syntax tree, so it suits
/// a language whose structure is its syntax tree.
pub(crate) struct QuerySymbolsParser {
    parser: tree_sitter::Parser,
    query: tree_sitter::Query,
    capture_roles: Vec<CaptureRole>,
    node_decoder: NodeDecoder,
}

impl QuerySymbolsParser {
    /// Compiles `query_source` for `language` and maps its capture names to semantic roles. Each
    /// `@name` and `@value` a match captures is turned into text by `node_decoder`.
    ///
    /// # Errors
    /// Returns an error if the query syntax is invalid or uses unrecognized capture names.
    pub(crate) fn new(
        language: &tree_sitter::Language,
        query_source: &str,
        node_decoder: NodeDecoder,
    ) -> anyhow::Result<Self> {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(language)
            .context("failed to set tree-sitter language")?;
        let query = tree_sitter::Query::new(language, query_source)
            .context("failed to compile tree-sitter symbol query")?;
        let capture_roles = query
            .capture_names()
            .iter()
            .map(|&name| match name {
                "def" => Ok(CaptureRole::Def),
                "item" => Ok(CaptureRole::Item),
                "container" => Ok(CaptureRole::Container),
                "name" => Ok(CaptureRole::Name),
                "value" => Ok(CaptureRole::Value),
                "document" => Ok(CaptureRole::Document),
                other => bail!("unrecognized query capture '@{other}'"),
            })
            .collect::<anyhow::Result<Vec<_>>>()?;

        Ok(Self {
            parser,
            query,
            capture_roles,
            node_decoder,
        })
    }

    /// Runs the query and returns what each match contributes, keyed by the id of the node the
    /// match attaches it to. A match with no `@def`, `@item` or `@container` contributes nothing.
    ///
    /// # Errors
    /// Returns an error if the source holds more than one document, or if the decoder rejects a
    /// captured name or value.
    fn collect_contributions(
        &self,
        tree: &tree_sitter::Tree,
        source: &str,
    ) -> anyhow::Result<HashMap<usize, Contribution>> {
        let mut cursor = tree_sitter::QueryCursor::new();
        let mut matches = cursor.matches(&self.query, tree.root_node(), source.as_bytes());

        let mut contributions = HashMap::new();
        let mut document_count = 0;
        while let Some(query_match) = matches.next() {
            let match_captures = MatchCaptures::new(query_match.captures(), &self.capture_roles);
            document_count += match_captures.documents;
            ensure!(
                document_count <= 1,
                "file holds more than one document which is not supported"
            );
            if let Some((node, contribution)) =
                match_captures.try_into_contribution(self.node_decoder, source)?
            {
                contributions.insert(node.id(), contribution);
            }
        }

        Ok(contributions)
    }
}

impl SymbolsParser for QuerySymbolsParser {
    /// Parses `source` and derives all its addressable symbols, in document order. Two symbols can
    /// share a path, when the source defines the same name twice.
    ///
    /// # Errors
    /// Returns an error if:
    /// - The source has a syntax error. The error mentions the position of the first one.
    /// - The source contains more than one document.
    /// - The decoder rejects a captured name or value.
    ///
    /// # Panics
    /// Panics if the query gives a `@def` no `@name`, which leaves its node without a path. Each
    /// language's own tests cover the query it ships.
    fn parse(&mut self, source: &str) -> anyhow::Result<Vec<Symbol>> {
        let tree = self
            .parser
            .parse(source, None)
            .context("failed to parse syntax tree")?;
        // tree-sitter recovers from a syntax error by guessing what the source meant, and a path
        // resolved through a guess can land on a wrong value. A missing value, for one, is
        // recovered as an empty one.
        if let Some(error) = first_syntax_error(&tree) {
            let position = position_at(source, error.start_byte());
            bail!(
                "file has a syntax error at line {}, column {}, and a path into it could resolve to the wrong value",
                position.line,
                position.character,
            );
        }
        let contributions = self.collect_contributions(&tree, source)?;
        Ok(symbols_from_contributions(&tree, &contributions))
    }
}

/// The first syntax error in `tree`, in document order, or `None` if the source parsed cleanly.
///
/// The node is the innermost one that error recovery produced there: a span it skipped, or a token
/// it assumed was missing.
fn first_syntax_error(tree: &tree_sitter::Tree) -> Option<tree_sitter::Node<'_>> {
    let mut node = tree.root_node();
    if !node.has_error() {
        return None;
    }
    // `has_error` also holds for every ancestor of an error, so descending into the first child
    // that has one leads to the first error. A token that error recovery skipped is an error node
    // without `has_error`, so it is matched on its own.
    let mut cursor = tree.walk();
    while let Some(child) = node
        .children(&mut cursor)
        .find(|child| child.is_error() || child.has_error())
    {
        node = child;
    }
    Some(node)
}

/// The captures of one query match, grouped by role.
#[derive(Default)]
struct MatchCaptures<'tree> {
    def: Option<tree_sitter::Node<'tree>>,
    item: Option<tree_sitter::Node<'tree>>,
    container: Option<tree_sitter::Node<'tree>>,
    /// The `@name` captures, in source order.
    names: Vec<tree_sitter::Node<'tree>>,
    value: Option<tree_sitter::Node<'tree>>,
    /// How many `@document` captures the match holds.
    documents: usize,
}

impl<'tree> MatchCaptures<'tree> {
    /// Groups the captures of one query match by role. `captures` are that match's captures, and
    /// `capture_roles` holds the role of every capture its query declares, indexed by the capture
    /// index each [`tree_sitter::QueryCapture`] carries.
    ///
    /// The `@name` captures are kept in source order and the `@document` captures are counted. For
    /// every other role, the last capture of that role in the match wins.
    ///
    /// # Panics
    /// Panics if a capture index falls outside `capture_roles`, which may happen when the captures
    /// and the roles come from different queries.
    fn new(
        captures: &[tree_sitter::QueryCapture<'tree>],
        capture_roles: &[CaptureRole],
    ) -> MatchCaptures<'tree> {
        let mut by_role = Self::default();
        for capture in captures {
            match capture_roles[capture.index as usize] {
                CaptureRole::Def => by_role.def = Some(capture.node),
                CaptureRole::Item => by_role.item = Some(capture.node),
                CaptureRole::Container => by_role.container = Some(capture.node),
                CaptureRole::Name => by_role.names.push(capture.node),
                CaptureRole::Value => by_role.value = Some(capture.node),
                CaptureRole::Document => by_role.documents += 1,
            }
        }
        by_role
    }

    /// The node the match attaches its contribution to, and the contribution itself. Returns
    /// `None` when the match has no `@def`, `@item` or `@container`.
    ///
    /// # Errors
    /// Returns an error if `node_decoder` rejects a captured name or value.
    fn try_into_contribution(
        self,
        node_decoder: NodeDecoder,
        source: &str,
    ) -> anyhow::Result<Option<(tree_sitter::Node<'tree>, Contribution)>> {
        // Decoding comes first because it costs nothing when the match contributes nothing: the
        // query rules give such a match no `@name` and no `@value` either.
        let names: Vec<String> = self
            .names
            .iter()
            .map(|name| node_decoder(name, source))
            .collect::<anyhow::Result<_>>()?;
        let value = self
            .value
            .map(|value| node_decoder(&value, source))
            .transpose()?;
        match (self.def, self.item, self.container) {
            (Some(node), _, _) => Ok(Some((node, Contribution::Def { names, value }))),
            (None, Some(node), _) => Ok(Some((node, Contribution::Item { names, value }))),
            // A container is not addressable, so a `@value` captured next to one has nothing to
            // belong to.
            (None, None, Some(node)) => Ok(Some((node, Contribution::Container { names }))),
            (None, None, None) => Ok(None),
        }
    }
}

/// What one query match contributes to the path of the node it is attached to, and to the paths of
/// that node's descendants.
enum Contribution {
    /// `@def`: addressable by its names.
    Def {
        names: Vec<String>,
        value: Option<String>,
    },
    /// `@item`: addressable by its names followed by its index among its parent's items.
    Item {
        names: Vec<String>,
        value: Option<String>,
    },
    /// `@container`: not addressable, but its names prefix the paths of its descendants.
    Container { names: Vec<String> },
}

impl Contribution {
    /// The decoded text of the match's `@name` captures, one segment each, in source order.
    fn names(&self) -> &[String] {
        match self {
            Self::Def { names, .. } | Self::Item { names, .. } | Self::Container { names } => names,
        }
    }
}

/// Builds a [`Symbol`] for every `@def` and `@item` contribution in `tree`, in document order.
fn symbols_from_contributions(
    tree: &tree_sitter::Tree,
    contributions: &HashMap<usize, Contribution>,
) -> Vec<Symbol> {
    let mut walk = Walk::new();
    let mut cursor = tree.walk();
    loop {
        let node = cursor.node();
        walk.enter(&node, contributions.get(&node.id()));

        if cursor.goto_first_child() {
            continue;
        }
        // Leave nodes until one has a next sibling to enter.
        loop {
            walk.leave();
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return walk.symbols;
            }
        }
    }
}

/// What a pre-order walk of a tree has derived so far, and the state it needs to carry on.
struct Walk {
    /// The symbols of the nodes the walk has entered, in the order it entered them.
    symbols: Vec<Symbol>,
    /// The segments contributed by the nodes between the root and the node last entered.
    path: Vec<String>,
    /// One frame per node on that route, plus a bottom frame that counts the root's own index.
    frames: Vec<Frame>,
}

impl Walk {
    fn new() -> Self {
        Self {
            symbols: Vec::new(),
            path: Vec::new(),
            frames: vec![Frame {
                path_len: 0,
                items: 0,
            }],
        }
    }

    /// Enters `node`, adding what it contributes to the path and deriving its symbol, if it has
    /// one. `contribution` is what the query attached to `node`, if anything.
    fn enter(&mut self, node: &tree_sitter::Node, contribution: Option<&Contribution>) {
        let path_len = self.path.len();
        if let Some(contribution) = contribution {
            self.path.extend(contribution.names().iter().cloned());
            self.add_symbol_from_contribution(node, contribution);
        }
        self.frames.push(Frame { path_len, items: 0 });
    }

    /// Leaves the node entered last, cutting the path back to what its ancestors contributed.
    fn leave(&mut self) {
        let frame = self.frames.pop().expect("every entered node has a frame");
        self.path.truncate(frame.path_len);
    }

    /// Derives the symbol `contribution` makes `node` addressable by, if it makes it addressable
    /// at all. An `@item` takes the next index among its parent's items as its last segment.
    fn add_symbol_from_contribution(
        &mut self,
        node: &tree_sitter::Node,
        contribution: &Contribution,
    ) {
        match contribution {
            Contribution::Container { .. } => {}
            Contribution::Def { value, .. } => {
                self.symbols
                    .push(Symbol::new(node, &self.path, value.clone()));
            }
            Contribution::Item { value, .. } => {
                let parent = self
                    .frames
                    .last_mut()
                    .expect("the bottom frame is never popped");
                self.path.push(parent.items.to_string());
                parent.items += 1;
                self.symbols
                    .push(Symbol::new(node, &self.path, value.clone()));
            }
        }
    }
}

/// The walk state of one node on the route from the root to the node last entered.
struct Frame {
    /// The length of the path before the node added its own segments.
    path_len: usize,
    /// How many of the node's children have been captured as `@item` so far.
    items: usize,
}

/// The 1-based line and character position of `byte_offset` in `source`.
fn position_at(source: &str, byte_offset: usize) -> Position {
    let line = source[..byte_offset].matches('\n').count() + 1;
    Position::new(line, character_column_at(source, byte_offset))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A parser for JSON that takes a captured node's text as it stands. Unescaping is each
    /// language's own concern, so the queries here capture the `string_content` inside a string
    /// to get its text without the quotes.
    fn json_symbols_parser(query_source: &str) -> anyhow::Result<QuerySymbolsParser> {
        QuerySymbolsParser::new(
            &tree_sitter_json::LANGUAGE.into(),
            query_source,
            |node, source| Ok(source[node.byte_range()].to_string()),
        )
    }

    /// A parser for the tests whose subject is not the query: `parse` refuses a source with a
    /// syntax error before it runs the query at all.
    fn parser_with_any_query() -> QuerySymbolsParser {
        json_symbols_parser("(pair key: (string) @name) @def").expect("the query compiles")
    }

    #[test]
    fn sequence_with_uncaptured_members_parse_indexes_only_the_captured_ones() -> anyhow::Result<()>
    {
        // The query captures only the strings, so the numbers between them are members the index
        // has to skip.
        let item_query = r#"(array (string (string_content) @value) @item)"#;
        let source = r#"[1, "first", 2, "second"]"#;

        let symbols = json_symbols_parser(item_query)?.parse(source)?;

        let derived: Vec<(String, Option<String>)> = symbols
            .into_iter()
            .map(|symbol| (symbol.path.to_string(), symbol.value))
            .collect();
        assert_eq!(
            derived,
            vec![
                ("/0".to_string(), Some("first".to_string())),
                ("/1".to_string(), Some("second".to_string())),
            ]
        );
        Ok(())
    }

    #[test]
    fn container_query_parse_prepends_container_segments() -> anyhow::Result<()> {
        let source = r#"{
            "outer": {
                "inner": 42
            }
        }"#;
        let container_query = r#"
            (pair key: (string (string_content) @name) value: (object)) @container
            (pair key: (string (string_content) @name) value: (number) @value) @def
        "#;
        let symbols = json_symbols_parser(container_query)?.parse(source)?;
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].path.to_string(), "/outer/inner");
        assert_eq!(symbols[0].value, Some("42".to_string()));
        Ok(())
    }

    #[test]
    fn language_decoder_parse_decodes_names_and_values_with_it() -> anyhow::Result<()> {
        let mut parser = QuerySymbolsParser::new(
            &tree_sitter_json::LANGUAGE.into(),
            r#"(pair key: (string) @name value: (string) @value) @def"#,
            |node, source| Ok(source[node.byte_range()].to_uppercase()),
        )?;

        let symbols = parser.parse(r#"{"k": "v"}"#)?;

        assert_eq!(
            symbols,
            vec![Symbol {
                path: SymbolPath::from_segments(vec![r#""K""#.to_string()]),
                def_byte_range: 1..9,
                value: Some(r#""V""#.to_string()),
            }]
        );
        Ok(())
    }

    #[test]
    fn multibyte_source_parse_returns_byte_ranges_and_character_columns() -> anyhow::Result<()> {
        let range_query = r#"
            (pair key: (string (string_content) @name) value: (array)) @def
            (array (number) @value @item)
        "#;
        let source = r#"{
  "é": [1, 22]
}"#;

        let symbols = json_symbols_parser(range_query)?.parse(source)?;

        let ranges: Vec<(String, Range<usize>, Range<Position>)> = symbols
            .iter()
            .map(|symbol| {
                (
                    symbol.path.to_string(),
                    symbol.def_byte_range.clone(),
                    symbol.position_range(source),
                )
            })
            .collect();
        assert_eq!(
            ranges,
            vec![
                (
                    "/%C3%A9".to_string(),
                    4..17,
                    Position::new(2, 3)..Position::new(2, 15)
                ),
                (
                    "/%C3%A9/0".to_string(),
                    11..12,
                    Position::new(2, 9)..Position::new(2, 10)
                ),
                (
                    "/%C3%A9/1".to_string(),
                    14..16,
                    Position::new(2, 12)..Position::new(2, 14)
                ),
            ]
        );
        Ok(())
    }

    #[test]
    fn missing_token_parse_returns_syntax_error_where_it_belongs() {
        // The value of "b" is missing, so error recovery assumes one right after the colon.
        let err = parser_with_any_query()
            .parse(r#"{"a": 1, "b": , "c": 3}"#)
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("syntax error at line 1, column 14,"),
            "unexpected error message: {err}"
        );
    }

    #[test]
    fn unexpected_token_parse_returns_syntax_error_at_the_token() {
        // The value of "a" lacks its closing quote. It ends at the quote that should open the next
        // key, which leaves the `b` after it unexpected.
        let err = parser_with_any_query()
            .parse(r#"{"a": "1, "b": "2"}"#)
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("syntax error at line 1, column 12,"),
            "unexpected error message: {err}"
        );
    }

    #[test]
    fn unparsable_span_parse_returns_syntax_error_at_its_start() {
        // A trailing comma, which JSONC allows and JSON does not, is left over as a span that does
        // not parse.
        let err = parser_with_any_query().parse(r#"{"a": 1,}"#).unwrap_err();
        assert!(
            err.to_string()
                .contains("syntax error at line 1, column 8,"),
            "unexpected error message: {err}"
        );
    }

    #[test]
    fn source_with_several_syntax_errors_parse_reports_the_first() {
        let source = r#"{
  "a": 1 "b": 2,
  "c": ,
}"#;
        let err = parser_with_any_query().parse(source).unwrap_err();
        assert!(
            err.to_string()
                .contains("syntax error at line 2, column 3,"),
            "unexpected error message: {err}"
        );
    }

    #[test]
    fn multi_document_file_parse_returns_error() {
        // Query capturing multiple document nodes on the AST
        let doc_query = r#"
            (pair) @document
            (object) @document
        "#;
        let mut parser = json_symbols_parser(doc_query).unwrap();

        let err = parser.parse(r#"{ "a": 1 }"#).unwrap_err();
        assert!(
            err.to_string()
                .contains("file holds more than one document"),
            "unexpected error message: {err}"
        );
    }

    #[test]
    fn single_document_file_parse_succeeds() -> anyhow::Result<()> {
        let symbols = json_symbols_parser("(document) @document")?.parse(r#"{ "a": 1 }"#)?;
        assert!(symbols.is_empty());
        Ok(())
    }

    #[test]
    fn unknown_capture_name_new_returns_error() {
        let err = json_symbols_parser("(pair) @unknown_role")
            .err()
            .expect("an unknown capture name is rejected");
        assert!(
            err.to_string()
                .contains("unrecognized query capture '@unknown_role'"),
            "unexpected error message: {err}"
        );
    }

    #[test]
    fn malformed_query_syntax_new_returns_error() {
        let err = json_symbols_parser("invalid (((( syntax")
            .err()
            .expect("a malformed query is rejected");
        assert!(
            err.to_string()
                .contains("failed to compile tree-sitter symbol query"),
            "unexpected error message: {err}"
        );
    }
}

#[cfg(test)]
mod resolve_tests {
    use super::*;

    fn path(path: &str) -> SymbolPath {
        SymbolPath::parse(path).expect("the path is valid")
    }

    /// A symbol at `path` whose definition starts at `start`, which tells apart symbols that
    /// share a path.
    fn symbol(path_text: &str, start: usize) -> Symbol {
        Symbol {
            path: path(path_text),
            def_byte_range: start..start + 1,
            value: None,
        }
    }

    #[test]
    fn path_of_one_symbol_returns_that_symbol() {
        let symbols = [symbol("/a/b", 0), symbol("/b", 1)];

        assert_eq!(resolve(&symbols, &path("/b")), Ok(&symbols[1]));
    }

    #[test]
    fn path_of_several_symbols_returns_every_one_as_ambiguous() {
        let symbols = [symbol("/v", 0), symbol("/w", 1), symbol("/v", 2)];

        assert_eq!(
            resolve(&symbols, &path("/v")),
            Err(ResolveError::Ambiguous {
                candidates: vec![&symbols[0], &symbols[2]]
            })
        );
    }

    #[test]
    fn misspelled_path_returns_the_alike_paths_as_hints() {
        // `/description` is 50% alike, below the floor.
        let symbols = [
            symbol("/description", 0),
            symbol("/version", 1),
            symbol("/name", 2),
        ];

        assert_eq!(
            resolve(&symbols, &path("/verison")),
            Err(ResolveError::NotFound {
                hints: vec![&symbols[1].path]
            })
        );
    }

    #[test]
    fn path_with_an_unescaped_slash_returns_the_escaped_path_as_a_hint() {
        let symbols = [symbol("/dependencies/@types~1node", 0), symbol("/name", 1)];

        assert_eq!(
            resolve(&symbols, &path("/dependencies/@types/node")),
            Err(ResolveError::NotFound {
                hints: vec![&symbols[0].path]
            })
        );
    }

    #[test]
    fn path_moved_deeper_returns_it_as_a_hint() {
        // Both candidates are well below the floor, and only the first ends in the same segment.
        let symbols = [
            symbol("/network/services/RetryConfig", 0),
            symbol("/network/services/timeout", 1),
        ];

        assert_eq!(
            resolve(&symbols, &path("/RetryConfig")),
            Err(ResolveError::NotFound {
                hints: vec![&symbols[0].path]
            })
        );
    }

    #[test]
    fn many_alike_paths_returns_the_three_most_alike_first() {
        // 62%, 73%, 89% and 80% alike.
        let symbols = [
            symbol("/time", 0),
            symbol("/timeout_ms", 1),
            symbol("/timeouts", 2),
            symbol("/timeout_s", 3),
        ];

        assert_eq!(
            resolve(&symbols, &path("/timeout")),
            Err(ResolveError::NotFound {
                hints: vec![&symbols[2].path, &symbols[3].path, &symbols[1].path]
            })
        );
    }

    #[test]
    fn path_exactly_at_the_similarity_floor_returns_it_as_a_hint() {
        // Two of five characters differ in the first, which is 60% alike, and three in the second.
        let symbols = [symbol("/abXY", 0), symbol("/aXYZ", 1)];

        assert_eq!(
            resolve(&symbols, &path("/abcd")),
            Err(ResolveError::NotFound {
                hints: vec![&symbols[0].path]
            })
        );
    }
}
