//! Derives the symbols of a file: the elements a symbol reference can address.

mod path;

pub(crate) use path::SymbolPath;

use crate::position::{LineIndex, Position};
use path::Segment;
use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;
use std::ops::Range;

/// An addressable element of a source file, such as a JSON key, an array item or a Markdown
/// section.
///
/// Contains the rooted [`SymbolPath`], the byte ranges where the symbol is written, and its value
/// if it has one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Symbol {
    /// The symbol path of this symbol (e.g. `/dependencies/inngest`).
    pub(crate) path: SymbolPath,
    /// The byte ranges of the source where the symbol is written, in document order and not
    /// overlapping. Never empty. A symbol written in several places has one range for each.
    pub(crate) def_byte_ranges: Vec<Range<usize>>,
    /// The value: a scalar, or the body of a Markdown section. `None` for a container, such as a
    /// JSON object.
    pub(crate) value: Option<ScalarValue>,
}

impl Symbol {
    /// The 1-based line and character range of each of the symbol's byte ranges in `source`,
    /// which must be the text the symbol was derived from.
    ///
    /// Takes time proportional to the length of `source`, so it suits the symbol a reference
    /// resolves to rather than every symbol a file derives.
    ///
    /// # Panics
    /// Panics if a byte range does not fit `source`.
    pub(crate) fn position_ranges(&self, source: &str) -> Vec<Range<Position>> {
        let lines = LineIndex::new(source);
        self.def_byte_ranges
            .iter()
            .map(|range| lines.range(range))
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
            #[expect(
                clippy::string_slice,
                reason = "parsers give symbol ranges of whole characters"
            )]
            (None, [range]) => Cow::Borrowed(&source[range.clone()]),
            #[expect(
                clippy::string_slice,
                reason = "parsers give symbol ranges of whole characters"
            )]
            (None, ranges) => {
                Cow::Owned(ranges.iter().map(|range| &source[range.clone()]).collect())
            }
        }
    }
}

/// `range` extended to just after the line break that ends its last line in `source`, or to the
/// end of `source` when no line break follows.
///
/// # Panics
/// Panics if `range` does not end on a character of `source`.
pub(crate) fn extended_past_line_break(source: &str, range: Range<usize>) -> Range<usize> {
    // `LineChanges::touches` places a deleted line at the start of the line after it. Ending
    // after the line break puts that point inside the range, so deleting the last line of a
    // symbol counts as touching it.
    #[expect(
        clippy::string_slice,
        reason = "parsers give symbol ranges of whole characters"
    )]
    let end = source[range.end..]
        .find('\n')
        .map_or(source.len(), |offset| range.end + offset + 1);
    range.start..end
}

/// The symbols of one file, and the names a reference must not resolve in it.
#[derive(Debug, Default)]
pub(crate) struct Symbols {
    /// Every symbol, in document order. Two symbols can share a path, when the source defines the
    /// same name twice.
    pub(crate) symbols: Vec<Symbol>,
    /// The paths whose definition or value blockwatch can't read reliably, each with the reason to
    /// report, such as a syntax error in the definition. A path here can also be in `symbols`.
    pub(crate) unreadable: HashMap<SymbolPath, String>,
    /// The words in the text that the parser had to skip, each with the reason to report. That
    /// text could define any of them, so every path with one of them as a segment is unreadable.
    pub(crate) skipped: HashMap<String, String>,
}

impl From<Vec<Symbol>> for Symbols {
    fn from(symbols: Vec<Symbol>) -> Self {
        Self {
            symbols,
            unreadable: HashMap::new(),
            skipped: HashMap::new(),
        }
    }
}

impl Symbols {
    /// The `symbols` of a file, in document order, and its `unreadable` paths, each with the
    /// reason to report.
    pub(crate) fn with_unreadable(
        symbols: Vec<Symbol>,
        unreadable: HashMap<SymbolPath, String>,
    ) -> Self {
        Self {
            symbols,
            unreadable,
            skipped: HashMap::new(),
        }
    }

    /// The `symbols` of a file, in document order, and the words in the text its parser had to
    /// skip, each with the reason to report.
    pub(crate) fn with_skipped(symbols: Vec<Symbol>, skipped: HashMap<String, String>) -> Self {
        Self {
            symbols,
            unreadable: HashMap::new(),
            skipped,
        }
    }

    /// Finds the one symbol at `path`.
    ///
    /// A filter in `path`, such as `~[name=blue]`, stands for the one child of the path before it
    /// whose `name` key holds the value `blue`. A child is a list item or a map entry. A string
    /// value matches without its quotes, and any other value as written. When several children
    /// share a path, such as two `ingress` blocks in HCL, the rest of the path looks only inside
    /// the one that the filter picked.
    ///
    /// When no symbol has `path`, the hints are the paths that resemble it: those at least 60%
    /// alike by edit distance, and those that end in the same segment. When `path` has a filter,
    /// they are only paths under the child that the last filter matched, and they keep every
    /// filter as written. When a filter matches no child, the hints are `path` with the filter's
    /// value swapped for each value of a child that makes the path resolve.
    ///
    /// # Errors
    /// Returns [`ResolveError::Unreadable`] when `path` is an unreadable path, or one of its
    /// segments is a word the parser skipped. So does a filter that leads to an unreadable path,
    /// or whose key is unreadable in one of the children. Returns [`ResolveError::NotFound`] when no symbol
    /// matches `path`, or a filter matches no child. Returns [`ResolveError::AmbiguousSymbol`]
    /// when several symbols match `path`, and [`ResolveError::AmbiguousFilter`] when a filter
    /// matches several children.
    pub(crate) fn resolve(&self, path: &SymbolPath) -> Result<&Symbol, ResolveError<'_>> {
        if let Some(unreadable) = self.unreadable_reason(path) {
            return Err(ResolveError::Unreadable(unreadable));
        }
        let (resolved, scope) = resolve_filters(self, path)?;
        let candidates: Vec<&Symbol> = in_scope(self, scope)
            .filter(|symbol| symbol.path == resolved)
            .collect();
        match candidates.len() {
            0 => Err(ResolveError::NotFound {
                hints: path_hints(self, scope, path, &resolved),
            }),
            1 => Ok(candidates[0]),
            _ => Err(ResolveError::AmbiguousSymbol { candidates }),
        }
    }

    /// Finds every symbol at `path`, in document order.
    ///
    /// A filter in `path` that matches several children looks for the rest of the path under each
    /// of them. Returns an empty list when no symbol matches `path`.
    ///
    /// # Errors
    /// Returns [`Unreadable`] when `path` is unreadable, a segment of `path` is a word the parser
    /// skipped, a filter compares an unreadable key, or a matched child leads to an unreadable
    /// path.
    pub(crate) fn resolve_all(&self, path: &SymbolPath) -> Result<Vec<&Symbol>, Unreadable<'_>> {
        if let Some(unreadable) = self.unreadable_reason(path) {
            return Err(unreadable);
        }
        let mut found = Vec::new();
        resolve_all_from(self, path, None, &mut found)?;
        Ok(found)
    }

    /// Why `path` itself cannot be read reliably, if it is unreadable or one of its key segments
    /// is a word the parser skipped.
    fn unreadable_reason(&self, path: &SymbolPath) -> Option<Unreadable<'_>> {
        // A problem in the path's own definition is closer than a skipped word, so it is reported
        // first.
        self.unreadable
            .get(path)
            .or_else(|| {
                path.segments().iter().find_map(|segment| match segment {
                    Segment::Key(name) => self.skipped.get(name),
                    // A filter stands for a list item or a map entry, not for a name the file
                    // defines.
                    Segment::Filter { .. } => None,
                })
            })
            .map(|reason| Unreadable { reason })
    }
}

