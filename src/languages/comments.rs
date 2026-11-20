//! Finds the comments in a file, and the comment styles that several languages share.

use super::{node_text, tree_sitter_parser};
use crate::position::{Position, character_column_at};
use std::ops::Range;
use tree_sitter::{Node, Parser, Tree, TreeCursor};

/// Parses comment string from a source code by returning an iterator of `Comment`s.
pub(super) trait CommentsParser: Send + Sync {
    /// Returns an iterator of `Comment`s from the source code.
    fn parse<'source>(
        &'source mut self,
        source_code: &'source str,
    ) -> impl Iterator<Item = Comment> + 'source;
}

/// What the comment walk does at a node.
pub(super) enum Visit {
    /// Recurse into the node's children; the node yields no comment.
    Continue,
    /// Stop at this node, not descending into its children.
    /// If the node is a comment, then it must contain `Some(Comment)`.
    Break(Option<Comment>),
}

/// Maps a node to a [`Visit`]: the one place a grammar's node kinds are interpreted, one per language.
pub(super) type NodeVisitor = Box<dyn Fn(&Node, &str) -> Visit + Send + Sync>;

pub(super) struct TreeSitterCommentsParser {
    parser: Parser,
    node_visitor: NodeVisitor,
    tree: Option<Tree>,
}

impl TreeSitterCommentsParser {
    pub(super) fn new(language: &tree_sitter::Language, node_visitor: NodeVisitor) -> Self {
        let parser = tree_sitter_parser(language);
        Self {
            parser,
            node_visitor,
            tree: None,
        }
    }

    /// Stops the walk at nodes of `kinds` without reading a comment out of them.
    pub(super) fn with_break_at_node_kinds(self, kinds: &'static [&'static str]) -> Self {
        let Self {
            parser,
            node_visitor,
            tree,
        } = self;
        Self {
            parser,
            node_visitor: Box::new(move |node, source_code| {
                if kinds.contains(&node.kind()) {
                    return Visit::Break(None);
                }
                node_visitor(node, source_code)
            }),
            tree,
        }
    }

    /// Stops the walk at every node the `predicate` accepts, without reading a comment out of it.
    ///
    /// Unlike [`Self::with_break_at_node_kinds`], the predicate also sees the source code, so it
    /// can judge a node by the surrounding text.
    pub(super) fn with_break_when(self, predicate: fn(&Node, &str) -> bool) -> Self {
        let Self {
            parser,
            node_visitor,
            tree,
        } = self;
        Self {
            parser,
            node_visitor: Box::new(move |node, source_code| {
                if predicate(node, source_code) {
                    return Visit::Break(None);
                }
                node_visitor(node, source_code)
            }),
            tree,
        }
    }
}

impl CommentsParser for TreeSitterCommentsParser {
    fn parse<'a>(&'a mut self, source_code: &'a str) -> impl Iterator<Item = Comment> + 'a {
        let tree = self.parser.parse(source_code, None).unwrap();
        self.tree = Some(tree);
        // It is safe to unwrap here because we just set self.tree
        Comments::new(self.tree.as_ref().unwrap(), &self.node_visitor, source_code)
    }
}

/// Builds a [`NodeVisitor`] from `comment_text`, a function that returns a node's normalized
/// comment text when the node is a comment, or `None` otherwise. The visitor turns that answer
/// into a [`Visit`], one node at a time:
///
/// - not a comment: [`Visit::Continue`] — keep walking into the node's children;
/// - a comment: [`Visit::Break`] carrying the comment — its children are not visited, because a
///   comment's own text already covers any comment nested inside it.
pub(super) fn comment_visitor<F>(comment_text: F) -> NodeVisitor
where
    F: Fn(&Node, &str) -> Option<String> + Send + Sync + 'static,
{
    Box::new(
        move |node, source_code| match comment_text(node, source_code) {
            None => Visit::Continue,
            Some(text) => Visit::Break(Some(Comment::from_node(node, source_code, text))),
        },
    )
}

struct Comments<'source> {
    cursor: TreeCursor<'source>,
    node_visitor: &'source NodeVisitor,
    source_code: &'source str,
    /// Whether the root node has been visited yet.
    start_visited: bool,
    /// Whether the next advance must skip the current node's children (set on [`Visit::Break`]).
    skip_children: bool,
    /// Whether the walk has run out of nodes. Once set, the iterator yields `None` forever: the
    /// cursor comes to rest on the root, so advancing it again would walk the whole tree a second
    /// time and re-emit every comment.
    done: bool,
}

