//! Turns the top-level definitions of a code file into symbols, with the rules every programming
//! language shares: what a value is, and which names a syntax error makes unreadable.

use super::{ScalarValue, Symbol, Symbols};
use crate::character_column_at;
use crate::language_parsers::node_text;
use crate::symbol_path::SymbolPath;
use std::collections::HashMap;
use std::ops::Range;
use tree_sitter::{Node, Tree};

/// A top-level definition that a language's walk found in a syntax tree.
pub(crate) struct Definition<'tree> {
    /// The identifiers it defines, usually one. Python's `A = B = 5` defines two.
    pub(crate) names: Vec<Node<'tree>>,
    /// The byte range of the whole definition, with its keywords, attributes and decorators, but
    /// without the comments above it.
    pub(crate) range: Range<usize>,
    /// The initializer, or `None` when the definition has none, such as a function or a class.
    pub(crate) value: Option<Node<'tree>>,
}

/// Turns a string literal into its text, following the language's own quoting and escaping
/// rules. Returns `None` for a node that is not a string literal, and for a string that is not
/// text, such as bytes. Their value is kept as written.
///
/// # Errors
/// Returns an error if the string uses an escape that blockwatch can't decode. The error shows
/// the escape.
type StringDecoder = fn(&Node, &str) -> anyhow::Result<Option<String>>;

/// The symbols of `source`, a code file parsed into `tree`, built from the top-level
/// `definitions` that a language's walk found in it.
///
/// Each name of a definition gives a symbol at `/name`, in the order of `definitions`. The
/// symbol's value is the definition's value without the parentheses around it: the text that
/// `decode_string` gives, or else the value as written.
///
/// A name is unreadable, with a reason that shows where the problem is, when:
/// - its definition holds a syntax error, such as a span the parser skipped or a token it
///   assumed was missing;
/// - it appears as a whole word in a span the parser skipped anywhere in the file;
/// - `decode_string` can't decode its value.
///
/// A definition with a problem of its own gives no symbol. Another definition of the same name
/// keeps its symbol, and so does a clean definition whose name a skipped span holds.
// <block name="code-symbol-rules" affects="docs/symbols.md:code-files, src/skill.md#/blockwatch/Symbols/Code files">
pub(crate) fn symbols(
    tree: &Tree,
    source: &str,
    definitions: &[Definition],
    decode_string: StringDecoder,
) -> Symbols {
    let errors = syntax_errors(tree);
    let mut symbols = Vec::new();
    let mut unreadable = HashMap::new();
    for definition in definitions {
        let names = definition.names.iter().map(|name| node_text(name, source));
        match read_value(definition, source, &errors, decode_string) {
            Ok(value) => symbols.extend(names.map(|name| Symbol {
                path: SymbolPath::from_segments(vec![name.to_string()]),
                def_byte_ranges: vec![definition.range.clone()],
                value: value.clone(),
            })),
            Err(problem) => {
                for name in names {
                    unreadable
                        .entry(name.to_string())
                        .or_insert_with(|| problem.reason(name, source));
                }
            }
        }
    }
    // A skipped span can hold a definition that the walk never saw, so every name in it is
    // unreadable, defined or not. A problem in a name's own definition is reported first, as it
    // is the closer one.
    for error in errors.iter().filter(|error| error.is_error()) {
        for name in names_in(node_text(error, source)) {
            unreadable
                .entry(name.to_string())
                .or_insert_with(|| Problem::SyntaxError(*error).reason(name, source));
        }
    }
    Symbols {
        symbols,
        unreadable,
    }
}

/// Why the names of a definition are unreadable.
enum Problem<'tree> {
    /// The syntax error at this node.
    SyntaxError(Node<'tree>),
    /// The string at this node, which the decoder refused with this error.
    Undecodable(Node<'tree>, anyhow::Error),
}

impl Problem<'_> {
    /// The reason to report for `name`, which shows where in `source` the problem is.
    fn reason(&self, name: &str, source: &str) -> String {
        let node = match self {
            Problem::SyntaxError(node) | Problem::Undecodable(node, _) => node,
        };
        // tree-sitter counts the rows, so only the column needs the text, and only of one line.
        let line = node.start_position().row + 1;
        let column = character_column_at(source, node.start_byte());
        match self {
            Problem::SyntaxError(_) => format!(
                "file has a syntax error at line {line}, column {column} near {name}, and the path could resolve to the wrong value"
            ),
            Problem::Undecodable(_, error) => format!(
                "blockwatch can't decode the string at line {line}, column {column}: {error:#}"
            ),
        }
    }
}