/// The value of a [`Symbol`]: a scalar, or the body of a Markdown section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ScalarValue {
    /// The text of the value. A string has no quotes and its escapes are decoded. Anything else
    /// is kept as written.
    pub(crate) text: String,
    /// The byte range of the source where the value is written, quotes included.
    pub(crate) byte_range: Range<usize>,
}

/// A symbol path whose definition or value blockwatch can't read reliably.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Unreadable<'a> {
    reason: &'a str,
}

impl fmt::Display for Unreadable<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.reason)
    }
}

/// Why a symbol path did not resolve to exactly one symbol.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ResolveError<'a> {
    /// No symbol has the path. `hints` holds the paths that resemble it, the most alike first.
    NotFound { hints: Vec<SymbolPath> },
    /// Several symbols have the path. `candidates` holds every one of them, in document order.
    AmbiguousSymbol { candidates: Vec<&'a Symbol> },
    /// `filter`, a filter in the path, matches several children. `keys` holds the key of each one
    /// that holds the filter's value, in document order.
    AmbiguousFilter {
        filter: Segment,
        keys: Vec<&'a Symbol>,
    },
    /// The path starts with a name whose definition or value blockwatch can't read reliably.
    Unreadable(Unreadable<'a>),
}

impl ResolveError<'_> {
    /// Why the path did not resolve, as a short phrase such as
    /// `symbol not found; did you mean: /version`. The phrase does not include the path itself.
    ///
    /// `source` is the text of the file the symbols come from. It places each ambiguous candidate,
    /// and each key an ambiguous filter matched, at its line and column.
    pub(crate) fn reason(&self, source: &str) -> String {
        match self {
            ResolveError::NotFound { hints } if hints.is_empty() => "symbol not found".to_string(),
            ResolveError::NotFound { hints } => {
                let hints: Vec<String> =
                    hints.iter().map(std::string::ToString::to_string).collect();
                format!("symbol not found; did you mean: {}", hints.join(", "))
            }
            ResolveError::AmbiguousSymbol { candidates } => {
                format!(
                    "ambiguous symbol, defined at {}",
                    positions(candidates, source)
                )
            }
            ResolveError::AmbiguousFilter { filter, keys } => {
                format!(
                    "{} items match {filter}, at {}",
                    keys.len(),
                    positions(keys, source)
                )
            }
            ResolveError::Unreadable(unreadable) => unreadable.to_string(),
        }
    }
}

/// Where each of `symbols` is first written in `source`, as `line:column`, joined with `, `.
fn positions(symbols: &[&Symbol], source: &str) -> String {
    // Symbols can share a path and a line, so only a line and a column tell them apart. Where a
    // symbol is first written is enough for that.
    let positions: Vec<String> = symbols
        .iter()
        .map(|symbol| {
            let ranges = symbol.position_ranges(source);
            let start = &ranges[0].start;
            format!("{}:{}", start.line, start.character)
        })
        .collect();
    positions.join(", ")
}

/// The most hints a path that does not resolve gets.
const MAX_HINTS: usize = 3;

/// How alike a path has to be to the one that does not resolve to be a hint, as a normalized edit
/// distance from 0 (nothing in common) to 1 (identical).
const MIN_HINT_SIMILARITY: f64 = 0.6;

/// Returns `path` with each filter swapped for the child it matches among `symbols`, such as
/// `/items/1` for `/items/~[name=blue]`, and the item that the rest of the path must sit inside,
/// if any. A path without filters comes back unchanged with `None`.
///
/// When several children share the path of the one a filter matches, such as two `ingress` blocks
/// in HCL, the returned item is the child that holds the matched key, if one does.
///
/// # Errors
/// Returns [`ResolveError::NotFound`] when a filter matches no child, and
/// [`ResolveError::AmbiguousFilter`] when it matches several. The error is about the first such
/// filter.
fn resolve_filters<'a>(
    symbols: &'a Symbols,
    path: &SymbolPath,
) -> Result<(SymbolPath, Option<&'a Symbol>), ResolveError<'a>> {
    // Filters are swapped one at a time, from the left. So a filter looks only under the children
    // that the filters before it matched.
    let mut resolved = path.clone();
    let mut scope: Option<&Symbol> = None;
    for (index, segment) in path.segments().iter().enumerate() {
        let (key, value) = match segment {
            Segment::Key(_) => continue,
            Segment::Filter { key, value } => (key, value),
        };
        let parent = &resolved.segments()[..index];
        if let Some(unreadable) = unreadable_child_key(symbols, parent, key) {
            return Err(ResolveError::Unreadable(unreadable));
        }
        let matches = matching_child_keys(symbols, scope, parent, key, value);
        match matches.as_slice() {
            [] => {
                // The second call to `child_keys` walks `scope` again. It happens only when no
                // child matches, so a filter that matches doesn't keep every key it looked at.
                let keys = child_keys(symbols, scope, parent, key);
                return Err(ResolveError::NotFound {
                    hints: filter_hints(symbols, path, index, key, keys),
                });
            }
            [key] => {
                resolved = resolved.with_segment(index, key.path.segments()[index].clone());
                scope = narrow_to_child(symbols, scope, &resolved.segments()[..=index], key);
            }
            _ => {
                return Err(ResolveError::AmbiguousFilter {
                    filter: segment.clone(),
                    keys: matches,
                });
            }
        }
    }
    // An unreadable item has no symbol, so the filter may have picked another item at its path.
    if let Some(reason) = symbols.unreadable.get(&resolved) {
        return Err(ResolveError::Unreadable(Unreadable { reason }));
    }
    Ok((resolved, scope))
}

/// Appends every symbol matching `resolved` inside `scope`, in document order.
///
/// # Errors
/// Returns [`Unreadable`] when a filter compares an unreadable key or leads to an unreadable path.
fn resolve_all_from<'a>(
    symbols: &'a Symbols,
    resolved: &SymbolPath,
    scope: Option<&'a Symbol>,
    found: &mut Vec<&'a Symbol>,
) -> Result<(), Unreadable<'a>> {
    let next_filter = resolved
        .segments()
        .iter()
        .enumerate()
        .find_map(|(index, segment)| match segment {
            Segment::Key(_) => None,
            Segment::Filter { key, value } => Some((index, key, value)),
        });
    let Some((index, key, value)) = next_filter else {
        if let Some(reason) = symbols.unreadable.get(resolved) {
            return Err(Unreadable { reason });
        }
        found.extend(in_scope(symbols, scope).filter(|symbol| symbol.path == *resolved));
        return Ok(());
    };
    let parent = &resolved.segments()[..index];
    if let Some(unreadable) = unreadable_child_key(symbols, parent, key) {
        return Err(unreadable);
    }
    for matched_key in matching_child_keys(symbols, scope, parent, key, value) {
        let child_path = resolved
            .clone()
            .with_segment(index, matched_key.path.segments()[index].clone());
        let child_scope = narrow_to_child(
            symbols,
            scope,
            &child_path.segments()[..=index],
            matched_key,
        );
        resolve_all_from(symbols, &child_path, child_scope, found)?;
    }
    Ok(())
}

