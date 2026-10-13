use crate::Position;
use crate::symbol_path::{Segment, SymbolPath};
use anyhow::{Context, bail};
use std::borrow::Cow;
use std::collections::HashMap;
use std::ops::Range;
use tree_sitter::StreamingIterator;

/// An addressable element of a source file, such as a JSON key or an array item.
///
/// Contains the rooted [`SymbolPath`], the byte ranges where the symbol is written, and the
/// scalar value if the symbol represents a scalar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Symbol {
    /// The symbol path of this symbol (e.g. `/dependencies/inngest`).
    pub(crate) path: SymbolPath,
    /// The byte ranges of the source where the symbol is written, in document order and not
    /// overlapping. Never empty. A symbol written in several places has one range for each.
    pub(crate) def_byte_ranges: Vec<Range<usize>>,
    /// The scalar value, or `None` if the symbol represents a container or composite node.
    pub(crate) value: Option<ScalarValue>,
}

impl Symbol {
    fn new(node: &tree_sitter::Node, path: &[String], value: Option<ScalarValue>) -> Self {
        // A path is empty only when the query gives a `@def` no `@name`, and `from_segments`
        // panics on that. An `@item` always gets its index, so it cannot be the cause.
        Self {
            path: SymbolPath::from_segments(path.to_vec()),
            def_byte_ranges: vec![node.byte_range()],
            value,
        }
    }

    /// The 1-based line and character range of each of the symbol's byte ranges in `source`,
    /// which must be the text the symbol was derived from.
    ///
    /// Takes time proportional to the definition's offset in `source`, so it suits the symbol a
    /// reference resolves to rather than every symbol a file derives.
    ///
    /// # Panics
    /// Panics if a byte range does not fit `source`.
    pub(crate) fn position_ranges(&self, source: &str) -> Vec<Range<Position>> {
        self.def_byte_ranges
            .iter()
            .map(|range| {
                Position::from_byte_offset(source, range.start)
                    ..Position::from_byte_offset(source, range.end)
            })
            .collect()
    }

    /// The symbol's value, or else the text of its definition in `source`, as an object has. A
    /// symbol written in several places gives the text of each, one after another, in document
    /// order. `source` must be the text the symbol was derived from.
    ///
    /// # Panics
    /// Panics if a byte range does not fit `source`.
    pub(crate) fn value_or_definition<'s>(&'s self, source: &'s str) -> Cow<'s, str> {
        match (&self.value, self.def_byte_ranges.as_slice()) {
            (Some(value), _) => Cow::Borrowed(&value.text),
            (None, [range]) => Cow::Borrowed(&source[range.clone()]),
            (None, ranges) => {
                Cow::Owned(ranges.iter().map(|range| &source[range.clone()]).collect())
            }
        }
    }
}

/// The value of a [`Symbol`] that represents a scalar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ScalarValue {
    /// The text of the value. A string has no quotes and its escapes are decoded. Any other
    /// scalar is kept as written.
    pub(crate) text: String,
    /// The byte range of the source where the value is written, quotes included.
    /// It covers the whole literal as returned by the grammar.
    pub(crate) byte_range: Range<usize>,
}

/// Why a symbol path did not resolve to exactly one symbol.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ResolveError<'a> {
    /// No symbol has the path. `hints` holds the paths that resemble it, the most alike first.
    NotFound { hints: Vec<SymbolPath> },
    /// Several symbols have the path. `candidates` holds every one of them, in document order.
    /// When a filter in the path matches several children, they are the keys it matched.
    Ambiguous { candidates: Vec<&'a Symbol> },
}