impl<'source> Comments<'source> {
    fn new(
        tree: &'source Tree,
        node_visitor: &'source NodeVisitor,
        source_code: &'source str,
    ) -> Self {
        let cursor = tree.walk();
        Self {
            cursor,
            node_visitor,
            source_code,
            start_visited: false,
            skip_children: false,
            done: false,
        }
    }

    /// Advances to the next node in pre-order, skipping the current node's children when `descend`
    /// is false. Returns `false` once the tree is exhausted.
    fn goto_next(&mut self, descend: bool) -> bool {
        if descend && self.cursor.goto_first_child() {
            return true;
        }
        loop {
            if self.cursor.goto_next_sibling() {
                return true;
            }
            if !self.cursor.goto_parent() {
                return false;
            }
        }
    }
}

impl<'source> Iterator for Comments<'source> {
    type Item = Comment;

    /// Yields comments in pre-order, and keeps yielding `None` once the tree is exhausted.
    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        loop {
            if !self.start_visited {
                self.start_visited = true;
            } else {
                let descend = !self.skip_children;
                if !self.goto_next(descend) {
                    self.done = true;
                    return None;
                }
            }
            self.skip_children = false;
            match (self.node_visitor)(&self.cursor.node(), self.source_code) {
                Visit::Continue => {}
                Visit::Break(comment) => {
                    self.skip_children = true;
                    if let Some(comment) = comment {
                        return Some(comment);
                    }
                }
            }
        }
    }
}

#[derive(Debug, PartialEq)]
/// A single comment extracted from a source file — the only place block tags may appear.
pub(crate) struct Comment {
    /// Position range of the comment in the source.
    pub(crate) position_range: Range<Position>,
    /// Byte offset (i.e. position) of the comment in the source.
    pub(crate) source_range: Range<usize>,
    /// The `comment_string` is expected to be the content of the comment with all language specific
    /// comment symbols like `//`, `/**`, `#`, etc replaced with the corresponding number of
    /// whitespaces ("  " for "//", "   " for `/**`, etc.) so that the length of the comment is
    /// preserved. Offsets into this text therefore map straight back onto the source file.
    pub(crate) comment_text: String,
}

impl Comment {
    /// Builds a [`Comment`] spanning `node`, converting tree-sitter's 0-based rows and byte columns
    /// to the 1-based line/character positions used throughout.
    pub(super) fn from_node(node: &Node, source_code: &str, comment_text: String) -> Self {
        Self {
            position_range: Position::new(
                node.start_position().row + 1,
                character_column_at(source_code, node.start_byte()),
            )
                ..Position::new(
                    node.end_position().row + 1,
                    character_column_at(source_code, node.end_byte()),
                ),
            source_range: node.start_byte()..node.end_byte(),
            comment_text,
        }
    }

    /// Maps `offset`, a byte offset within `comment_text`, onto its position in the source file.
    ///
    /// Columns count characters, so the offset is converted by counting the characters it skips
    /// rather than the bytes: the two differ on any line holding a multi-byte character.
    pub(crate) fn position_at(&self, offset: usize) -> Position {
        #[expect(
            clippy::string_slice,
            reason = "a tag starts at a `<` and ends after a `>`"
        )]
        let text_before = &self.comment_text[..offset];
        match text_before.rfind('\n') {
            // Past the comment's first line, so the column is measured from that line's start.
            #[expect(clippy::string_slice, reason = "`line_break` is the index of a `\\n`")]
            Some(line_break) => Position::new(
                self.position_range.start.line + text_before.matches('\n').count(),
                text_before[line_break + 1..].chars().count() + 1,
            ),
            // Still on the comment's first line, which starts at the comment's own column.
            None => Position::new(
                self.position_range.start.line,
                self.position_range.start.character + text_before.chars().count(),
            ),
        }
    }

    /// Sets the columns from where the comment's bytes fall in `source`.
    pub(super) fn set_character_columns(&mut self, source: &str) {
        self.position_range.start.character = character_column_at(source, self.source_range.start);
        self.position_range.end.character = character_column_at(source, self.source_range.end);
    }

    /// Shifts a comment that was parsed from a region of `source` into the coordinates of the
    /// full `source`. The region starts at byte `region_start_byte`, on the 0-based row
    /// `region_start_row`.
    pub(super) fn shift_into_source(
        &mut self,
        region_start_row: usize,
        region_start_byte: usize,
        source: &str,
    ) {
        self.position_range.start.line += region_start_row;
        self.position_range.end.line += region_start_row;
        self.source_range.start += region_start_byte;
        self.source_range.end += region_start_byte;
        // A region can start mid-line, so the columns from the region parse are not source columns.
        self.set_character_columns(source);
    }
}