/// The symbols of `symbols` written inside `scope`, or every symbol when `scope` is `None`, in
/// document order.
fn in_scope<'a>(
    symbols: &'a Symbols,
    scope: Option<&'a Symbol>,
) -> impl Iterator<Item = &'a Symbol> {
    symbols
        .symbols
        .iter()
        .filter(move |symbol| scope.is_none_or(|container| is_inside(symbol, container)))
}

/// Why a child of `parent` has an unreadable `key`, if any.
fn unreadable_child_key<'a>(
    symbols: &'a Symbols,
    parent: &[Segment],
    key: &str,
) -> Option<Unreadable<'a>> {
    // An unreadable path has no symbol, so the child it belongs to can't be told apart. Its
    // key could hold the filter's value too.
    symbols
        .unreadable
        .iter()
        .find(|(unreadable, _)| is_child_key(unreadable, parent, key))
        .map(|(_, reason)| Unreadable { reason })
}

/// The `key` keys of the children of `parent` inside `scope` whose scalar value is `value`, in
/// document order.
fn matching_child_keys<'s>(
    symbols: &'s Symbols,
    scope: Option<&'s Symbol>,
    parent: &[Segment],
    key: &str,
    value: &str,
) -> Vec<&'s Symbol> {
    child_keys(symbols, scope, parent, key)
        .filter(|symbol| symbol.value.as_ref().is_some_and(|held| held.text == value))
        .collect()
}

/// The child that holds `key` when several symbols inside `scope` have the path `child`, such as
/// two `ingress` blocks in HCL. Returns `scope` unchanged when at most one symbol has that path,
/// or when none of them holds `key`.
fn narrow_to_child<'a>(
    symbols: &'a Symbols,
    scope: Option<&'a Symbol>,
    child: &[Segment],
    key: &Symbol,
) -> Option<&'a Symbol> {
    let mut siblings = in_scope(symbols, scope).filter(|symbol| symbol.path.segments() == child);
    // A child alone at its path keeps every symbol under that path, even one written outside it,
    // such as a member that a Swift extension adds to a type.
    let (Some(first), Some(second)) = (siblings.next(), siblings.next()) else {
        return scope;
    };
    // A key written outside every child, such as in a Swift extension, picks none of them, so
    // the path stays ambiguous.
    [first, second]
        .into_iter()
        .chain(siblings)
        .find(|sibling| is_inside(key, sibling))
        .or(scope)
}

/// Whether `symbol` is written inside `item`: whether it starts where `item` is written.
fn is_inside(symbol: &Symbol, item: &Symbol) -> bool {
    // A file's symbols nest without overlapping, so a symbol that starts inside the item ends
    // inside it too.
    let start = symbol.def_byte_ranges[0].start;
    item.def_byte_ranges
        .iter()
        .any(|range| range.contains(&start))
}

/// The `key` keys of the children of `parent` inside `scope`, in document order. For `parent`
/// `/items` and `key` `name`, they are the symbols at `/items/0/name`, `/items/1/name` and so on.
/// A child is a list item or a map entry.
fn child_keys<'s>(
    symbols: &'s Symbols,
    scope: Option<&'s Symbol>,
    parent: &[Segment],
    key: &str,
) -> impl Iterator<Item = &'s Symbol> {
    in_scope(symbols, scope).filter(move |symbol| is_child_key(&symbol.path, parent, key))
}

/// Whether `path` is the path of the `key` key of a child of `parent`, such as `/items/0/name`
/// for `parent` `/items` and `key` `name`.
fn is_child_key(path: &SymbolPath, parent: &[Segment], key: &str) -> bool {
    // A key deeper inside a child, such as `/items/0/content/name`, describes something inside the
    // child, not the child itself.
    let segments = path.segments();
    segments.len() == parent.len() + 2
        && segments.starts_with(parent)
        && match &segments[parent.len() + 1] {
            Segment::Key(name) => name == key,
            Segment::Filter { .. } => false,
        }
}

/// The paths to suggest when no symbol has `resolved`, which is `path` with its filters swapped
/// for the children they matched. The most alike come first.
///
/// When `path` has a filter, the hints are only paths under the child that the last filter
/// matched, written with every filter of `path`. Otherwise they come from all of `symbols`.
fn path_hints(
    symbols: &Symbols,
    scope: Option<&Symbol>,
    path: &SymbolPath,
    resolved: &SymbolPath,
) -> Vec<SymbolPath> {
    let last_filter = path.segments().iter().rposition(|segment| match segment {
        Segment::Key(_) => false,
        Segment::Filter { .. } => true,
    });
    match last_filter {
        None => hints(
            in_scope(symbols, scope).map(|symbol| &symbol.path),
            path,
            MIN_HINT_SIMILARITY,
        ),
        Some(last) => {
            // A path under another child points at an item that the filters did not pick. A
            // position in place of a filter breaks when an item is added above it.
            let matched = &resolved.segments()[..=last];
            let candidates: Vec<SymbolPath> = in_scope(symbols, scope)
                .filter(|symbol| symbol.path.segments().starts_with(matched))
                // The keys up to the last filter are the same in both paths, so only the filters
                // change.
                .map(|symbol| {
                    (0..=last).fold(symbol.path.clone(), |hint, index| {
                        hint.with_segment(index, path.segments()[index].clone())
                    })
                })
                .collect();
            hints(&candidates, path, MIN_HINT_SIMILARITY)
        }
    }
}

