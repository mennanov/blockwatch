//! Helpers for the tree-sitter grammars: a parser for a grammar, walks over a tree, and the check
//! for a syntax error.

use crate::position::LineIndex;
use anyhow::bail;
use tree_sitter::{Node, Parser, Tree};

/// Creates a Tree-sitter [`Parser`] configured for `language`.
///
/// # Panics
/// Panics if the tree-sitter crate doesn't support the grammar's ABI version.
pub(super) fn tree_sitter_parser(language: &tree_sitter::Language) -> Parser {
    let mut parser = Parser::new();
    // The grammars are compiled into the binary, so this fails on every run or on none. Each
    // grammar is checked on its own, and every test that calls `LanguageParsers::new` loads them
    // all, so a bad grammar fails the tests before a release.
    if let Err(error) = parser.set_language(language) {
        panic!(
            "the tree-sitter crate can't load the {} grammar: {error}",
            language.name().unwrap_or("unnamed")
        );
    }
    parser
}

/// The text of `node` in `source`, the text that tree-sitter parsed.
pub(super) fn node_text<'source>(node: &Node, source: &'source str) -> &'source str {
    #[expect(
        clippy::string_slice,
        reason = "tree-sitter nodes start and end on whole characters"
    )]
    &source[node.byte_range()]
}

/// The named children of `node`, without comments, in document order.
pub(super) fn members(node: Node) -> Vec<Node> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|child| !child.is_extra())
        .collect()
}

/// Calls `visit` on `root` and then on the nodes below it, in document order. `visit` returns
/// whether to visit the children of the node it gets.
pub(super) fn walk_tree<'tree>(root: Node<'tree>, mut visit: impl FnMut(Node<'tree>) -> bool) {
    // A cursor moves through the tree without a stack of its own, so deeply nested source can't
    // overflow the call stack, and the walk allocates nothing for each node. It never leaves the
    // node it starts at.
    let mut cursor = root.walk();
    loop {
        if visit(cursor.node()) && cursor.goto_first_child() {
            continue;
        }
        // The next node is the next sibling of this node, or of the nearest parent with one.
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                return;
            }
        }
    }
}

/// Refuses `tree`, parsed from `source`, if it has a syntax error.
///
/// # Errors
/// Returns an error that shows the line and column of the first syntax error.
pub(super) fn ensure_no_syntax_error(tree: &Tree, source: &str) -> anyhow::Result<()> {
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
fn first_syntax_error(tree: &Tree) -> Option<Node<'_>> {
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
mod syntax_error_tests {
    // Any grammar shows how error recovery shapes a tree. JSON's has the shortest sources.
    use super::*;
    use anyhow::Context;

    /// What [`ensure_no_syntax_error`] gives for `source`, a JSON file.
    fn ensure_no_syntax_error_in_json(source: &str) -> anyhow::Result<()> {
        let tree = tree_sitter_parser(&tree_sitter_json::LANGUAGE.into())
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