/// Blanks every byte to a space, keeping line breaks (`\n`, `\r`) so that row and column offsets
/// stay aligned with the original bytes.
pub(super) fn blank_preserving_line_breaks(bytes: &mut [u8]) {
    for byte in bytes {
        if *byte != b'\n' && *byte != b'\r' {
            *byte = b' ';
        }
    }
}

/// `comment` with each byte outside `content`, a byte range of it, turned into a space, so it
/// keeps its length. The bytes outside `content` must be ASCII, such as the markers of a comment.
pub(super) fn blank_outside(comment: &str, content: Range<usize>) -> String {
    #[expect(clippy::string_slice, reason = "the bytes around `content` are ASCII")]
    let text = &comment[content.clone()];
    format!(
        "{}{text}{}",
        " ".repeat(content.start),
        " ".repeat(comment.len() - content.end)
    )
}

/// `comment`, a line comment, with its marker blanked, so it keeps its length. Returns `None` when
/// `comment` starts with none of `markers`.
///
/// A marker in `markers` is a run of one character, such as `//`, `#` or `--`, with an optional
/// suffix, such as the `!` of `//!` or the `:` of `#:`. The first marker that `comment` starts with
/// is blanked. Its run takes every copy of its character that follows, so `//` blanks all of
/// `////`. A suffix that ends with `<` counts only when white space or the end of the comment
/// follows it, so the `<` of `//<` stays in `//<block name="x">`.
pub(super) fn blank_line_marker(comment: &str, markers: &[&str]) -> Option<String> {
    markers.iter().find_map(|marker| {
        let character = marker.chars().next()?;
        let suffix = marker.trim_start_matches(character);
        let after_run = comment.trim_start_matches(character);
        let run_length = comment.len() - after_run.len();
        if run_length < marker.len() - suffix.len() {
            return None;
        }
        let rest = after_run.strip_prefix(suffix)?;
        // Without white space, the `<` can start the text, such as a tag.
        if suffix.ends_with('<') && !rest.chars().next().is_none_or(char::is_whitespace) {
            return None;
        }
        let marker_length = comment.len() - rest.len();
        Some(format!("{}{rest}", " ".repeat(marker_length)))
    })
}

/// The line comment markers of most languages with `//` comments. `//!` starts a doc comment in
/// some of them, such as Rust, and blanking it in the others keeps tags wrapped over `//!` lines
/// working there too.
pub(super) const SLASH_MARKERS: &[&str] = &["//!", "//"];

/// The line comment markers of C and C++. Doxygen documents the member before a comment that
/// starts with `///<` or `//!<`.
pub(super) const DOXYGEN_MARKERS: &[&str] = &["//!<", "//<", "//!", "//"];

/// The line comment markers of a language with both `//` and `#` line comments.
pub(super) const SLASH_AND_HASH_MARKERS: &[&str] = &["//!", "//", "#"];

/// A comments parser for a language whose comments are line comments that start with one of
/// `line_markers` (see [`blank_line_marker`]), or `/* */` comments. A node is a comment when its
/// kind is in `comment_node_kinds`.
pub(super) fn c_style_comments_parser(
    language: &tree_sitter::Language,
    comment_node_kinds: &'static [&'static str],
    line_markers: &'static [&'static str],
) -> TreeSitterCommentsParser {
    TreeSitterCommentsParser::new(
        language,
        comment_visitor(move |node, source_code| {
            comment_node_kinds
                .contains(&node.kind())
                .then(|| c_style_comment_text(node_text(node, source_code), line_markers))
        }),
    )
}

/// `comment`, a line comment that starts with one of `line_markers`, or a `/* */` comment, with its
/// markers blanked, so it keeps its length. Any other comment is returned as it is.
pub(super) fn c_style_comment_text(comment: &str, line_markers: &[&str]) -> String {
    match blank_line_marker(comment, line_markers) {
        Some(text) => text,
        None if comment.starts_with("/*") => c_style_multiline_comment_processor(comment),
        // A grammar can read other text as a comment, such as a shebang in Scala.
        None => comment.to_string(),
    }
}

/// Comments parser shared by the ECMAScript-family grammars (JavaScript, TypeScript, TSX). All
/// three use the same node kinds for comments, so they differ only in which grammar is loaded.
pub(super) fn ecmascript_comments_parser(
    language: &tree_sitter::Language,
) -> TreeSitterCommentsParser {
    TreeSitterCommentsParser::new(language, comment_visitor(ecmascript_comment_text))
}