impl ResolveError<'_> {
    /// Why the path did not resolve, as a short phrase such as
    /// `symbol not found; did you mean: /version`. The phrase does not include the path itself.
    ///
    /// `source` is the text of the file the symbols come from. It places each ambiguous candidate
    /// at its line and column.
    pub(crate) fn reason(&self, source: &str) -> String {
        match self {
            ResolveError::NotFound { hints } if hints.is_empty() => "symbol not found".to_string(),
            ResolveError::NotFound { hints } => {
                let hints: Vec<String> = hints.iter().map(|hint| hint.to_string()).collect();
                format!("symbol not found; did you mean: {}", hints.join(", "))
            }
            ResolveError::Ambiguous { candidates } => {
                // The candidates share one path, and can share a line too, so only a line and a
                // column tell them apart. Where a candidate is written first is enough for that.
                let positions: Vec<String> = candidates
                    .iter()
                    .map(|candidate| {
                        let ranges = candidate.position_ranges(source);
                        let start = &ranges[0].start;
                        format!("{}:{}", start.line, start.character)
                    })
                    .collect();
                format!("ambiguous symbol, defined at {}", positions.join(", "))
            }
        }
    }
}

/// The most hints a path that does not resolve gets.
const MAX_HINTS: usize = 3;

/// How alike a path has to be to the one that does not resolve to be a hint, as a normalized edit
/// distance from 0 (nothing in common) to 1 (identical).
const MIN_HINT_SIMILARITY: f64 = 0.6;

/// Finds a matching `Symbol` in `symbols` for the given `path`.
///
/// A filter in `path`, such as `~[name=blue]`, stands for the one child of the path before it
/// whose `name` key holds the value `blue`. A child is a list item or a map entry. A string value
/// matches without its quotes, and any other value as written.
///
/// When no symbol has `path`, the hints are the paths that resemble it: those at least 60% alike
/// by edit distance, and those that end in the same segment. Their filters are swapped for the
/// children they matched. When a filter matches no child, the hints are `path` with the filter's
/// value swapped for the value each child holds.
///
/// # Errors
/// Returns [`ResolveError::NotFound`] when no symbol matches `path`, or a filter matches no child.
/// Returns [`ResolveError::Ambiguous`] when several symbols match `path`, or a filter matches
/// several children.
pub(crate) fn resolve<'a>(
    symbols: &'a [Symbol],
    path: &SymbolPath,
) -> Result<&'a Symbol, ResolveError<'a>> {
    let resolved = resolve_filters(symbols, path)?;
    let candidates: Vec<&Symbol> = symbols
        .iter()
        .filter(|symbol| symbol.path == resolved)
        .collect();
    match candidates.len() {
        0 => Err(ResolveError::NotFound {
            hints: hints(symbols.iter().map(|symbol| &symbol.path), &resolved),
        }),
        1 => Ok(candidates[0]),
        _ => Err(ResolveError::Ambiguous { candidates }),
    }
}

/// Returns `path` with each filter swapped for the child it matches among `symbols`, such as
/// `/items/1` for `/items/~[name=blue]`. A path without filters comes back unchanged.
///
/// # Errors
/// Returns [`ResolveError::NotFound`] when a filter matches no child, and
/// [`ResolveError::Ambiguous`] when it matches several. The error is about the first such filter.
fn resolve_filters<'a>(
    symbols: &'a [Symbol],
    path: &SymbolPath,
) -> Result<SymbolPath, ResolveError<'a>> {
    // Filters are swapped one at a time, from the left. So a filter looks only under the children
    // that the filters before it matched.
    let mut resolved = path.clone();
    for (index, segment) in path.segments().iter().enumerate() {
        let (key, value) = match segment {
            Segment::Key(_) => continue,
            Segment::Filter { key, value } => (key, value),
        };
        let parent = &resolved.segments()[..index];
        let matches: Vec<&Symbol> = child_keys(symbols, parent, key)
            .filter(|symbol| {
                symbol
                    .value
                    .as_ref()
                    .is_some_and(|held| held.text == *value)
            })
            .collect();
        match matches.len() {
            0 => {
                // The second call to `child_keys` walks `symbols` again. It happens only when no
                // child matches, so a filter that matches doesn't keep every key it looked at.
                let keys = child_keys(symbols, parent, key);
                return Err(ResolveError::NotFound {
                    hints: filter_hints(path, index, key, keys),
                });
            }
            1 => {
                let child = matches[0].path.segments()[index].clone();
                resolved = resolved.with_segment(index, child);
            }
            _ => {
                return Err(ResolveError::Ambiguous {
                    candidates: matches,
                });
            }
        }
    }
    Ok(resolved)
}

