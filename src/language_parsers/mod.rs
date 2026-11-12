//! One comment parser per supported language, most of them backed by tree-sitter.

mod bash;
mod c;
mod c_sharp;
mod cmake;
mod cpp;
mod css;
mod dart;
mod dockerfile;
mod elixir;
mod go;
mod graphql;
mod groovy;
mod hcl;
mod html;
mod java;
mod javascript;
mod json;
mod kotlin;
mod lean;
mod lua;
mod makefile;
mod markdown;
mod mdx;
mod nix;
mod ocaml;
mod php;
mod proto;
mod python;
mod ruby;
/// Rust comment parsing. Serves as the stand-in language for the block parser's own unit tests,
/// which is why it alone is visible outside this module.
pub(crate) mod rust;
mod scala;
mod sql;
mod starlark;
mod swift;
pub(crate) mod toml;
mod tsx;
mod typescript;
mod xml;
mod yaml;

use crate::block_parser::BlocksFromCommentsParser;
use crate::blocks::Block;
use crate::language_resolver::{Language, LanguageResolver};
use crate::symbols::{Symbols, SymbolsParser};
use crate::{Position, character_column_at};
use anyhow::Context;
use std::collections::HashMap;
use std::ffi::OsString;
use std::ops::Range;
use std::path::Path;
use std::sync::Mutex;
use tree_sitter::{Node, Parser, Tree, TreeCursor};

/// Parses the source files of one language.
pub(crate) trait LanguageParser: Send + Sync {
    /// Returns an iterator over the [`Block`]s found in the given `contents` string.
    ///
    /// The blocks are required to be yielded sorted by the `starts_at` field in ascending order.
    ///
    /// The iteration stops at the first error: whatever follows a malformed or unbalanced tag
    /// cannot be trusted to belong to the block the source intended.
    ///
    /// Returned boxed rather than as an `impl Iterator` because the trait is used as
    /// `dyn LanguageParser`, which a return-position `impl Trait` would rule out.
    fn parse_blocks<'a>(
        &'a mut self,
        contents: &'a str,
    ) -> Box<dyn Iterator<Item = anyhow::Result<Block>> + 'a>;

    /// Returns the addressable symbols defined in `contents`, and the names a reference must not
    /// resolve in it.
    ///
    /// # Errors
    /// Returns an error if the language has no symbols, or if they cannot be derived from
    /// `contents`.
    fn parse_symbols(&mut self, contents: &str) -> anyhow::Result<Symbols>;
}

/// A [`LanguageParser`] that finds blocks in the comments `C` extracts, and symbols if the language
/// has a [`SymbolsParser`].
struct LanguageParserImpl<C: CommentsParser> {
    blocks_parser: BlocksFromCommentsParser<C>,
    symbols_parser: Option<Box<dyn SymbolsParser>>,
}

impl<C: CommentsParser> LanguageParserImpl<C> {
    /// Creates a parser for a language whose comments `comments_parser` extracts, and which has
    /// no symbols.
    fn new(comments_parser: C) -> Self {
        Self {
            blocks_parser: BlocksFromCommentsParser::new(comments_parser),
            symbols_parser: None,
        }
    }

    /// Gives the language the symbols that `symbols_parser` derives.
    fn with_symbols(self, symbols_parser: impl SymbolsParser + 'static) -> Self {
        Self {
            symbols_parser: Some(Box::new(symbols_parser)),
            ..self
        }
    }
}

impl<C: CommentsParser> LanguageParser for LanguageParserImpl<C> {
    fn parse_blocks<'a>(
        &'a mut self,
        contents: &'a str,
    ) -> Box<dyn Iterator<Item = anyhow::Result<Block>> + 'a> {
        Box::new(self.blocks_parser.parse_blocks(contents))
    }

    fn parse_symbols(&mut self, contents: &str) -> anyhow::Result<Symbols> {
        match &mut self.symbols_parser {
            Some(symbols_parser) => symbols_parser.parse(contents),
            None => anyhow::bail!("symbols are not supported for this language"),
        }
    }
}