/// Returns the text of `node` with its comment markers blanked, preserving its length, or `None`
/// if `node` is not a comment. A comment is one of:
///
/// - a `//` or `/* */` comment;
/// - a legacy HTML-like comment (Annex B of the ECMAScript spec): a single line starting with
///   `<!--` or `-->`;
/// - a JSX `{...}` that holds nothing but a comment, such as `{/* text */}`.
fn ecmascript_comment_text(node: &Node, source_code: &str) -> Option<String> {
    let text = node_text(node, source_code);
    match node.kind() {
        "comment" => Some(c_style_comment_text(text, SLASH_MARKERS)),
        "html_comment" => Some(if text.starts_with("<!--") {
            text.replacen("<!--", "    ", 1)
        } else {
            text.replacen("-->", "   ", 1)
        }),
        "jsx_expression" => jsx_comment_text(node, source_code),
        _ => None,
    }
}

/// The text of `expression`, a JSX `{...}`, with its braces and comment markers blanked, when it
/// holds nothing but a comment, such as `{/* text */}`. Returns `None` otherwise.
fn jsx_comment_text(expression: &Node, source_code: &str) -> Option<String> {
    // The braces join the comment, so that a block's content starts after the `}` of its start
    // tag and ends before the `{` of its end tag.
    let child = expression.named_child(0)?;
    #[expect(
        clippy::string_slice,
        reason = "tree-sitter nodes start and end on whole characters"
    )]
    let before = &source_code[expression.start_byte()..child.start_byte()];
    #[expect(
        clippy::string_slice,
        reason = "tree-sitter nodes start and end on whole characters"
    )]
    let after = &source_code[child.end_byte()..expression.end_byte()];
    // Code or a second comment after the first child keeps the braces out. So does a `}` that
    // tree-sitter made up to recover from an error, because it takes no bytes.
    if after.trim() != "}" {
        return None;
    }
    Some(format!(
        "{}{}{}",
        before.replace('{', " "),
        ecmascript_comment_text(&child, source_code)?,
        after.replace('}', " ")
    ))
}

/// A comments parser for a language whose comments are line comments that start with one of
/// `line_markers` (see [`blank_line_marker`]). A node is a comment when its kind is
/// `comment_node_kind`.
pub(super) fn python_style_comments_parser(
    language: &tree_sitter::Language,
    comment_node_kind: &'static str,
    line_markers: &'static [&'static str],
) -> TreeSitterCommentsParser {
    TreeSitterCommentsParser::new(
        language,
        comment_visitor(move |node, source_code| {
            if node.kind() != comment_node_kind {
                return None;
            }
            let comment = node_text(node, source_code);
            // A comment form without a marker is kept intact, rather than blanking a `#` that
            // belongs to its content.
            Some(blank_line_marker(comment, line_markers).unwrap_or_else(|| comment.to_string()))
        }),
    )
}

/// XML-style comments parser.
pub(super) fn xml_style_comments_parser(
    language: &tree_sitter::Language,
    comment_node_kind: &'static str,
) -> TreeSitterCommentsParser {
    TreeSitterCommentsParser::new(
        language,
        comment_visitor(move |node, source_code| {
            (node.kind() == comment_node_kind)
                .then(|| xml_comment_text(node_text(node, source_code)))
        }),
    )
}

/// `comment`, an XML or HTML comment, with its `<!--` and `-->` blanked, so it keeps its length.
fn xml_comment_text(comment: &str) -> String {
    let (before, rest) = comment
        .split_once("<!--")
        .expect("open comment tag is expected");
    let (content, after) = rest
        .rsplit_once("-->")
        .expect("close comment tag is expected");
    format!("{before}    {content}   {after}")
}

pub(super) fn c_style_multiline_comment_processor(comment: &str) -> String {
    let (before, rest) = comment
        .split_once("/*")
        .expect("expected '/*' in a comment");
    // Scala's grammar and cssparser end a comment that is never closed at the end of the file,
    // so the `*/` can be missing.
    let (content, closing) = match rest.rsplit_once("*/") {
        Some((content, after)) => (content, format!("  {after}")),
        None => (rest, String::new()),
    };
    format!("{before}  {}{closing}", blank_decorative_stars(content))
}

/// Returns `content`, the text between a block comment's delimiters, with the `*` that start each
/// line blanked when only whitespace comes before them. Such stars only decorate the comment, as in
/// ` * ` and ` ** ` lines, and would otherwise break a tag that spans those lines.
pub(super) fn blank_decorative_stars(content: &str) -> String {
    let mut result = String::with_capacity(content.len());
    for line in content.split_inclusive('\n') {
        let text = line.trim_start();
        let after_stars = text.trim_start_matches('*');
        #[expect(clippy::string_slice, reason = "`text` is a suffix of `line`")]
        result.push_str(&line[..line.len() - text.len()]);
        result.push_str(&" ".repeat(text.len() - after_stars.len()));
        result.push_str(after_stars);
    }
    result
}