/// The `key` keys of the children of `parent` among `symbols`, in document order. For `parent`
/// `/items` and `key` `name`, they are the symbols at `/items/0/name`, `/items/1/name` and so on.
/// A child is a list item or a map entry.
fn child_keys<'s>(
    symbols: &'s [Symbol],
    parent: &[Segment],
    key: &str,
) -> impl Iterator<Item = &'s Symbol> {
    let key = Segment::Key(key.to_string());
    // A key deeper inside a child, such as `/items/0/content/name`, describes something inside the
    // child, not the child itself.
    symbols.iter().filter(move |symbol| {
        let segments = symbol.path.segments();
        segments.len() == parent.len() + 2
            && segments.starts_with(parent)
            && segments[parent.len() + 1] == key
    })
}

/// The paths to suggest when the filter at `index` in `path`, on `key`, matches none of `keys`:
/// `path` with the filter's value swapped for the value each of `keys` holds. The most alike come
/// first.
fn filter_hints<'s>(
    path: &SymbolPath,
    index: usize,
    key: &str,
    keys: impl Iterator<Item = &'s Symbol>,
) -> Vec<SymbolPath> {
    // The hints start from `path`, so the filters before `index` stay as the reference wrote them.
    let alternatives: Vec<SymbolPath> = keys
        // A key that holds a list or a map has no value to put in a filter.
        .filter_map(|symbol| symbol.value.as_ref())
        .map(|held| {
            let filter = Segment::Filter {
                key: key.to_string(),
                value: held.text.clone(),
            };
            path.clone().with_segment(index, filter)
        })
        .collect();
    hints(&alternatives, path)
}