/// The value of `definition` in `source`, or `None` when it has none. `errors` are the syntax
/// errors of the file.
///
/// # Errors
/// Returns the problem when one of `errors` belongs to the definition, or when `decode_string`
/// refuses its value.
fn read_value<'tree>(
    definition: &Definition<'tree>,
    source: &str,
    errors: &[Node<'tree>],
    decode_string: StringDecoder,
) -> Result<Option<ScalarValue>, Problem<'tree>> {
    // The parser may have read the definition wrongly around the error, so its value could be
    // another definition's.
    if let Some(error) = errors
        .iter()
        .find(|error| error_in(&definition.range, &error.byte_range()))
    {
        return Err(Problem::SyntaxError(*error));
    }
    let value = match definition.value {
        Some(value) => without_parentheses(value),
        None => return Ok(None),
    };
    let text = match decode_string(&value, source) {
        Ok(Some(text)) => text,
        Ok(None) => node_text(&value, source).to_string(),
        Err(error) => return Err(Problem::Undecodable(value, error)),
    };
    Ok(Some(ScalarValue {
        text,
        byte_range: value.byte_range(),
    }))
}

/// The syntax errors of `tree`, in document order: each span the parser skipped, and each token
/// it assumed was missing. An error inside a skipped span is part of that span.
fn syntax_errors(tree: &Tree) -> Vec<Node<'_>> {
    let root = tree.root_node();
    if !root.has_error() {
        return Vec::new();
    }
    // The walk keeps its own stack, as deeply nested source could overflow the call stack. It
    // enters only the nodes that hold an error.
    let mut errors = Vec::new();
    let mut pending = vec![root];
    let mut cursor = tree.walk();
    while let Some(node) = pending.pop() {
        if node.is_error() || node.is_missing() {
            errors.push(node);
        } else {
            let first = pending.len();
            // A token that the parser skipped is an error node that doesn't hold an error.
            pending.extend(
                node.children(&mut cursor)
                    .filter(|child| child.is_error() || child.has_error()),
            );
            // The stack takes the last child first, so the children go on it in reverse.
            pending[first..].reverse();
        }
    }
    errors
}

/// Whether a syntax error at `error` belongs to the definition at `definition`. Both are byte
/// ranges.
fn error_in(definition: &Range<usize>, error: &Range<usize>) -> bool {
    if error.is_empty() {
        // A missing token takes no space. It can sit right at the end of the definition it
        // belongs to, as a `}` missing at the end of a file does.
        definition.start <= error.start && error.start <= definition.end
    } else {
        error.start < definition.end && definition.start < error.end
    }
}

/// `node` without the parentheses around it, however many there are. In `(1) + (2)`, the
/// parentheses are not around the whole value, so they stay.
fn without_parentheses(mut node: Node<'_>) -> Node<'_> {
    let mut cursor = node.walk();
    // Every grammar with code symbols gives parentheses this node kind.
    while node.kind() == "parenthesized_expression" {
        let only = {
            // A comment inside the parentheses is a child too, but not part of the value.
            let mut inner = node
                .named_children(&mut cursor)
                .filter(|child| !child.is_extra());
            match (inner.next(), inner.next()) {
                (Some(only), None) => only,
                _ => break,
            }
        };
        node = only;
    }
    node
}

/// The words in `text` that could be names: runs of letters, digits and `_` that don't start
/// with a digit.
fn names_in(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|word| word.chars().next().is_some_and(|first| !first.is_numeric()))
}
// </block>

#[cfg(test)]
mod tests {
    // `symbols` reads a real syntax tree, with the spans the parser skipped and the tokens it
    // assumed were missing, and only a grammar can make one. Python stands in for any language:
    // its top-level assignments are the shortest walk, and it has parenthesized values. The walk
    // and the decoder here are only as complete as these tests need.
    use super::*;
    use crate::language_parsers::tree_sitter_parser;
    use anyhow::{Context, bail};
    use std::collections::HashMap;

    /// The symbols of `source`, a Python file, with the definitions of [`python_definitions`].
    fn symbols_of(source: &str) -> anyhow::Result<Symbols> {
        let tree = tree_sitter_parser(&tree_sitter_python::LANGUAGE.into())?
            .parse(source, None)
            .context("failed to parse syntax tree")?;
        let definitions = python_definitions(&tree);
        Ok(symbols(&tree, source, &definitions, decode_plain_string))
    }