/// A parser for one language, shared between files and threads.
///
/// Each parser owns a mutable tree-sitter `Parser` and is therefore behind a `Mutex`.
pub(crate) type SharedLanguageParser = Mutex<Box<dyn LanguageParser>>;

/// The language parsers of a run, one for each [`Language`], and how a file name selects one.
pub(crate) struct LanguageParsers {
    resolver: LanguageResolver,
    bash: SharedLanguageParser,
    c: SharedLanguageParser,
    cmake: SharedLanguageParser,
    c_sharp: SharedLanguageParser,
    cpp: SharedLanguageParser,
    css: SharedLanguageParser,
    dart: SharedLanguageParser,
    dockerfile: SharedLanguageParser,
    elixir: SharedLanguageParser,
    go: SharedLanguageParser,
    go_module: SharedLanguageParser,
    graphql: SharedLanguageParser,
    groovy: SharedLanguageParser,
    hcl: SharedLanguageParser,
    html: SharedLanguageParser,
    java: SharedLanguageParser,
    javascript: SharedLanguageParser,
    json: SharedLanguageParser,
    kotlin: SharedLanguageParser,
    lean: SharedLanguageParser,
    lua: SharedLanguageParser,
    makefile: SharedLanguageParser,
    markdown: SharedLanguageParser,
    mdx: SharedLanguageParser,
    nix: SharedLanguageParser,
    ocaml: SharedLanguageParser,
    php: SharedLanguageParser,
    proto: SharedLanguageParser,
    python: SharedLanguageParser,
    ruby: SharedLanguageParser,
    rust: SharedLanguageParser,
    scala: SharedLanguageParser,
    sql: SharedLanguageParser,
    starlark: SharedLanguageParser,
    swift: SharedLanguageParser,
    toml: SharedLanguageParser,
    tsx: SharedLanguageParser,
    typescript: SharedLanguageParser,
    xml: SharedLanguageParser,
    yaml: SharedLanguageParser,
    zsh: SharedLanguageParser,
}

impl LanguageParsers {
    /// Uses `extension_mappings` to select a parser for the extensions and file names they map.
    pub(crate) fn with_extension_mappings(
        self,
        extension_mappings: HashMap<OsString, Language>,
    ) -> Self {
        Self {
            resolver: LanguageResolver::new(extension_mappings),
            ..self
        }
    }

    /// The parser for `file_path`, or `None` if its name has no language.
    pub(crate) fn parser_for(&self, file_path: &Path) -> Option<&SharedLanguageParser> {
        self.resolver
            .resolve(file_path)
            .map(|language| self.parser(language))
    }

    /// The parser for `language`.
    pub(crate) fn parser(&self, language: Language) -> &SharedLanguageParser {
        match language {
            Language::Bash => &self.bash,
            Language::C => &self.c,
            Language::CMake => &self.cmake,
            Language::CSharp => &self.c_sharp,
            Language::Cpp => &self.cpp,
            Language::Css => &self.css,
            Language::Dart => &self.dart,
            Language::Dockerfile => &self.dockerfile,
            Language::Elixir => &self.elixir,
            Language::Go => &self.go,
            Language::GoModule => &self.go_module,
            Language::GraphQl => &self.graphql,
            Language::Groovy => &self.groovy,
            Language::Hcl => &self.hcl,
            Language::Html => &self.html,
            Language::Java => &self.java,
            Language::JavaScript => &self.javascript,
            Language::Json => &self.json,
            Language::Kotlin => &self.kotlin,
            Language::Lean => &self.lean,
            Language::Lua => &self.lua,
            Language::Makefile => &self.makefile,
            Language::Markdown => &self.markdown,
            Language::Mdx => &self.mdx,
            Language::Nix => &self.nix,
            Language::OCaml => &self.ocaml,
            Language::Php => &self.php,
            Language::Proto => &self.proto,
            Language::Python => &self.python,
            Language::Ruby => &self.ruby,
            Language::Rust => &self.rust,
            Language::Scala => &self.scala,
            Language::Sql => &self.sql,
            Language::Starlark => &self.starlark,
            Language::Swift => &self.swift,
            Language::Toml => &self.toml,
            Language::Tsx => &self.tsx,
            Language::TypeScript => &self.typescript,
            Language::Xml => &self.xml,
            Language::Yaml => &self.yaml,
            Language::Zsh => &self.zsh,
        }
    }
}