/// The paths to suggest when the filter at `index` in `path`, on `key`, matches none of `keys`:
/// `path` with the filter's value swapped for each value of `keys` that makes the path resolve in
/// `symbols`. The most alike come first.
fn filter_hints<'s>(
    symbols: &Symbols,
    path: &SymbolPath,
    index: usize,
    key: &str,
    keys: impl Iterator<Item = &'s Symbol>,
) -> Vec<SymbolPath> {
    // The hints start from `path`, so the filters before `index` stay as the reference wrote them.
    // A value that several children hold makes the filter ambiguous, so its path doesn't resolve.
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
        .filter(|alternative| symbols.resolve(alternative).is_ok())
        .collect();
    // Each alternative resolves and differs from `path` only in the filter's value, so a
    // similarity cutoff would only drop real choices.
    hints(&alternatives, path, 0.0)
}

/// The paths among `candidates` that resemble `path`, the most alike first: those at least
/// `min_similarity` alike, as a normalized edit distance, and those that end in the same segment.
fn hints<'p>(
    candidates: impl IntoIterator<Item = &'p SymbolPath>,
    path: &SymbolPath,
    min_similarity: f64,
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
            *similarity >= min_similarity || candidate.segments().last() == path.segments().last()
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
    /// Derives all the addressable symbols in `source`, and the names a reference must not
    /// resolve in it.
    ///
    /// # Errors
    /// Returns an error if no symbol of `source` can be derived reliably, such as when a data file
    /// has a syntax error. The error mentions the position of the problem when there is one.
    fn parse(&mut self, source: &str) -> anyhow::Result<Symbols>;
}