    /// The top-level definitions of a Python file that the tests use: assignments to names,
    /// chained ones included, functions and classes.
    fn python_definitions(tree: &Tree) -> Vec<Definition<'_>> {
        let root = tree.root_node();
        let mut cursor = root.walk();
        root.named_children(&mut cursor)
            .filter_map(|statement| match statement.kind() {
                "expression_statement" => assignment(statement),
                "function_definition" | "class_definition" => Some(Definition {
                    names: vec![statement.child_by_field_name("name")?],
                    range: statement.byte_range(),
                    value: None,
                }),
                _ => None,
            })
            .collect()
    }

    /// The definition that `statement` makes when it assigns to names, such as `A = B = 5`.
    fn assignment(statement: Node<'_>) -> Option<Definition<'_>> {
        let mut names = Vec::new();
        let mut value = statement.named_child(0);
        while let Some(assignment) = value.filter(|node| node.kind() == "assignment") {
            names.push(assignment.child_by_field_name("left")?);
            value = assignment.child_by_field_name("right");
        }
        (!names.is_empty()).then_some(Definition {
            names,
            range: statement.byte_range(),
            value,
        })
    }

    /// Decodes a string by removing its quotes, and refuses `\N{...}`, an escape that a decoder
    /// can't read.
    fn decode_plain_string(node: &Node, source: &str) -> anyhow::Result<Option<String>> {
        if node.kind() != "string" {
            return Ok(None);
        }
        let text = &source[node.byte_range()];
        if text.contains(r"\N{") {
            bail!(r"`\N{{...}}` needs the Unicode name table");
        }
        Ok(Some(text[1..text.len() - 1].to_string()))
    }

    /// The path of each symbol, with the text of its definition.
    fn definitions(symbols: &Symbols, source: &str) -> Vec<(String, String)> {
        symbols
            .symbols
            .iter()
            .map(|symbol| {
                let [range] = symbol.def_byte_ranges.as_slice() else {
                    panic!("a code symbol is written in one place");
                };
                (symbol.path.to_string(), source[range.clone()].to_string())
            })
            .collect()
    }

    /// The unreadable names, with their reasons.
    fn unreadable(entries: &[(&str, &str)]) -> HashMap<String, String> {
        entries
            .iter()
            .map(|(name, reason)| (name.to_string(), reason.to_string()))
            .collect()
    }

    #[test]
    fn values_of_every_kind_symbols_returns_them_as_written_or_decoded() -> anyhow::Result<()> {
        let source = r#"NUMBER = 0x1F
TEXT = "plain"
EXPRESSION = 20 * 1024 * 1024
WRAPPED = ((32))
SPLIT = (1) + (2)
COMMENTED = (  # note
    5
)
ANNOTATED: int
def function():
    pass
"#;

        let symbols = symbols_of(source)?;

        // Each value with the text of the source where it is written.
        let values: Vec<(String, Option<(String, &str)>)> = symbols
            .symbols
            .into_iter()
            .map(|symbol| {
                let value = symbol
                    .value
                    .map(|value| (value.text, &source[value.byte_range]));
                (symbol.path.to_string(), value)
            })
            .collect();
        let value = |text: &str, written: &'static str| Some((text.to_string(), written));
        assert_eq!(
            values,
            vec![
                ("/NUMBER".to_string(), value("0x1F", "0x1F")),
                ("/TEXT".to_string(), value("plain", r#""plain""#)),
                (
                    "/EXPRESSION".to_string(),
                    value("20 * 1024 * 1024", "20 * 1024 * 1024")
                ),
                ("/WRAPPED".to_string(), value("32", "32")),
                ("/SPLIT".to_string(), value("(1) + (2)", "(1) + (2)")),
                ("/COMMENTED".to_string(), value("5", "5")),
                ("/ANNOTATED".to_string(), None),
                ("/function".to_string(), None),
            ]
        );
        Ok(())
    }

    #[test]
    fn chained_assignment_symbols_returns_each_name_with_its_range_and_value() -> anyhow::Result<()>
    {
        let source = "A = B = 5\n";

        let symbols = symbols_of(source)?;

        assert_eq!(
            definitions(&symbols, source),
            vec![
                ("/A".to_string(), "A = B = 5".to_string()),
                ("/B".to_string(), "A = B = 5".to_string()),
            ]
        );
        let values: Vec<Option<&str>> = symbols
            .symbols
            .iter()
            .map(|symbol| symbol.value.as_ref().map(|value| value.text.as_str()))
            .collect();
        assert_eq!(values, vec![Some("5"), Some("5")]);
        Ok(())
    }

    #[test]
    fn syntax_error_in_a_definition_symbols_makes_its_name_and_the_words_it_skipped_unreadable()
    -> anyhow::Result<()> {
        // The parser skips `= (1` and `B`, then reads `X = 2`. So `X` would read the value of
        // `B`, and `B` would not be found.
        let source = r#"A = 1
X = (1
B = 2
C = 3
"#;

        let symbols = symbols_of(source)?;

        assert_eq!(
            definitions(&symbols, source),
            vec![
                ("/A".to_string(), "A = 1".to_string()),
                ("/C".to_string(), "C = 3".to_string()),
            ]
        );
        assert_eq!(
            symbols.unreadable,
            unreadable(&[
                (
                    "X",
                    "file has a syntax error at line 2, column 3 near X, and the path could resolve to the wrong value"
                ),
                (
                    "B",
                    "file has a syntax error at line 2, column 3 near B, and the path could resolve to the wrong value"
                ),
            ])
        );
        Ok(())
    }

    #[test]
    fn missing_token_in_a_definition_symbols_makes_its_name_unreadable() -> anyhow::Result<()> {
        // The parser assumes a `)` right after the `(`, and skips nothing.
        let source = r#"def f(:
    pass
Y = 2
"#;

        let symbols = symbols_of(source)?;

        assert_eq!(
            definitions(&symbols, source),
            vec![("/Y".to_string(), "Y = 2".to_string())]
        );
        assert_eq!(
            symbols.unreadable,
            unreadable(&[(
                "f",
                "file has a syntax error at line 1, column 7 near f, and the path could resolve to the wrong value"
            )])
        );
        Ok(())
    }

    #[test]
    fn several_errors_near_a_name_symbols_reports_the_first_in_its_definition() -> anyhow::Result<()>
    {
        // The parser skips the `N` on line 1, and each `for` on line 2.
        let source = r#"print(M N)
N = [1, for, 2, for]
"#;

        let symbols = symbols_of(source)?;

        assert_eq!(
            symbols.unreadable["N"],
            "file has a syntax error at line 2, column 9 near N, and the path could resolve to the wrong value"
        );
        Ok(())
    }

    #[test]
    fn skipped_words_symbols_makes_only_whole_words_unreadable() -> anyhow::Result<()> {
        // The parser skips `AB A_1 Aé A1 1A`. A word that starts with a digit can't be a name.
        let source = r#"A = 1
print(AB A_1 Aé A1 1A x)
"#;

        let symbols = symbols_of(source)?;

        assert_eq!(
            definitions(&symbols, source),
            vec![("/A".to_string(), "A = 1".to_string())]
        );
        let mut names: Vec<&str> = symbols.unreadable.keys().map(String::as_str).collect();
        names.sort();
        assert_eq!(names, vec!["A1", "AB", "A_1", "Aé"]);
        Ok(())
    }

    #[test]
    fn undecodable_string_symbols_makes_its_name_unreadable() -> anyhow::Result<()> {
        let source = r#"A = "\N{DASH}"
B = "plain"
"#;

        let symbols = symbols_of(source)?;

        assert_eq!(
            definitions(&symbols, source),
            vec![("/B".to_string(), r#"B = "plain""#.to_string())]
        );
        assert_eq!(
            symbols.unreadable,
            unreadable(&[(
                "A",
                r"blockwatch can't decode the string at line 1, column 5: `\N{...}` needs the Unicode name table"
            )])
        );
        Ok(())
    }

    #[test]
    fn error_at_the_edge_of_a_definition_error_in_counts_it_only_when_it_is_empty() {
        let definition = 10..20;

        assert!(error_in(&definition, &(20..20)), "missing at the end");
        assert!(error_in(&definition, &(10..10)), "missing at the start");
        assert!(error_in(&definition, &(5..11)), "skipped across the start");
        assert!(!error_in(&definition, &(5..10)), "skipped up to the start");
        assert!(!error_in(&definition, &(20..25)), "skipped from the end");
        assert!(!error_in(&definition, &(21..21)), "missing after the end");
    }
}