/// Builds every language parser, with no extension mappings.
///
/// Returns an error if a grammar fails to load.
pub(crate) fn language_parsers() -> anyhow::Result<LanguageParsers> {
    fn parser<P: LanguageParser + 'static>(p: P) -> SharedLanguageParser {
        Mutex::new(Box::new(p))
    }

    Ok(LanguageParsers {
        resolver: LanguageResolver::new(HashMap::new()),
        bash: parser(bash::parser()?),
        c: parser(c::parser()?),
        cmake: parser(cmake::parser()?),
        c_sharp: parser(c_sharp::parser()?),
        cpp: parser(cpp::parser()?),
        css: parser(css::parser()?),
        dart: parser(dart::parser()?),
        dockerfile: parser(dockerfile::parser()?),
        elixir: parser(elixir::parser()?),
        go: parser(go::parser()?),
        go_module: parser(go::module_files_parser()?),
        graphql: parser(graphql::parser()?),
        groovy: parser(groovy::parser()?),
        hcl: parser(hcl::parser()?),
        html: parser(html::parser()?),
        java: parser(java::parser()?),
        javascript: parser(javascript::parser()?),
        json: parser(json::parser()?),
        kotlin: parser(kotlin::parser()?),
        lean: parser(lean::parser()?),
        lua: parser(lua::parser()?),
        makefile: parser(makefile::parser()?),
        markdown: parser(markdown::parser()?),
        mdx: parser(mdx::parser()?),
        nix: parser(nix::parser()?),
        ocaml: parser(ocaml::parser()?),
        php: parser(php::parser()?),
        proto: parser(proto::parser()?),
        python: parser(python::parser()?),
        ruby: parser(ruby::parser()?),
        rust: parser(rust::parser()?),
        scala: parser(scala::parser()?),
        sql: parser(sql::parser()?),
        starlark: parser(starlark::parser()?),
        swift: parser(swift::parser()?),
        toml: parser(toml::parser()?),
        tsx: parser(tsx::parser()?),
        typescript: parser(typescript::parser()?),
        xml: parser(xml::parser()?),
        yaml: parser(yaml::parser()?),
        zsh: parser(bash::zsh_parser()?),
    })
}

/// Parses comment string from a source code by returning an iterator of `Comment`s.
pub(crate) trait CommentsParser: Send + Sync {
    /// Returns an iterator of `Comment`s from the source code.
    fn parse<'source>(
        &'source mut self,
        source_code: &'source str,
    ) -> impl Iterator<Item = Comment> + 'source;
}

/// What the comment walk does at a node.
enum Visit {
    /// Recurse into the node's children; the node yields no comment.
    Continue,
    /// Stop at this node, not descending into its children.
    /// If the node is a comment, then it must contain `Some(Comment)`.
    Break(Option<Comment>),
}

/// Maps a node to a [`Visit`]: the one place a grammar's node kinds are interpreted, one per language.
type NodeVisitor = Box<dyn Fn(&Node, &str) -> Visit + Send + Sync>;

/// Creates a Tree-sitter [`Parser`] configured for `language`.
///
/// # Errors
/// Returns an error if the language cannot be set on the parser.
pub(crate) fn tree_sitter_parser(language: &tree_sitter::Language) -> anyhow::Result<Parser> {
    let mut parser = Parser::new();
    parser
        .set_language(language)
        .context("failed to set tree-sitter language")?;
    Ok(parser)
}

struct TreeSitterCommentsParser {
    parser: Parser,
    node_visitor: NodeVisitor,
    tree: Option<Tree>,
}

impl TreeSitterCommentsParser {
    fn new(language: &tree_sitter::Language, node_visitor: NodeVisitor) -> anyhow::Result<Self> {
        let parser = tree_sitter_parser(language)?;
        Ok(Self {
            parser,
            node_visitor,
            tree: None,
        })
    }