#[cfg(test)]
#[expect(
    clippy::single_range_in_vec_init,
    reason = "a `Vec` of one range is the expected value here"
)]
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

    /// A symbol at `path` written at `range`, so that it holds the symbols written inside `range`.
    fn element(path_text: &str, range: Range<usize>) -> Symbol {
        Symbol {
            path: path(path_text),
            def_byte_ranges: vec![range],
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

    /// The filter `~[key=value]`.
    fn filter(key: &str, value: &str) -> Segment {
        Segment::Filter {
            key: key.to_string(),
            value: value.to_string(),
        }
    }

    /// The symbols of one item of the list that `named_items` builds.
    struct ListItem {
        name: Symbol,
        content: Symbol,
    }

    /// The symbols of the list `/items`, with one item for each of `names`, and the symbols of
    /// each item in the same order. Each item is a map with two keys: `name`, which holds one of
    /// `names`, and `content`, which holds a list or a map.
    fn named_items<const N: usize>(names: [&str; N]) -> (Symbols, [ListItem; N]) {
        let items: [ListItem; N] = std::array::from_fn(|index| ListItem {
            name: scalar(&format!("/items/{index}/name"), 2 * index, names[index]),
            content: symbol(&format!("/items/{index}/content"), 2 * index + 1),
        });
        let symbols = items
            .iter()
            .flat_map(|item| [item.name.clone(), item.content.clone()])
            .collect::<Vec<Symbol>>();
        (symbols.into(), items)
    }

    #[test]
    fn path_of_one_symbol_returns_that_symbol() {
        let b = symbol("/b", 1);
        let symbols = Symbols::from(vec![symbol("/a/b", 0), b.clone()]);

        assert_eq!(symbols.resolve(&path("/b")), Ok(&b));
    }

    #[test]
    fn path_of_several_symbols_returns_every_one_as_ambiguous() {
        let first = symbol("/v", 0);
        let second = symbol("/v", 2);
        let symbols = Symbols::from(vec![first.clone(), symbol("/w", 1), second.clone()]);

        assert_eq!(
            symbols.resolve(&path("/v")),
            Err(ResolveError::AmbiguousSymbol {
                candidates: vec![&first, &second]
            })
        );
    }

    /// The symbols `/a`, `/a/b`, `/a/c`, `/b` and `/bc`, with `/a/c` unreadable and `b` skipped,
    /// and each of the symbols in that order.
    fn symbols_with_unreadable_names() -> (Symbols, [Symbol; 5]) {
        let named = [
            symbol("/a", 0),
            symbol("/a/b", 1),
            symbol("/a/c", 2),
            symbol("/b", 3),
            symbol("/bc", 4),
        ];
        let symbols = Symbols {
            symbols: named.to_vec(),
            unreadable: HashMap::from([(path("/a/c"), "syntax error in c".to_string())]),
            skipped: HashMap::from([("b".to_string(), "syntax error near b".to_string())]),
        };
        (symbols, named)
    }

    #[test]
    fn unreadable_path_returns_its_reason_even_when_a_symbol_has_it() {
        let (symbols, _) = symbols_with_unreadable_names();

        let error = symbols.resolve(&path("/a/c")).unwrap_err();

        assert_eq!(error.reason(""), "syntax error in c");
    }

    #[test]
    fn path_around_an_unreadable_path_resolves() {
        let (symbols, [a, ..]) = symbols_with_unreadable_names();

        assert_eq!(symbols.resolve(&path("/a")), Ok(&a));
    }

    #[test]
    fn skipped_word_returns_its_reason_even_when_a_symbol_has_it() {
        let (symbols, _) = symbols_with_unreadable_names();

        let error = symbols.resolve(&path("/b")).unwrap_err();

        assert_eq!(error.reason(""), "syntax error near b");
    }

    #[test]
    fn path_under_a_skipped_word_returns_its_reason() {
        let (symbols, _) = symbols_with_unreadable_names();

        assert_eq!(
            symbols.resolve(&path("/b/c")),
            Err(ResolveError::Unreadable(Unreadable {
                reason: "syntax error near b"
            }))
        );
    }

    #[test]
    fn path_that_ends_in_a_skipped_word_returns_its_reason() {
        let (symbols, _) = symbols_with_unreadable_names();

        assert_eq!(
            symbols.resolve(&path("/a/b")),
            Err(ResolveError::Unreadable(Unreadable {
                reason: "syntax error near b"
            }))
        );
    }

    #[test]
    fn name_that_starts_with_a_skipped_word_resolves() {
        let (symbols, [.., bc]) = symbols_with_unreadable_names();

        assert_eq!(symbols.resolve(&path("/bc")), Ok(&bc));
    }

    #[test]
    fn misspelled_path_returns_the_alike_paths_as_hints() {
        // `/description` is 50% alike, below the floor.
        let symbols = Symbols::from(vec![
            symbol("/description", 0),
            symbol("/version", 1),
            symbol("/name", 2),
        ]);

        assert_eq!(
            symbols.resolve(&path("/verison")),
            Err(ResolveError::NotFound {
                hints: vec![path("/version")]
            })
        );
    }

    #[test]
    fn path_with_an_unescaped_slash_returns_the_escaped_path_as_a_hint() {
        let symbols = Symbols::from(vec![
            symbol("/dependencies/@types~1node", 0),
            symbol("/name", 1),
        ]);

        assert_eq!(
            symbols.resolve(&path("/dependencies/@types/node")),
            Err(ResolveError::NotFound {
                hints: vec![path("/dependencies/@types~1node")]
            })
        );
    }

    #[test]
    fn path_moved_deeper_returns_it_as_a_hint() {
        // Both candidates are well below the floor, and only the first ends in the same segment.
        let symbols = Symbols::from(vec![
            symbol("/network/services/RetryConfig", 0),
            symbol("/network/services/timeout", 1),
        ]);

        assert_eq!(
            symbols.resolve(&path("/RetryConfig")),
            Err(ResolveError::NotFound {
                hints: vec![path("/network/services/RetryConfig")]
            })
        );
    }

    #[test]
    fn many_alike_paths_returns_the_three_most_alike_first() {
        // 62%, 73%, 89% and 80% alike.
        let symbols = Symbols::from(vec![
            symbol("/time", 0),
            symbol("/timeout_ms", 1),
            symbol("/timeouts", 2),
            symbol("/timeout_s", 3),
        ]);

        assert_eq!(
            symbols.resolve(&path("/timeout")),
            Err(ResolveError::NotFound {
                hints: vec![path("/timeouts"), path("/timeout_s"), path("/timeout_ms")]
            })
        );
    }

    #[test]
    fn path_exactly_at_the_similarity_floor_returns_it_as_a_hint() {
        // Two of five characters differ in the first, which is 60% alike, and three in the second.
        let symbols = Symbols::from(vec![symbol("/abXY", 0), symbol("/aXYZ", 1)]);

        assert_eq!(
            symbols.resolve(&path("/abcd")),
            Err(ResolveError::NotFound {
                hints: vec![path("/abXY")]
            })
        );
    }

    #[test]
    fn filter_matching_one_child_returns_the_symbol_under_it() {
        let (symbols, [_red, blue]) = named_items(["red", "blue"]);

        assert_eq!(
            symbols.resolve(&path("/items/~[name=blue]/content")),
            Ok(&blue.content)
        );
    }

    #[test]
    fn filter_matching_several_children_returns_their_keys_as_ambiguous() {
        let (symbols, [first_blue, _red, second_blue]) = named_items(["blue", "red", "blue"]);

        assert_eq!(
            symbols.resolve(&path("/items/~[name=blue]/content")),
            Err(ResolveError::AmbiguousFilter {
                filter: filter("name", "blue"),
                keys: vec![&first_blue.name, &second_blue.name]
            })
        );
    }

    #[test]
    fn filter_matching_no_child_returns_the_other_values_as_hints() {
        // `blues` is more alike to `blue` than `red` is, so it comes first.
        let (symbols, _) = named_items(["red", "blues"]);

        assert_eq!(
            symbols.resolve(&path("/items/~[name=blue]/content")),
            Err(ResolveError::NotFound {
                hints: vec![
                    path("/items/~[name=blues]/content"),
                    path("/items/~[name=red]/content"),
                ]
            })
        );
    }

    #[test]
    fn filter_matching_no_child_leaves_values_of_several_children_out_of_hints() {
        // `blues` is the most alike value, but two children hold it, so
        // `/items/~[name=blues]/content` would not resolve either.
        let (symbols, _) = named_items(["blues", "red", "blues"]);

        assert_eq!(
            symbols.resolve(&path("/items/~[name=blue]/content")),
            Err(ResolveError::NotFound {
                hints: vec![path("/items/~[name=red]/content")]
            })
        );
    }

    #[test]
    fn filter_matching_no_child_leaves_children_without_the_rest_of_the_path_out_of_hints() {
        // `blues` is the most alike value, but its item has no `content`.
        let symbols = Symbols::from(vec![
            scalar("/items/0/name", 0, "blues"),
            scalar("/items/1/name", 1, "red"),
            symbol("/items/1/content", 2),
        ]);

        assert_eq!(
            symbols.resolve(&path("/items/~[name=blue]/content")),
            Err(ResolveError::NotFound {
                hints: vec![path("/items/~[name=red]/content")]
            })
        );
    }

    #[test]
    fn filter_matching_no_child_leaves_children_where_a_later_filter_fails_out_of_hints() {
        // Only the `first` group has an item named `blue`.
        let symbols = Symbols::from(vec![
            scalar("/groups/0/name", 0, "first"),
            scalar("/groups/0/items/0/name", 1, "blue"),
            symbol("/groups/0/items/0/content", 2),
            scalar("/groups/1/name", 3, "second"),
            scalar("/groups/1/items/0/name", 4, "red"),
            symbol("/groups/1/items/0/content", 5),
        ]);

        assert_eq!(
            symbols.resolve(&path("/groups/~[name=secnd]/items/~[name=blue]/content")),
            Err(ResolveError::NotFound {
                hints: vec![path("/groups/~[name=first]/items/~[name=blue]/content")]
            })
        );
    }

    #[test]
    fn filter_at_the_end_matching_no_child_returns_hints_below_the_similarity_floor() {
        // `/~[name=crimson]` is 56% alike to `/~[name=blue]`, below the 60% that other hints need.
        let symbols = Symbols::from(vec![
            element("/0", 0..2),
            scalar("/0/name", 1, "red"),
            element("/1", 2..4),
            scalar("/1/name", 3, "crimson"),
        ]);

        assert_eq!(
            symbols.resolve(&path("/~[name=blue]")),
            Err(ResolveError::NotFound {
                hints: vec![path("/~[name=red]"), path("/~[name=crimson]")]
            })
        );
    }

    #[test]
    fn filters_in_two_segments_resolve_one_after_the_other() {
        // Both groups have an item named `blue`, so the second filter has to look only under
        // `second`.
        let second_content = symbol("/groups/1/items/0/content", 5);
        let symbols = Symbols::from(vec![
            scalar("/groups/0/name", 0, "first"),
            scalar("/groups/0/items/0/name", 1, "blue"),
            symbol("/groups/0/items/0/content", 2),
            scalar("/groups/1/name", 3, "second"),
            scalar("/groups/1/items/0/name", 4, "blue"),
            second_content.clone(),
        ]);

        assert_eq!(
            symbols.resolve(&path("/groups/~[name=second]/items/~[name=blue]/content")),
            Ok(&second_content)
        );
    }

    #[test]
    fn filter_at_the_root_selects_a_list_item() {
        // The file is a list, so no key comes before the filter.
        let blue = symbol("/1", 2);
        let symbols = Symbols::from(vec![
            symbol("/0", 0),
            scalar("/0/name", 1, "red"),
            blue.clone(),
            scalar("/1/name", 3, "blue"),
        ]);

        assert_eq!(symbols.resolve(&path("/~[name=blue]")), Ok(&blue));
    }

    #[test]
    fn filter_selects_an_entry_of_a_map() {
        // `b` is a key of a map, not a position in a list.
        let blue = symbol("/items/b", 2);
        let symbols = Symbols::from(vec![
            symbol("/items/a", 0),
            scalar("/items/a/name", 1, "red"),
            blue.clone(),
            scalar("/items/b/name", 3, "blue"),
        ]);

        assert_eq!(symbols.resolve(&path("/items/~[name=blue]")), Ok(&blue));
    }

    #[test]
    fn filter_ignores_a_value_deeper_inside_the_key() {
        let symbols = Symbols::from(vec![
            symbol("/items/0", 0),
            symbol("/items/0/name", 1),
            scalar("/items/0/name/first", 2, "blue"),
        ]);

        assert_eq!(
            symbols.resolve(&path("/items/~[name=blue]")),
            Err(ResolveError::NotFound { hints: vec![] })
        );
    }

    #[test]
    fn filter_ignores_another_key_that_holds_the_value() {
        let symbols = Symbols::from(vec![
            symbol("/items/0", 0),
            scalar("/items/0/color", 1, "blue"),
        ]);

        assert_eq!(
            symbols.resolve(&path("/items/~[name=blue]")),
            Err(ResolveError::NotFound { hints: vec![] })
        );
    }

    #[test]
    fn second_filter_matching_no_child_returns_hints_with_the_first_filter_as_written() {
        let symbols = Symbols::from(vec![
            scalar("/groups/0/name", 0, "first"),
            scalar("/groups/0/items/0/name", 1, "blue"),
            symbol("/groups/0/items/0/content", 2),
        ]);

        assert_eq!(
            symbols.resolve(&path("/groups/~[name=first]/items/~[name=red]/content")),
            Err(ResolveError::NotFound {
                hints: vec![path("/groups/~[name=first]/items/~[name=blue]/content")]
            })
        );
    }

    #[test]
    fn filter_on_a_key_without_a_value_leaves_that_key_out_of_hints() {
        // The `name` of the first item is a list or a map, so it has no value to compare.
        let symbols = Symbols::from(vec![
            symbol("/items/0/name", 0),
            scalar("/items/1/name", 1, "red"),
            symbol("/items/1/content", 2),
        ]);

        assert_eq!(
            symbols.resolve(&path("/items/~[name=blue]/content")),
            Err(ResolveError::NotFound {
                hints: vec![path("/items/~[name=red]/content")]
            })
        );
    }

    #[test]
    fn filter_then_missing_key_returns_hints_only_under_the_matched_child() {
        // `/items/1/content` is 88% alike to `/items/0/contents`, the path with the filter resolved,
        // but it belongs to `red`.
        let (symbols, _) = named_items(["blue", "red"]);

        assert_eq!(
            symbols.resolve(&path("/items/~[name=blue]/contents")),
            Err(ResolveError::NotFound {
                hints: vec![
                    path("/items/~[name=blue]/content"),
                    path("/items/~[name=blue]/name"),
                ]
            })
        );
    }

    #[test]
    fn filters_then_missing_key_return_hints_with_every_filter_as_written() {
        let symbols = Symbols::from(vec![
            scalar("/groups/0/name", 0, "first"),
            scalar("/groups/0/items/0/name", 1, "blue"),
            symbol("/groups/0/items/0/content", 2),
        ]);

        assert_eq!(
            symbols.resolve(&path("/groups/~[name=first]/items/~[name=blue]/contents")),
            Err(ResolveError::NotFound {
                hints: vec![
                    path("/groups/~[name=first]/items/~[name=blue]/content"),
                    path("/groups/~[name=first]/items/~[name=blue]/name"),
                ]
            })
        );
    }

    /// The symbols of one item that shares its path with other items.
    struct ItemAtOnePath {
        /// The item itself.
        element: Symbol,
        name: Symbol,
        content: Symbol,
    }

    impl ItemAtOnePath {
        /// The item at `path`, written at `range`, with its two keys: `name`, which holds `name`,
        /// and `content`.
        fn new(path_text: &str, range: Range<usize>, name: &str) -> Self {
            Self {
                element: element(path_text, range.clone()),
                name: scalar(&format!("{path_text}/name"), range.start + 1, name),
                content: symbol(&format!("{path_text}/content"), range.start + 2),
            }
        }

        /// The item's symbols, in document order.
        fn symbols(&self) -> [Symbol; 3] {
            [
                self.element.clone(),
                self.name.clone(),
                self.content.clone(),
            ]
        }
    }

    /// The symbols of `/items`, with one item for each of `names`, all at the path `/items/item`,
    /// as two `ingress` blocks in HCL are, and the symbols of each item in the same order. Each
    /// item starts right where the one before it ends.
    fn items_at_one_path<const N: usize>(names: [&str; N]) -> (Symbols, [ItemAtOnePath; N]) {
        let items: [ItemAtOnePath; N] = std::array::from_fn(|index| {
            let start = 10 * index;
            ItemAtOnePath::new("/items/item", start..start + 10, names[index])
        });
        let symbols = items
            .iter()
            .flat_map(ItemAtOnePath::symbols)
            .collect::<Vec<Symbol>>();
        (symbols.into(), items)
    }

    #[test]
    fn filter_over_items_at_one_path_returns_the_symbol_under_the_picked_item() {
        let (symbols, [_red, blue]) = items_at_one_path(["red", "blue"]);

        assert_eq!(
            symbols.resolve(&path("/items/~[name=blue]/content")),
            Ok(&blue.content)
        );
    }

    #[test]
    fn filter_at_the_end_over_items_at_one_path_returns_the_picked_item() {
        // `blue` starts right where `red` ends.
        let (symbols, [red, _blue]) = items_at_one_path(["red", "blue"]);

        assert_eq!(
            symbols.resolve(&path("/items/~[name=red]")),
            Ok(&red.element)
        );
    }

    #[test]
    fn filter_over_items_at_one_path_then_key_of_another_item_returns_not_found() {
        // Only `red` has `content`, and the hints come only from `blue`.
        let symbols = Symbols::from(vec![
            element("/items/item", 0..10),
            scalar("/items/item/name", 1, "red"),
            symbol("/items/item/content", 2),
            element("/items/item", 10..20),
            scalar("/items/item/name", 11, "blue"),
        ]);

        assert_eq!(
            symbols.resolve(&path("/items/~[name=blue]/content")),
            Err(ResolveError::NotFound {
                hints: vec![
                    path("/items/~[name=blue]/name"),
                    path("/items/~[name=blue]")
                ]
            })
        );
    }

    /// The symbols of one group that `groups_at_one_path` builds.
    struct GroupAtOnePath {
        name: Symbol,
        /// The one item in the group's list `items`.
        item: ItemAtOnePath,
    }

    /// The symbols of `/groups`, with one group for each of `groups`, all at the path
    /// `/groups/group`, and the symbols of each group in the same order. Each group holds a
    /// `name`, which holds the first value of its pair, and a list `items` with one item, whose
    /// `name` holds the second value.
    fn groups_at_one_path<const N: usize>(
        groups: [(&str, &str); N],
    ) -> (Symbols, [GroupAtOnePath; N]) {
        let mut symbols = Vec::new();
        let groups: [GroupAtOnePath; N] = std::array::from_fn(|index| {
            let (group_name, item_name) = groups[index];
            let start = 10 * index;
            let group = GroupAtOnePath {
                name: scalar("/groups/group/name", start + 1, group_name),
                item: ItemAtOnePath::new(
                    "/groups/group/items/item",
                    start + 3..start + 10,
                    item_name,
                ),
            };
            symbols.extend([
                element("/groups/group", start..start + 10),
                group.name.clone(),
                element("/groups/group/items", start + 2..start + 10),
            ]);
            symbols.extend(group.item.symbols());
            group
        });
        (symbols.into(), groups)
    }

    #[test]
    fn second_filter_over_groups_at_one_path_looks_only_inside_the_first_pick() {
        let (symbols, [_first, second]) =
            groups_at_one_path([("first", "blue"), ("second", "blue")]);

        assert_eq!(
            symbols.resolve(&path("/groups/~[name=second]/items/~[name=blue]/content")),
            Ok(&second.item.content)
        );
    }

    #[test]
    fn filter_matching_under_two_groups_at_one_path_returns_their_keys_as_ambiguous() {
        let (symbols, [first, second]) =
            groups_at_one_path([("first", "blue"), ("second", "blue")]);

        assert_eq!(
            symbols.resolve(&path("/groups/group/items/~[name=blue]/content")),
            Err(ResolveError::AmbiguousFilter {
                filter: filter("name", "blue"),
                keys: vec![&first.item.name, &second.item.name]
            })
        );
    }

    #[test]
    fn filter_under_a_parent_at_one_path_picks_among_the_items_of_every_copy() {
        // The groups have no key that tells them apart, so the filter can't pick a group first.
        let (symbols, [_red, blue]) = groups_at_one_path([("same", "red"), ("same", "blue")]);

        assert_eq!(
            symbols.resolve(&path("/groups/group/items/~[name=blue]/content")),
            Ok(&blue.item.content)
        );
    }

    #[test]
    fn second_filter_matching_no_child_returns_hints_only_from_the_first_pick() {
        let (symbols, _) = groups_at_one_path([("first", "blue"), ("second", "red")]);

        assert_eq!(
            symbols.resolve(&path("/groups/~[name=second]/items/~[name=green]/content")),
            Err(ResolveError::NotFound {
                hints: vec![path("/groups/~[name=second]/items/~[name=red]/content")]
            })
        );
    }

    #[test]
    fn filter_over_an_item_with_its_own_path_returns_a_symbol_written_outside_it() {
        // A Swift extension adds members to a type outside the type's text. The type is the only
        // item at its path, so the filter has no other item to tell it apart from.
        let extra = symbol("/Config/extra", 20);
        let symbols = Symbols::from(vec![
            element("/Config", 0..10),
            scalar("/Config/name", 1, "blue"),
            extra.clone(),
        ]);

        assert_eq!(symbols.resolve(&path("/~[name=blue]/extra")), Ok(&extra));
    }

    #[test]
    fn filter_over_items_at_one_path_with_the_key_outside_them_returns_them_as_ambiguous() {
        // A type defined twice, such as in two branches of an `#if`, and a Swift extension of it
        // outside both copies.
        let first = element("/Config", 0..10);
        let second = element("/Config", 10..20);
        let symbols = Symbols::from(vec![
            first.clone(),
            second.clone(),
            scalar("/Config/name", 30, "blue"),
        ]);

        assert_eq!(
            symbols.resolve(&path("/~[name=blue]")),
            Err(ResolveError::AmbiguousSymbol {
                candidates: vec![&first, &second]
            })
        );
    }

    #[test]
    fn second_filter_with_key_outside_inner_siblings_keeps_the_outer_pick() {
        // When an outer filter already narrowed to one sibling, an inner filter whose matched key
        // sits outside two inner siblings stays inside the outer sibling instead of widening to
        // other outer copies.
        let inner_a = element("/Outer/Inner", 22..26);
        let inner_b = element("/Outer/Inner", 26..30);
        let symbols = Symbols::from(vec![
            element("/Outer", 0..20),
            scalar("/Outer/name", 1, "first"),
            element("/Outer/Inner", 2..6),
            element("/Outer", 20..40),
            scalar("/Outer/name", 21, "second"),
            inner_a.clone(),
            inner_b.clone(),
            scalar("/Outer/Inner/name", 35, "blue"),
        ]);

        assert_eq!(
            symbols.resolve(&path("/~[name=second]/~[name=blue]")),
            Err(ResolveError::AmbiguousSymbol {
                candidates: vec![&inner_a, &inner_b]
            })
        );
    }

    /// The symbols of `items`, except `unreadable`, whose path is unreadable instead, as the path
    /// of a value that blockwatch can't decode is.
    fn with_unreadable(items: &[ItemAtOnePath], unreadable: &Symbol) -> Symbols {
        let symbols = items
            .iter()
            .flat_map(ItemAtOnePath::symbols)
            .filter(|symbol| symbol != unreadable)
            .collect();
        let reason = "can't decode".to_string();
        Symbols::with_unreadable(symbols, HashMap::from([(unreadable.path.clone(), reason)]))
    }

    #[test]
    fn filter_that_picks_an_unreadable_item_returns_its_reason() {
        // Only `blue` has a symbol at the path, so without the check the filter would read it.
        let (_, items) = items_at_one_path(["red", "blue"]);
        let [red, _blue] = &items;
        let symbols = with_unreadable(&items, &red.element);

        assert_eq!(
            symbols.resolve(&path("/items/~[name=red]")),
            Err(ResolveError::Unreadable(Unreadable {
                reason: "can't decode"
            }))
        );
    }

    #[test]
    fn filter_then_an_unreadable_key_of_the_picked_item_returns_its_reason() {
        let (_, items) = items_at_one_path(["red", "blue"]);
        let [red, _blue] = &items;
        let symbols = with_unreadable(&items, &red.content);

        assert_eq!(
            symbols.resolve(&path("/items/~[name=red]/content")),
            Err(ResolveError::Unreadable(Unreadable {
                reason: "can't decode"
            }))
        );
    }

    #[test]
    fn filter_over_items_with_an_unreadable_key_returns_its_reason() {
        // The first item's `name` could be `red` too, so the filter can't tell which item it picks.
        let (_, items) = items_at_one_path(["red", "red"]);
        let [first, _second] = &items;
        let symbols = with_unreadable(&items, &first.name);

        assert_eq!(
            symbols.resolve(&path("/items/~[name=red]/content")),
            Err(ResolveError::Unreadable(Unreadable {
                reason: "can't decode"
            }))
        );
    }

    #[test]
    fn filter_with_unreadable_keys_only_outside_its_children_resolves() {
        let (symbols, [_red, blue]) = items_at_one_path(["red", "blue"]);
        let unreadable = [
            "/other/item/name",
            "/items/item/content/name",
            "/items/name",
        ]
        .map(|unreadable| (path(unreadable), "can't decode".to_string()));
        let symbols = Symbols::with_unreadable(symbols.symbols, HashMap::from(unreadable));

        assert_eq!(
            symbols.resolve(&path("/items/~[name=blue]/content")),
            Ok(&blue.content)
        );
    }

    #[test]
    fn duplicate_keys_on_one_line_reason_lists_each_line_and_column() {
        let source = r#"{"v": 1, "v": 2}"#;
        let first = symbol("/v", 1);
        let second = symbol("/v", 9);
        let error = ResolveError::AmbiguousSymbol {
            candidates: vec![&first, &second],
        };

        assert_eq!(
            error.reason(source),
            "ambiguous symbol, defined at 1:2, 1:10"
        );
    }

    #[test]
    fn ambiguous_filter_reason_counts_the_children_and_lists_where_each_key_is() {
        let source = "- name: blue\n- name: blue\n";
        let first = symbol("/0/name", 2);
        let second = symbol("/1/name", 15);
        let error = ResolveError::AmbiguousFilter {
            filter: filter("name", "blue"),
            keys: vec![&first, &second],
        };

        assert_eq!(
            error.reason(source),
            "2 items match ~[name=blue], at 1:3, 2:3"
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

    #[test]
    fn resolve_all_returns_every_symbol_at_a_path_or_empty_when_missing() {
        // A path without filters returns every matching symbol in document order, and empty when
        // none matches.
        let first = symbol("/v", 0);
        let second = symbol("/v", 2);
        let symbols = Symbols::from(vec![first.clone(), symbol("/w", 1), second.clone()]);

        assert_eq!(symbols.resolve_all(&path("/v")), Ok(vec![&first, &second]));
        assert_eq!(symbols.resolve_all(&path("/missing")), Ok(vec![]));
    }

    #[test]
    fn resolve_all_with_filter_matching_several_children_returns_symbols_under_each() {
        // A filter that matches several items looks for the rest of the path under each one,
        // skipping items that lack it.
        let first_content = symbol("/items/0/content", 1);
        let third_content = symbol("/items/2/content", 5);
        let symbols = Symbols::from(vec![
            scalar("/items/0/name", 0, "a"),
            first_content.clone(),
            scalar("/items/1/name", 2, "a"),
            scalar("/items/2/name", 4, "a"),
            third_content.clone(),
            scalar("/items/3/name", 6, "b"),
            symbol("/items/3/content", 7),
        ]);

        assert_eq!(
            symbols.resolve_all(&path("/items/~[name=a]/content")),
            Ok(vec![&first_content, &third_content])
        );
        assert_eq!(
            symbols.resolve_all(&path("/items/~[name=c]/content")),
            Ok(vec![])
        );
    }

    #[test]
    fn resolve_all_with_filter_over_items_at_one_path_stays_inside_matched_items() {
        // When siblings share a path, `resolve_all` must look for the rest of the path only inside
        // items that matched the filter, never in a non-matching sibling.
        let matched_content = symbol("/items/item/content", 12);
        let symbols = Symbols::from(vec![
            element("/items/item", 0..10),
            scalar("/items/item/name", 1, "a"),
            element("/items/item", 10..20),
            scalar("/items/item/name", 11, "a"),
            matched_content.clone(),
            element("/items/item", 20..30),
            scalar("/items/item/name", 21, "b"),
            symbol("/items/item/content", 22),
        ]);

        assert_eq!(
            symbols.resolve_all(&path("/items/~[name=a]/content")),
            Ok(vec![&matched_content])
        );
        // Neither `a` item has `extra`; only `b` does.
        let with_extra = Symbols::from(vec![
            element("/items/item", 0..10),
            scalar("/items/item/name", 1, "a"),
            element("/items/item", 10..20),
            scalar("/items/item/name", 11, "a"),
            element("/items/item", 20..30),
            scalar("/items/item/name", 21, "b"),
            symbol("/items/item/extra", 22),
        ]);
        assert_eq!(
            with_extra.resolve_all(&path("/items/~[name=a]/extra")),
            Ok(vec![])
        );
    }

    #[test]
    fn resolve_all_with_two_filters_collects_symbols_from_every_matched_branch() {
        // Each filter expands every matching child before resolving the next filter.
        let first = symbol("/groups/0/items/0/content", 2);
        let second = symbol("/groups/1/items/1/content", 7);
        let symbols = Symbols::from(vec![
            scalar("/groups/0/name", 0, "g"),
            scalar("/groups/0/items/0/name", 1, "x"),
            first.clone(),
            scalar("/groups/1/name", 3, "g"),
            scalar("/groups/1/items/0/name", 4, "y"),
            symbol("/groups/1/items/0/content", 5),
            scalar("/groups/1/items/1/name", 6, "x"),
            second.clone(),
        ]);

        assert_eq!(
            symbols.resolve_all(&path("/groups/~[name=g]/items/~[name=x]/content")),
            Ok(vec![&first, &second])
        );
    }

    #[test]
    fn resolve_all_returns_unreadable_reason_for_unreadable_paths_and_filter_keys() {
        // An unreadable path, a skipped word, an unreadable filter key, or an unreadable target
        // under a matched child each return their reason as an error.
        let (symbols, _) = symbols_with_unreadable_names();
        assert_eq!(
            symbols.resolve_all(&path("/a/c")),
            Err(Unreadable {
                reason: "syntax error in c"
            })
        );
        assert_eq!(
            symbols.resolve_all(&path("/a/b")),
            Err(Unreadable {
                reason: "syntax error near b"
            })
        );

        let (_, items) = items_at_one_path(["red", "red"]);
        let [first, _second] = &items;
        let unreadable_key = with_unreadable(&items, &first.name);
        assert_eq!(
            unreadable_key.resolve_all(&path("/items/~[name=red]/content")),
            Err(Unreadable {
                reason: "can't decode"
            })
        );

        let unreadable_target = with_unreadable(&items, &first.content);
        assert_eq!(
            unreadable_target.resolve_all(&path("/items/~[name=red]/content")),
            Err(Unreadable {
                reason: "can't decode"
            })
        );
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
