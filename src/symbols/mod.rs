//! Derives the symbols of a file: the elements a symbol reference can address.

pub(crate) mod code;

use crate::blocks::Content;
use crate::symbol_path::{Segment, SymbolPath};
use crate::{LineIndex, Position};
use anyhow::bail;
use std::borrow::Cow;
use std::collections::HashMap;
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

    /// What a reference to the symbol reads, as the content of a block, with `lines` indexing
    /// `source`. `source` must be the text the symbol was derived from.
    ///
    /// A value written as is, such as a number or a Markdown section, keeps the position of each
    /// of its lines. Any other text, such as a value with quotes or escapes, or the text of a
    /// symbol written in several places, is not a part of the source. Each of its lines is at the
    /// start of the symbol.
    ///
    /// # Panics
    /// Panics if a byte range does not fit `source`.
    pub(crate) fn content(&self, source: &str, lines: &LineIndex) -> Content {
        match (&self.value, self.def_byte_ranges.as_slice()) {
            (Some(value), _) if source.get(value.byte_range.clone()) == Some(&value.text) => {
                Content::source(value.byte_range.clone(), lines.range(&value.byte_range))
            }
            (Some(value), _) => {
                Content::decoded(value.text.clone(), lines.range(&value.byte_range))
            }
            (None, [definition]) => Content::source(definition.clone(), lines.range(definition)),
            (None, ranges) => {
                let start = ranges.first().expect("a symbol is written somewhere").start;
                let end = ranges.last().expect("a symbol is written somewhere").end;
                Content::decoded(
                    self.value_or_definition(source).into_owned(),
                    lines.range(&(start..end)),
                )
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
    unreadable: HashMap<SymbolPath, String>,
    /// The words in the text that the parser had to skip, each with the reason to report. That
    /// text could define any of them, so every path with one of them as a segment is unreadable.
    skipped: HashMap<String, String>,
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
    /// Finds the one symbol at `path`.
    ///
    /// A filter in `path`, such as `~[name=blue]`, stands for the one child of the path before it
    /// whose `name` key holds the value `blue`. A child is a list item or a map entry. A string
    /// value matches without its quotes, and any other value as written. When several children
    /// share a path, such as XML elements with one name, the rest of the path looks only inside
    /// the one that the filter picked.
    ///
    /// When no symbol has `path`, the hints are the paths that resemble it: those at least 60%
    /// alike by edit distance, and those that end in the same segment. When `path` has a filter,
    /// they are only paths under the child that the last filter matched, and they keep every
    /// filter as written. When a filter matches no child, the hints are `path` with the filter's
    /// value swapped for a value that exactly one child holds.
    ///
    /// # Errors
    /// Returns [`ResolveError::Unreadable`] when `path` is an unreadable path, or one of its
    /// segments is a word the parser skipped. Returns [`ResolveError::NotFound`] when no symbol
    /// matches `path`, or a filter matches no child. Returns [`ResolveError::AmbiguousSymbol`]
    /// when several symbols match `path`, and [`ResolveError::AmbiguousFilter`] when a filter
    /// matches several children.
    pub(crate) fn resolve(&self, path: &SymbolPath) -> Result<&Symbol, ResolveError<'_>> {
        // A problem in the path's own definition is closer than a skipped word, so it is reported
        // first.
        let unreadable = self.unreadable.get(path).or_else(|| {
            path.segments().iter().find_map(|segment| match segment {
                Segment::Key(name) => self.skipped.get(name),
                // A filter stands for a list item or a map entry, not for a name the file defines.
                Segment::Filter { .. } => None,
            })
        });
        if let Some(reason) = unreadable {
            return Err(ResolveError::Unreadable { reason });
        }
        let (resolved, scope) = resolve_filters(&self.symbols, path)?;
        let candidates: Vec<&Symbol> = scope
            .iter()
            .copied()
            .filter(|symbol| symbol.path == resolved)
            .collect();
        match candidates.len() {
            0 => Err(ResolveError::NotFound {
                hints: path_hints(&scope, path, &resolved),
            }),
            1 => Ok(candidates[0]),
            _ => Err(ResolveError::AmbiguousSymbol { candidates }),
        }
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
    /// `reason` says why.
    Unreadable { reason: &'a str },
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
                let hints: Vec<String> = hints.iter().map(|hint| hint.to_string()).collect();
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
            ResolveError::Unreadable { reason } => reason.to_string(),
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
/// `/items/1` for `/items/~[name=blue]`, and the symbols that the rest of the path can resolve
/// to, in document order. A path without filters comes back unchanged, with every symbol.
///
/// When several children share the path of the one a filter matches, such as XML elements with
/// one name, the symbols are only those inside the child that holds the matched key.
///
/// # Errors
/// Returns [`ResolveError::NotFound`] when a filter matches no child, and
/// [`ResolveError::AmbiguousFilter`] when it matches several. The error is about the first such
/// filter.
fn resolve_filters<'a>(
    symbols: &'a [Symbol],
    path: &SymbolPath,
) -> Result<(SymbolPath, Vec<&'a Symbol>), ResolveError<'a>> {
    // Filters are swapped one at a time, from the left. So a filter looks only under the children
    // that the filters before it matched.
    let mut resolved = path.clone();
    let mut scope: Vec<&Symbol> = symbols.iter().collect();
    for (index, segment) in path.segments().iter().enumerate() {
        let (key, value) = match segment {
            Segment::Key(_) => continue,
            Segment::Filter { key, value } => (key, value),
        };
        let parent = &resolved.segments()[..index];
        let matches: Vec<&Symbol> = child_keys(&scope, parent, key)
            .filter(|symbol| {
                symbol
                    .value
                    .as_ref()
                    .is_some_and(|held| held.text == *value)
            })
            .collect();
        match matches.len() {
            0 => {
                // The second call to `child_keys` walks `scope` again. It happens only when no
                // child matches, so a filter that matches doesn't keep every key it looked at.
                let keys = child_keys(&scope, parent, key);
                return Err(ResolveError::NotFound {
                    hints: filter_hints(path, index, key, keys),
                });
            }
            1 => {
                let key = matches[0];
                resolved = resolved.with_segment(index, key.path.segments()[index].clone());
                let child = &resolved.segments()[..=index];
                let siblings: Vec<&Symbol> = scope
                    .iter()
                    .copied()
                    .filter(|symbol| symbol.path.segments() == child)
                    .collect();
                // A child alone at its path keeps every symbol under that path, even one written
                // outside it, such as a member that a Swift extension adds to a type.
                if siblings.len() > 1 {
                    scope.retain(|symbol| {
                        siblings
                            .iter()
                            .any(|sibling| is_inside(key, sibling) && is_inside(symbol, sibling))
                    });
                }
            }
            _ => {
                return Err(ResolveError::AmbiguousFilter {
                    filter: segment.clone(),
                    keys: matches,
                });
            }
        }
    }
    Ok((resolved, scope))
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

/// The `key` keys of the children of `parent` among `symbols`, in document order. For `parent`
/// `/items` and `key` `name`, they are the symbols at `/items/0/name`, `/items/1/name` and so on.
/// A child is a list item or a map entry.
fn child_keys<'s>(
    symbols: &[&'s Symbol],
    parent: &[Segment],
    key: &str,
) -> impl Iterator<Item = &'s Symbol> {
    let key = Segment::Key(key.to_string());
    // A key deeper inside a child, such as `/items/0/content/name`, describes something inside the
    // child, not the child itself.
    symbols.iter().copied().filter(move |symbol| {
        let segments = symbol.path.segments();
        segments.len() == parent.len() + 2
            && segments.starts_with(parent)
            && segments[parent.len() + 1] == key
    })
}

/// The paths to suggest when no symbol has `resolved`, which is `path` with its filters swapped
/// for the children they matched. The most alike come first.
///
/// When `path` has a filter, the hints are only paths under the child that the last filter
/// matched, written with every filter of `path`. Otherwise they come from all of `symbols`.
fn path_hints(symbols: &[&Symbol], path: &SymbolPath, resolved: &SymbolPath) -> Vec<SymbolPath> {
    let last_filter = path.segments().iter().rposition(|segment| match segment {
        Segment::Key(_) => false,
        Segment::Filter { .. } => true,
    });
    match last_filter {
        None => hints(
            symbols.iter().map(|symbol| &symbol.path),
            path,
            MIN_HINT_SIMILARITY,
        ),
        Some(last) => {
            // A path under another child points at an item that the filters did not pick. A
            // position in place of a filter breaks when an item is added above it.
            let matched = &resolved.segments()[..=last];
            let candidates: Vec<SymbolPath> = symbols
                .iter()
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
/// `path` with the filter's value swapped for each value that exactly one of `keys` holds. The
/// most alike come first.
fn filter_hints<'s>(
    path: &SymbolPath,
    index: usize,
    key: &str,
    keys: impl Iterator<Item = &'s Symbol>,
) -> Vec<SymbolPath> {
    let values: Vec<&str> = keys
        // A key that holds a list or a map has no value to put in a filter.
        .filter_map(|symbol| symbol.value.as_ref())
        .map(|held| held.text.as_str())
        .collect();
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for value in &values {
        *counts.entry(value).or_default() += 1;
    }
    // A value that several children hold would make the hint match several children too. The
    // hints start from `path`, so the filters before `index` stay as the reference wrote them.
    let alternatives: Vec<SymbolPath> = values
        .iter()
        .filter(|value| counts[*value] == 1)
        .map(|value| {
            let filter = Segment::Filter {
                key: key.to_string(),
                value: value.to_string(),
            };
            path.clone().with_segment(index, filter)
        })
        .collect();
    // Each alternative points at a real child and differs from `path` only in the filter's value,
    // so a similarity cutoff would only drop real choices.
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
            let position = LineIndex::new(source).position(error.start_byte());
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

    /// The symbols of the list `/items`, with one item for each of `names`. Each item is a map
    /// with two keys: `name`, which holds one of `names`, and `content`, which holds a list or a
    /// map.
    fn named_items(names: &[&str]) -> Symbols {
        names
            .iter()
            .enumerate()
            .flat_map(|(index, name)| {
                [
                    scalar(&format!("/items/{index}/name"), 2 * index, name),
                    symbol(&format!("/items/{index}/content"), 2 * index + 1),
                ]
            })
            .collect::<Vec<Symbol>>()
            .into()
    }

    #[test]
    fn path_of_one_symbol_returns_that_symbol() {
        let symbols = Symbols::from(vec![symbol("/a/b", 0), symbol("/b", 1)]);

        assert_eq!(symbols.resolve(&path("/b")), Ok(&symbols.symbols[1]));
    }

    #[test]
    fn path_of_several_symbols_returns_every_one_as_ambiguous() {
        let symbols = Symbols::from(vec![symbol("/v", 0), symbol("/w", 1), symbol("/v", 2)]);

        assert_eq!(
            symbols.resolve(&path("/v")),
            Err(ResolveError::AmbiguousSymbol {
                candidates: vec![&symbols.symbols[0], &symbols.symbols[2]]
            })
        );
    }

    /// The symbols `/a`, `/a/b`, `/a/c`, `/b` and `/bc`, with `/a/c` unreadable and `b` skipped.
    fn symbols_with_unreadable_names() -> Symbols {
        Symbols {
            symbols: vec![
                symbol("/a", 0),
                symbol("/a/b", 1),
                symbol("/a/c", 2),
                symbol("/b", 3),
                symbol("/bc", 4),
            ],
            unreadable: HashMap::from([(path("/a/c"), "syntax error in c".to_string())]),
            skipped: HashMap::from([("b".to_string(), "syntax error near b".to_string())]),
        }
    }

    #[test]
    fn unreadable_path_returns_its_reason_even_when_a_symbol_has_it() {
        let symbols = symbols_with_unreadable_names();

        let error = symbols.resolve(&path("/a/c")).unwrap_err();

        assert_eq!(error.reason(""), "syntax error in c");
    }

    #[test]
    fn path_around_an_unreadable_path_resolves() {
        let symbols = symbols_with_unreadable_names();

        assert_eq!(symbols.resolve(&path("/a")), Ok(&symbols.symbols[0]));
    }

    #[test]
    fn skipped_word_returns_its_reason_even_when_a_symbol_has_it() {
        let symbols = symbols_with_unreadable_names();

        let error = symbols.resolve(&path("/b")).unwrap_err();

        assert_eq!(error.reason(""), "syntax error near b");
    }

    #[test]
    fn path_under_a_skipped_word_returns_its_reason() {
        let symbols = symbols_with_unreadable_names();

        assert_eq!(
            symbols.resolve(&path("/b/c")),
            Err(ResolveError::Unreadable {
                reason: "syntax error near b"
            })
        );
    }

    #[test]
    fn path_that_ends_in_a_skipped_word_returns_its_reason() {
        let symbols = symbols_with_unreadable_names();

        assert_eq!(
            symbols.resolve(&path("/a/b")),
            Err(ResolveError::Unreadable {
                reason: "syntax error near b"
            })
        );
    }

    #[test]
    fn name_that_starts_with_a_skipped_word_resolves() {
        let symbols = symbols_with_unreadable_names();

        assert_eq!(symbols.resolve(&path("/bc")), Ok(&symbols.symbols[4]));
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
                hints: vec![symbols.symbols[1].path.clone()]
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
                hints: vec![symbols.symbols[0].path.clone()]
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
                hints: vec![symbols.symbols[0].path.clone()]
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
                hints: vec![
                    symbols.symbols[2].path.clone(),
                    symbols.symbols[3].path.clone(),
                    symbols.symbols[1].path.clone(),
                ]
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
                hints: vec![symbols.symbols[0].path.clone()]
            })
        );
    }

    #[test]
    fn filter_matching_one_child_returns_the_symbol_under_it() {
        let symbols = named_items(&["red", "blue"]);

        assert_eq!(
            symbols.resolve(&path("/items/~[name=blue]/content")),
            Ok(&symbols.symbols[3])
        );
    }

    #[test]
    fn filter_matching_several_children_returns_their_keys_as_ambiguous() {
        let symbols = named_items(&["blue", "red", "blue"]);

        assert_eq!(
            symbols.resolve(&path("/items/~[name=blue]/content")),
            Err(ResolveError::AmbiguousFilter {
                filter: path("/~[name=blue]").segments()[0].clone(),
                keys: vec![&symbols.symbols[0], &symbols.symbols[4]]
            })
        );
    }

    #[test]
    fn filter_matching_no_child_returns_the_other_values_as_hints() {
        // `blues` is more alike to `blue` than `red` is, so it comes first.
        let symbols = named_items(&["red", "blues"]);

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
        let symbols = named_items(&["blues", "red", "blues"]);

        assert_eq!(
            symbols.resolve(&path("/items/~[name=blue]/content")),
            Err(ResolveError::NotFound {
                hints: vec![path("/items/~[name=red]/content")]
            })
        );
    }

    #[test]
    fn filter_at_the_end_matching_no_child_returns_hints_below_the_similarity_floor() {
        // `/~[name=crimson]` is 56% alike to `/~[name=blue]`, below the 60% that other hints need.
        let symbols = Symbols::from(vec![
            scalar("/0/name", 0, "red"),
            scalar("/1/name", 1, "crimson"),
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
        let symbols = Symbols::from(vec![
            scalar("/groups/0/name", 0, "first"),
            scalar("/groups/0/items/0/name", 1, "blue"),
            symbol("/groups/0/items/0/content", 2),
            scalar("/groups/1/name", 3, "second"),
            scalar("/groups/1/items/0/name", 4, "blue"),
            symbol("/groups/1/items/0/content", 5),
        ]);

        assert_eq!(
            symbols.resolve(&path("/groups/~[name=second]/items/~[name=blue]/content")),
            Ok(&symbols.symbols[5])
        );
    }

    #[test]
    fn filter_at_the_root_selects_a_list_item() {
        // The file is a list, so no key comes before the filter.
        let symbols = Symbols::from(vec![
            symbol("/0", 0),
            scalar("/0/name", 1, "red"),
            symbol("/1", 2),
            scalar("/1/name", 3, "blue"),
        ]);

        assert_eq!(
            symbols.resolve(&path("/~[name=blue]")),
            Ok(&symbols.symbols[2])
        );
    }

    #[test]
    fn filter_selects_an_entry_of_a_map() {
        // `b` is a key of a map, not a position in a list.
        let symbols = Symbols::from(vec![
            symbol("/items/a", 0),
            scalar("/items/a/name", 1, "red"),
            symbol("/items/b", 2),
            scalar("/items/b/name", 3, "blue"),
        ]);

        assert_eq!(
            symbols.resolve(&path("/items/~[name=blue]")),
            Ok(&symbols.symbols[2])
        );
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
        let symbols = named_items(&["blue", "red"]);

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

    /// The symbols of `/items`, with one item for each of `names`, all at the path `/items/item`,
    /// as XML gives sibling elements with one name. Each item holds two keys: `name`, which holds
    /// one of `names`, and `content`.
    fn items_at_one_path(names: &[&str]) -> Symbols {
        names
            .iter()
            .enumerate()
            .flat_map(|(index, name)| {
                let start = 10 * index;
                [
                    element("/items/item", start..start + 10),
                    scalar("/items/item/name", start + 1, name),
                    symbol("/items/item/content", start + 2),
                ]
            })
            .collect::<Vec<Symbol>>()
            .into()
    }

    #[test]
    fn filter_over_items_at_one_path_returns_the_symbol_under_the_picked_item() {
        let symbols = items_at_one_path(&["red", "blue"]);

        assert_eq!(
            symbols.resolve(&path("/items/~[name=blue]/content")),
            Ok(&symbols.symbols[5])
        );
    }

    #[test]
    fn filter_at_the_end_over_items_at_one_path_returns_the_picked_item() {
        // `blue` starts right where `red` ends.
        let symbols = items_at_one_path(&["red", "blue"]);

        assert_eq!(
            symbols.resolve(&path("/items/~[name=red]")),
            Ok(&symbols.symbols[0])
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

    /// The symbols of `/groups`, with one group for each of `groups`, all at the path
    /// `/groups/group`. Each group holds a `name`, which holds the first value of its pair, and a
    /// list `items` with one item. That item holds a `name`, which holds the second value, and
    /// `content`.
    fn groups_at_one_path(groups: &[(&str, &str)]) -> Symbols {
        groups
            .iter()
            .enumerate()
            .flat_map(|(index, (group_name, item_name))| {
                let start = 10 * index;
                [
                    element("/groups/group", start..start + 10),
                    scalar("/groups/group/name", start + 1, group_name),
                    element("/groups/group/items", start + 2..start + 10),
                    element("/groups/group/items/item", start + 3..start + 10),
                    scalar("/groups/group/items/item/name", start + 4, item_name),
                    symbol("/groups/group/items/item/content", start + 5),
                ]
            })
            .collect::<Vec<Symbol>>()
            .into()
    }

    #[test]
    fn second_filter_over_groups_at_one_path_looks_only_inside_the_first_pick() {
        let symbols = groups_at_one_path(&[("first", "blue"), ("second", "blue")]);

        assert_eq!(
            symbols.resolve(&path("/groups/~[name=second]/items/~[name=blue]/content")),
            Ok(&symbols.symbols[11])
        );
    }

    #[test]
    fn filter_matching_under_two_groups_at_one_path_returns_their_keys_as_ambiguous() {
        let symbols = groups_at_one_path(&[("first", "blue"), ("second", "blue")]);

        assert_eq!(
            symbols.resolve(&path("/groups/group/items/~[name=blue]/content")),
            Err(ResolveError::AmbiguousFilter {
                filter: path("/~[name=blue]").segments()[0].clone(),
                keys: vec![&symbols.symbols[4], &symbols.symbols[10]]
            })
        );
    }

    #[test]
    fn second_filter_matching_no_child_returns_hints_only_from_the_first_pick() {
        let symbols = groups_at_one_path(&[("first", "blue"), ("second", "red")]);

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
        let symbols = Symbols::from(vec![
            element("/Config", 0..10),
            scalar("/Config/name", 1, "blue"),
            symbol("/Config/extra", 20),
        ]);

        assert_eq!(
            symbols.resolve(&path("/~[name=blue]/extra")),
            Ok(&symbols.symbols[2])
        );
    }

    #[test]
    fn duplicate_keys_on_one_line_reason_lists_each_line_and_column() {
        let source = r#"{"v": 1, "v": 2}"#;
        let symbols = Symbols::from(vec![symbol("/v", 1), symbol("/v", 9)]);
        let error = ResolveError::AmbiguousSymbol {
            candidates: vec![&symbols.symbols[0], &symbols.symbols[1]],
        };

        assert_eq!(
            error.reason(source),
            "ambiguous symbol, defined at 1:2, 1:10"
        );
    }

    #[test]
    fn ambiguous_filter_reason_counts_the_children_and_lists_where_each_key_is() {
        let source = "- name: blue\n- name: blue\n";
        let keys = [symbol("/0/name", 2), symbol("/1/name", 15)];
        let error = ResolveError::AmbiguousFilter {
            filter: path("/~[name=blue]").segments()[0].clone(),
            keys: vec![&keys[0], &keys[1]],
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
    fn symbol_with_several_ranges_content_puts_each_line_at_the_first_range() {
        let (source, symbol) = symbol_written_twice();

        let content = symbol.content(source, &LineIndex::new(source));

        assert_eq!(content.text(source), "first\nsecond\n");
        assert_eq!(content.position(1, 0), Position::new(1, 1));
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

#[cfg(test)]
mod syntax_error_tests {
    // Any grammar shows how error recovery shapes a tree. JSON's has the shortest sources.
    use super::*;
    use crate::language_parsers::tree_sitter_parser;
    use anyhow::Context;

    /// What [`ensure_no_syntax_error`] gives for `source`, a JSON file.
    fn ensure_no_syntax_error_in_json(source: &str) -> anyhow::Result<()> {
        let tree = tree_sitter_parser(&tree_sitter_json::LANGUAGE.into())?
            .parse(source, None)
            .context("failed to parse syntax tree")?;
        ensure_no_syntax_error(&tree, source)
    }

    #[test]
    fn missing_token_returns_syntax_error_where_it_belongs() {
        // The value of "b" is missing, so error recovery assumes one right after the colon.
        let err = ensure_no_syntax_error_in_json(r#"{"a": 1, "b": , "c": 3}"#).unwrap_err();
        assert!(
            err.to_string()
                .contains("syntax error at line 1, column 14,"),
            "unexpected error message: {err}"
        );
    }

    #[test]
    fn unexpected_token_returns_syntax_error_at_the_token() {
        // The value of "a" lacks its closing quote. It ends at the quote that should open the next
        // key, which leaves the `b` after it unexpected.
        let err = ensure_no_syntax_error_in_json(r#"{"a": "1, "b": "2"}"#).unwrap_err();
        assert!(
            err.to_string()
                .contains("syntax error at line 1, column 12,"),
            "unexpected error message: {err}"
        );
    }

    #[test]
    fn unparsable_span_returns_syntax_error_at_its_start() {
        // A trailing comma, which JSONC allows and JSON does not, is left over as a span that does
        // not parse.
        let err = ensure_no_syntax_error_in_json(r#"{"a": 1,}"#).unwrap_err();
        assert!(
            err.to_string()
                .contains("syntax error at line 1, column 8,"),
            "unexpected error message: {err}"
        );
    }

    #[test]
    fn source_with_several_syntax_errors_returns_the_first() {
        let source = r#"{
  "a": 1 "b": 2,
  "c": ,
}"#;
        let err = ensure_no_syntax_error_in_json(source).unwrap_err();
        assert!(
            err.to_string()
                .contains("syntax error at line 2, column 3,"),
            "unexpected error message: {err}"
        );
    }
}