/// The paths among `candidates` that resemble `path`, the most alike first.
fn hints<'p>(
    candidates: impl IntoIterator<Item = &'p SymbolPath>,
    path: &SymbolPath,
) -> Vec<SymbolPath> {
    // The paths are compared as written, escapes included, so that a `/` left unescaped in a key
    // reads as a small difference.
    let written = path.to_string();
    let mut alike: Vec<(f64, &SymbolPath)> = candidates
        .into_iter()
        .map(|candidate| {
            let similarity = strsim::normalized_levenshtein(&written, &candidate.to_string());
            (similarity, candidate)
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
        .map(|(_, candidate)| candidate.clone())
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
    /// A scalar node contributing a value. Its range is where the value is written.
    Value,
}

/// Turns a captured `@name` or `@value` node into the text it denotes in `source`, following the
/// language's own quoting and escaping rules.
///
/// # Errors
/// Returns an error if the node's text is not a valid spelling in the language.
type NodeDecoder = fn(&tree_sitter::Node, &str) -> anyhow::Result<String>;

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
    /// Returns an error if the decoder rejects a captured name or value.
    fn collect_contributions(
        &self,
        tree: &tree_sitter::Tree,
        source: &str,
    ) -> anyhow::Result<HashMap<usize, Contribution>> {
        let mut cursor = tree_sitter::QueryCursor::new();
        let mut matches = cursor.matches(&self.query, tree.root_node(), source.as_bytes());

        let mut contributions = HashMap::new();
        while let Some(query_match) = matches.next() {
            let match_captures = MatchCaptures::new(query_match.captures(), &self.capture_roles);
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
        ensure_no_syntax_error(&tree, source)?;
        let contributions = self.collect_contributions(&tree, source)?;
        Ok(symbols_from_contributions(&tree, &contributions))
    }
}

/// Refuses `tree`, parsed from `source`, if it has a syntax error.
///
/// # Errors
/// Returns an error that shows the line and column of the first syntax error.
pub(crate) fn ensure_no_syntax_error(tree: &tree_sitter::Tree, source: &str) -> anyhow::Result<()> {
    // tree-sitter recovers from a syntax error by guessing what the source meant, and a path
    // resolved through a guess can land on a wrong value. A missing value, for one, is recovered
    // as an empty one.
    match first_syntax_error(tree) {
        Some(error) => {
            let position = Position::from_byte_offset(source, error.start_byte());
            bail!(
                "file has a syntax error at line {}, column {}, and a path into it could resolve to the wrong value",
                position.line,
                position.character,
            )
        }
        None => Ok(()),
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
}

impl<'tree> MatchCaptures<'tree> {
    /// Groups the captures of one query match by role. `captures` are that match's captures, and
    /// `capture_roles` holds the role of every capture its query declares, indexed by the capture
    /// index each [`tree_sitter::QueryCapture`] carries.
    ///
    /// The `@name` captures are kept in source order. For every other role, the last capture of that
    /// role in the match wins.
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
        let value = match self.value {
            Some(value) => Some(ScalarValue {
                text: node_decoder(&value, source)?,
                byte_range: value.byte_range(),
            }),
            None => None,
        };
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
        value: Option<ScalarValue>,
    },
    /// `@item`: addressable by its names followed by its index among its parent's items.
    Item {
        names: Vec<String>,
        value: Option<ScalarValue>,
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

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
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
            .map(|symbol| {
                let value = symbol.value.map(|value| value.text);
                (symbol.path.to_string(), value)
            })
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
        let value = symbols[0].value.as_ref().map(|value| value.text.as_str());
        assert_eq!(value, Some("42"));
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
                def_byte_ranges: vec![1..9],
                value: Some(ScalarValue {
                    text: r#""V""#.to_string(),
                    byte_range: 6..9,
                }),
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

        type PathAndRanges = (String, Vec<Range<usize>>, Vec<Range<Position>>);
        let ranges: Vec<PathAndRanges> = symbols
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
                    "/%C3%A9".to_string(),
                    vec![4..17],
                    vec![Position::new(2, 3)..Position::new(2, 15)]
                ),
                (
                    "/%C3%A9/0".to_string(),
                    vec![11..12],
                    vec![Position::new(2, 9)..Position::new(2, 10)]
                ),
                (
                    "/%C3%A9/1".to_string(),
                    vec![14..16],
                    vec![Position::new(2, 12)..Position::new(2, 14)]
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
#[allow(clippy::single_range_in_vec_init)]
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
            def_byte_ranges: vec![start..start + 1],
            value: None,
        }
    }

    /// A symbol at `path` that holds the scalar `value`.
    fn scalar(path_text: &str, start: usize, value: &str) -> Symbol {
        Symbol {
            value: Some(ScalarValue {
                text: value.to_string(),
                byte_range: start..start + 1,
            }),
            ..symbol(path_text, start)
        }
    }

    /// The symbols of the list `/items`, with one item for each of `names`. Each item is a map
    /// with two keys: `name`, which holds one of `names`, and `content`, which holds a list or a
    /// map.
    fn named_items(names: &[&str]) -> Vec<Symbol> {
        names
            .iter()
            .enumerate()
            .flat_map(|(index, name)| {
                [
                    scalar(&format!("/items/{index}/name"), 2 * index, name),
                    symbol(&format!("/items/{index}/content"), 2 * index + 1),
                ]
            })
            .collect()
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
                hints: vec![symbols[1].path.clone()]
            })
        );
    }

    #[test]
    fn path_with_an_unescaped_slash_returns_the_escaped_path_as_a_hint() {
        let symbols = [symbol("/dependencies/@types~1node", 0), symbol("/name", 1)];

        assert_eq!(
            resolve(&symbols, &path("/dependencies/@types/node")),
            Err(ResolveError::NotFound {
                hints: vec![symbols[0].path.clone()]
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
                hints: vec![symbols[0].path.clone()]
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
                hints: vec![
                    symbols[2].path.clone(),
                    symbols[3].path.clone(),
                    symbols[1].path.clone(),
                ]
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
                hints: vec![symbols[0].path.clone()]
            })
        );
    }

    #[test]
    fn filter_matching_one_child_returns_the_symbol_under_it() {
        let symbols = named_items(&["red", "blue"]);

        assert_eq!(
            resolve(&symbols, &path("/items/~[name=blue]/content")),
            Ok(&symbols[3])
        );
    }

    #[test]
    fn filter_matching_several_children_returns_their_keys_as_ambiguous() {
        let symbols = named_items(&["blue", "red", "blue"]);

        assert_eq!(
            resolve(&symbols, &path("/items/~[name=blue]/content")),
            Err(ResolveError::Ambiguous {
                candidates: vec![&symbols[0], &symbols[4]]
            })
        );
    }

    #[test]
    fn filter_matching_no_child_returns_the_other_values_as_hints() {
        // `blues` is more alike to `blue` than `red` is, so it comes first.
        let symbols = named_items(&["red", "blues"]);

        assert_eq!(
            resolve(&symbols, &path("/items/~[name=blue]/content")),
            Err(ResolveError::NotFound {
                hints: vec![
                    path("/items/~[name=blues]/content"),
                    path("/items/~[name=red]/content"),
                ]
            })
        );
    }

    #[test]
    fn filters_in_two_segments_resolve_one_after_the_other() {
        // Both groups have an item named `blue`, so the second filter has to look only under
        // `second`.
        let symbols = [
            scalar("/groups/0/name", 0, "first"),
            scalar("/groups/0/items/0/name", 1, "blue"),
            symbol("/groups/0/items/0/content", 2),
            scalar("/groups/1/name", 3, "second"),
            scalar("/groups/1/items/0/name", 4, "blue"),
            symbol("/groups/1/items/0/content", 5),
        ];

        assert_eq!(
            resolve(
                &symbols,
                &path("/groups/~[name=second]/items/~[name=blue]/content")
            ),
            Ok(&symbols[5])
        );
    }

    #[test]
    fn filter_at_the_root_selects_a_list_item() {
        // The file is a list, so no key comes before the filter.
        let symbols = [
            symbol("/0", 0),
            scalar("/0/name", 1, "red"),
            symbol("/1", 2),
            scalar("/1/name", 3, "blue"),
        ];

        assert_eq!(resolve(&symbols, &path("/~[name=blue]")), Ok(&symbols[2]));
    }

    #[test]
    fn filter_selects_an_entry_of_a_map() {
        // `b` is a key of a map, not a position in a list.
        let symbols = [
            symbol("/items/a", 0),
            scalar("/items/a/name", 1, "red"),
            symbol("/items/b", 2),
            scalar("/items/b/name", 3, "blue"),
        ];

        assert_eq!(
            resolve(&symbols, &path("/items/~[name=blue]")),
            Ok(&symbols[2])
        );
    }

    #[test]
    fn filter_ignores_a_value_deeper_inside_the_key() {
        let symbols = [
            symbol("/items/0", 0),
            symbol("/items/0/name", 1),
            scalar("/items/0/name/first", 2, "blue"),
        ];

        assert_eq!(
            resolve(&symbols, &path("/items/~[name=blue]")),
            Err(ResolveError::NotFound { hints: vec![] })
        );
    }

    #[test]
    fn filter_ignores_another_key_that_holds_the_value() {
        let symbols = [symbol("/items/0", 0), scalar("/items/0/color", 1, "blue")];

        assert_eq!(
            resolve(&symbols, &path("/items/~[name=blue]")),
            Err(ResolveError::NotFound { hints: vec![] })
        );
    }

    #[test]
    fn second_filter_matching_no_child_returns_hints_with_the_first_filter_as_written() {
        let symbols = [
            scalar("/groups/0/name", 0, "first"),
            scalar("/groups/0/items/0/name", 1, "blue"),
        ];

        assert_eq!(
            resolve(
                &symbols,
                &path("/groups/~[name=first]/items/~[name=red]/content")
            ),
            Err(ResolveError::NotFound {
                hints: vec![path("/groups/~[name=first]/items/~[name=blue]/content")]
            })
        );
    }

    #[test]
    fn filter_on_a_key_without_a_value_leaves_that_key_out_of_hints() {
        // The `name` of the first item is a list or a map, so it has no value to compare.
        let symbols = [
            symbol("/items/0/name", 0),
            scalar("/items/1/name", 1, "red"),
        ];

        assert_eq!(
            resolve(&symbols, &path("/items/~[name=blue]/content")),
            Err(ResolveError::NotFound {
                hints: vec![path("/items/~[name=red]/content")]
            })
        );
    }

    #[test]
    fn filter_then_missing_key_returns_the_alike_paths_under_the_match_as_hints() {
        // The hint is alike to `/items/0/contents`, the path with the filter resolved. The path as
        // written is too long to be 60% alike to it.
        let symbols = named_items(&["blue"]);

        assert_eq!(
            resolve(&symbols, &path("/items/~[name=blue]/contents")),
            Err(ResolveError::NotFound {
                hints: vec![path("/items/0/content")]
            })
        );
    }

    #[test]
    fn duplicate_keys_on_one_line_reason_lists_each_line_and_column() {
        let source = r#"{"v": 1, "v": 2}"#;
        let symbols = [symbol("/v", 1), symbol("/v", 9)];
        let error = ResolveError::Ambiguous {
            candidates: vec![&symbols[0], &symbols[1]],
        };

        assert_eq!(
            error.reason(source),
            "ambiguous symbol, defined at 1:2, 1:10"
        );
    }

    #[test]
    fn missing_path_with_hints_reason_lists_them_as_written() {
        let hints = [
            path("/dependencies/@types~1node"),
            path("/dependencies/zod"),
        ];
        let error = ResolveError::NotFound {
            hints: hints.to_vec(),
        };

        assert_eq!(
            error.reason(""),
            "symbol not found; did you mean: /dependencies/@types~1node, /dependencies/zod"
        );
    }

    #[test]
    fn missing_path_without_hints_reason_says_only_that() {
        let error = ResolveError::NotFound { hints: vec![] };

        assert_eq!(error.reason(""), "symbol not found");
    }
}

#[cfg(test)]
mod symbol_tests {
    use super::*;

    /// A source whose first and third lines hold a symbol written in two places, and the symbol.
    fn symbol_written_twice() -> (&'static str, Symbol) {
        let source = "first\nunrelated\nsecond\n";
        let symbol = Symbol {
            path: SymbolPath::parse("/a").expect("the path is valid"),
            def_byte_ranges: vec![0..6, 16..23],
            value: None,
        };
        (source, symbol)
    }

    #[test]
    fn symbol_with_several_ranges_value_or_definition_joins_their_text_in_order() {
        let (source, symbol) = symbol_written_twice();

        assert_eq!(symbol.value_or_definition(source), "first\nsecond\n");
    }

    #[test]
    fn symbol_with_several_ranges_position_ranges_returns_one_per_range() {
        let (source, symbol) = symbol_written_twice();

        assert_eq!(
            symbol.position_ranges(source),
            vec![
                Position::new(1, 1)..Position::new(2, 1),
                Position::new(3, 1)..Position::new(4, 1),
            ]
        );
    }
}