    /// Stops the walk at nodes of `kinds` without reading a comment out of them.
    fn with_break_at_node_kinds(self, kinds: &'static [&'static str]) -> Self {
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
    fn with_break_when(self, predicate: fn(&Node, &str) -> bool) -> Self {
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
fn comment_visitor<F>(comment_text: F) -> NodeVisitor
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

/// The text of `node` in `source`, the text that tree-sitter parsed.
pub(crate) fn node_text<'source>(node: &Node, source: &'source str) -> &'source str {
    #[expect(
        clippy::string_slice,
        reason = "tree-sitter nodes start and end on whole characters"
    )]
    &source[node.byte_range()]
}

/// The named children of `node`, without comments, in document order.
fn members(node: Node) -> Vec<Node> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|child| !child.is_extra())
        .collect()
}

/// Calls `visit` on `root` and then on the nodes below it, in document order. `visit` returns
/// whether to visit the children of the node it gets.
pub(crate) fn walk_tree<'tree>(root: Node<'tree>, mut visit: impl FnMut(Node<'tree>) -> bool) {
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
    fn from_node(node: &Node, source_code: &str, comment_text: String) -> Self {
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
    fn set_character_columns(&mut self, source: &str) {
        self.position_range.start.character = character_column_at(source, self.source_range.start);
        self.position_range.end.character = character_column_at(source, self.source_range.end);
    }

    /// Shifts a comment that was parsed from a region of `source` into the coordinates of the
    /// full `source`. The region starts at byte `region_start_byte`, on the 0-based row
    /// `region_start_row`.
    fn shift_into_source(
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
fn blank_preserving_line_breaks(bytes: &mut [u8]) {
    for byte in bytes {
        if *byte != b'\n' && *byte != b'\r' {
            *byte = b' ';
        }
    }
}

/// `comment` with each byte outside `content`, a byte range of it, turned into a space, so it
/// keeps its length. The bytes outside `content` must be ASCII, such as the markers of a comment.
fn blank_outside(comment: &str, content: Range<usize>) -> String {
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
fn blank_line_marker(comment: &str, markers: &[&str]) -> Option<String> {
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
const SLASH_MARKERS: &[&str] = &["//!", "//"];

/// The line comment markers of C and C++. Doxygen documents the member before a comment that
/// starts with `///<` or `//!<`.
const DOXYGEN_MARKERS: &[&str] = &["//!<", "//<", "//!", "//"];

/// The line comment markers of a language with both `//` and `#` line comments.
const SLASH_AND_HASH_MARKERS: &[&str] = &["//!", "//", "#"];

/// A comments parser for a language whose comments are line comments that start with one of
/// `line_markers` (see [`blank_line_marker`]), or `/* */` comments. A node is a comment when its
/// kind is in `comment_node_kinds`.
fn c_style_comments_parser(
    language: &tree_sitter::Language,
    comment_node_kinds: &'static [&'static str],
    line_markers: &'static [&'static str],
) -> anyhow::Result<TreeSitterCommentsParser> {
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
fn c_style_comment_text(comment: &str, line_markers: &[&str]) -> String {
    match blank_line_marker(comment, line_markers) {
        Some(text) => text,
        None if comment.starts_with("/*") => c_style_multiline_comment_processor(comment),
        // A grammar can read other text as a comment, such as a shebang in Scala.
        None => comment.to_string(),
    }
}

/// Comments parser shared by the ECMAScript-family grammars (JavaScript, TypeScript, TSX). All
/// three use the same node kinds for comments, so they differ only in which grammar is loaded.
fn ecmascript_comments_parser(
    language: &tree_sitter::Language,
) -> anyhow::Result<TreeSitterCommentsParser> {
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
fn python_style_comments_parser(
    language: &tree_sitter::Language,
    comment_node_kind: &'static str,
    line_markers: &'static [&'static str],
) -> anyhow::Result<TreeSitterCommentsParser> {
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
fn xml_style_comments_parser(
    language: &tree_sitter::Language,
    comment_node_kind: &'static str,
) -> anyhow::Result<TreeSitterCommentsParser> {
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

fn c_style_multiline_comment_processor(comment: &str) -> String {
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
fn blank_decorative_stars(content: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::symbol_path::SymbolPath;

    #[test]
    fn extensions_with_symbols_match_the_documented_list() -> anyhow::Result<()> {
        let mut extensions_with_symbols = Vec::new();
        let parsers = language_parsers()?;
        for &(name, language) in crate::language_resolver::NAMES {
            let parser = parsers.parser(language);
            // A parser without symbols refuses every source with the same error, so an empty
            // source is enough to tell. A parser with symbols may accept it or report it as broken.
            let has_symbols = match parser
                .lock()
                .expect("a thread panicked while it held the parser")
                .parse_symbols("")
            {
                Ok(_) => true,
                Err(err) => err.to_string() != "symbols are not supported for this language",
            };
            if has_symbols {
                extensions_with_symbols.push(name);
            }
        }
        extensions_with_symbols.sort();

        /* <block name="extensions-with-symbols"
        same-as="docs/symbols.md:extensions-with-symbols, src/skill.md:extensions-with-symbols"
        same-as-pattern='"(?P<value>[^"]+)"'> */
        let documented = [
            "BUCK",
            "BUILD",
            "Containerfile",
            "Dockerfile",
            "GNUmakefile",
            "Makefile",
            "Pipfile",
            "Tiltfile",
            "WORKSPACE",
            "bash",
            "bash_profile",
            "bashrc",
            "bazel",
            "bzl",
            "bzlmod",
            "c",
            "cc",
            "cff",
            "cjs",
            "containerfile",
            "cpp",
            "cs",
            "csproj",
            "css",
            "cts",
            "cxx",
            "d.ts",
            "dart",
            "dockerfile",
            "envrc",
            "fsproj",
            "go",
            "h",
            "hcl",
            "hh",
            "hpp",
            "hxx",
            "java",
            "js",
            "json",
            "jsonc",
            "jsx",
            "kt",
            "kts",
            "mak",
            "makefile",
            "markdown",
            "md",
            "mdx",
            "mjs",
            "mk",
            "mts",
            "nomad",
            "php",
            "phtml",
            "props",
            "proto",
            "py",
            "pyi",
            "pyw",
            "rs",
            "sh",
            "star",
            "swift",
            "targets",
            "tf",
            "tfvars",
            "tofu",
            "toml",
            "ts",
            "tsx",
            "vbproj",
            "vcxproj",
            "webmanifest",
            "xaml",
            "xml",
            "xsd",
            "xsl",
            "xslt",
            "yaml",
            "yml",
        ];
        // </block>
        assert_eq!(extensions_with_symbols, documented);
        Ok(())
    }

    #[test]
    fn language_without_symbols_parse_symbols_returns_error() -> anyhow::Result<()> {
        let mut parser = ruby::parser()?;

        let err = parser.parse_symbols("def main; end").unwrap_err();

        assert!(
            err.to_string()
                .contains("symbols are not supported for this language"),
            "unexpected error message: {err}"
        );
        Ok(())
    }

    #[test]
    fn typescript_file_with_type_assertion_resolves_its_symbols() -> anyhow::Result<()> {
        let parsers = language_parsers()?;
        let path = SymbolPath::parse("/a")?;
        let resolves = |parser: &SharedLanguageParser| -> anyhow::Result<bool> {
            let symbols = parser
                .lock()
                .expect("a thread panicked while it held the parser")
                .parse_symbols("let a = <string>b;\n")?;
            Ok(symbols.resolve(&path).is_ok())
        };

        let typescript = parsers
            .parser_for(Path::new("a.ts"))
            .expect("`.ts` files have a parser");
        assert!(resolves(typescript)?);
        // TSX reads `<string>` as the start of an element, so the source tells the two apart.
        assert!(!resolves(parsers.parser(Language::Tsx))?);
        Ok(())
    }
}
