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
mod kotlin;
mod lua;
mod makefile;
mod markdown;
mod nix;
mod php;
mod proto;
mod python;
mod ruby;
// pub(crate) visibility is needed by the unit tests in block_parser.rs
pub(crate) mod rust;
mod scala;
mod sql;
mod starlark;
mod swift;
mod toml;
mod tsx;
mod typescript;
mod xml;
mod yaml;

use crate::Position;
use crate::block_parser::BlocksParser;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::OsString;
use std::ops::Range;
use std::rc::Rc;
use tree_sitter::{Language, Node, Parser, Tree, TreeCursor};

pub(crate) type LanguageParser = Rc<RefCell<Box<dyn BlocksParser>>>;

/// Returns a map of all available language parsers by their file extensions.
pub fn language_parsers() -> anyhow::Result<HashMap<OsString, LanguageParser>> {
    fn parser<P: BlocksParser + 'static>(p: P) -> LanguageParser {
        Rc::new(RefCell::new(Box::new(p) as Box<dyn BlocksParser>))
    }

    let bash_parser = parser(bash::parser()?);
    let c_parser = parser(c::parser()?);
    let c_sharp_parser = parser(c_sharp::parser()?);
    let cmake_parser = parser(cmake::parser()?);
    let cpp_parser = parser(cpp::parser()?);
    let css_parser = parser(css::parser()?);
    let dart_parser = parser(dart::parser()?);
    let dockerfile_parser = parser(dockerfile::parser()?);
    let elixir_parser = parser(elixir::parser()?);
    let go_parser = parser(go::parser()?);
    let graphql_parser = parser(graphql::parser()?);
    let groovy_parser = parser(groovy::parser()?);
    let hcl_parser = parser(hcl::parser()?);
    let html_parser = parser(html::parser()?);
    let java_parser = parser(java::parser()?);
    let js_parser = parser(javascript::parser()?);
    let kotlin_parser = parser(kotlin::parser()?);
    let lua_parser = parser(lua::parser()?);
    let makefile_parser = parser(makefile::parser()?);
    let markdown_parser = parser(markdown::parser()?);
    let nix_parser = parser(nix::parser()?);
    let php_parser = parser(php::parser()?);
    let proto_parser = parser(proto::parser()?);
    let python_parser = parser(python::parser()?);
    let ruby_parser = parser(ruby::parser()?);
    let rust_parser = parser(rust::parser()?);
    let scala_parser = parser(scala::parser()?);
    let sql_parser = parser(sql::parser()?);
    let starlark_parser = parser(starlark::parser()?);
    let swift_parser = parser(swift::parser()?);
    let toml_parser = parser(toml::parser()?);
    let typescript_parser = parser(typescript::parser()?);
    let typescript_tsx_parser = parser(tsx::parser()?);
    let xml_parser = parser(xml::parser()?);
    let yaml_parser = parser(yaml::parser()?);

    Ok(HashMap::from([
        // <block affects="README.md:supported-grammar, src/blocks.rs:supported-extensions" keep-sorted>
        ("BUILD".into(), Rc::clone(&starlark_parser)),
        ("CMakeLists.txt".into(), Rc::clone(&cmake_parser)),
        ("Containerfile".into(), Rc::clone(&dockerfile_parser)),
        ("Dockerfile".into(), Rc::clone(&dockerfile_parser)),
        ("Jenkinsfile".into(), Rc::clone(&groovy_parser)),
        ("Makefile".into(), Rc::clone(&makefile_parser)),
        ("WORKSPACE".into(), Rc::clone(&starlark_parser)),
        ("bash".into(), Rc::clone(&bash_parser)),
        ("bazel".into(), Rc::clone(&starlark_parser)),
        ("bzl".into(), Rc::clone(&starlark_parser)),
        ("bzlmod".into(), Rc::clone(&starlark_parser)),
        ("c".into(), c_parser),
        ("cc".into(), Rc::clone(&cpp_parser)),
        ("cmake".into(), cmake_parser),
        ("containerfile".into(), Rc::clone(&dockerfile_parser)),
        ("cpp".into(), Rc::clone(&cpp_parser)),
        ("cs".into(), c_sharp_parser),
        ("css".into(), css_parser),
        ("d.ts".into(), Rc::clone(&typescript_parser)),
        ("dart".into(), dart_parser),
        ("dockerfile".into(), dockerfile_parser),
        ("ex".into(), Rc::clone(&elixir_parser)),
        ("exs".into(), elixir_parser),
        ("go".into(), Rc::clone(&go_parser)),
        ("go.mod".into(), Rc::clone(&go_parser)),
        ("go.sum".into(), Rc::clone(&go_parser)),
        ("go.work".into(), go_parser),
        ("gql".into(), Rc::clone(&graphql_parser)),
        ("gradle".into(), Rc::clone(&groovy_parser)),
        ("graphql".into(), graphql_parser),
        ("groovy".into(), Rc::clone(&groovy_parser)),
        ("h".into(), cpp_parser),
        ("hcl".into(), Rc::clone(&hcl_parser)),
        ("htm".into(), Rc::clone(&html_parser)),
        ("html".into(), html_parser),
        ("java".into(), java_parser),
        ("jenkinsfile".into(), groovy_parser),
        ("js".into(), Rc::clone(&js_parser)),
        ("jsx".into(), js_parser),
        ("kt".into(), Rc::clone(&kotlin_parser)),
        ("kts".into(), kotlin_parser),
        ("lua".into(), lua_parser),
        ("makefile".into(), Rc::clone(&makefile_parser)),
        ("markdown".into(), Rc::clone(&markdown_parser)),
        ("md".into(), markdown_parser),
        ("mk".into(), makefile_parser),
        ("nix".into(), nix_parser),
        ("php".into(), Rc::clone(&php_parser)),
        ("phtml".into(), php_parser),
        ("proto".into(), proto_parser),
        ("py".into(), Rc::clone(&python_parser)),
        ("pyi".into(), python_parser),
        ("rb".into(), ruby_parser),
        ("rs".into(), rust_parser),
        ("sbt".into(), Rc::clone(&scala_parser)),
        ("scala".into(), scala_parser),
        ("sh".into(), bash_parser),
        ("sql".into(), sql_parser),
        ("star".into(), starlark_parser),
        ("swift".into(), swift_parser),
        ("tf".into(), Rc::clone(&hcl_parser)),
        ("tfvars".into(), hcl_parser),
        ("toml".into(), toml_parser),
        ("ts".into(), typescript_parser),
        ("tsx".into(), typescript_tsx_parser),
        ("xml".into(), xml_parser),
        ("yaml".into(), Rc::clone(&yaml_parser)),
        ("yml".into(), yaml_parser),
        // </block>
    ]))
}

/// Parses comment string from a source code by returning an iterator of `Comment`s.
pub(crate) trait CommentsParser {
    /// Returns an iterator of `Comment`s from the source code.
    fn parse<'source>(
        &'source mut self,
        source_code: &'source str,
    ) -> impl Iterator<Item = Comment> + 'source;
}

type NodeVisitor = Box<dyn Fn(&Node, &str) -> Option<String>>;

struct TreeSitterCommentsParser {
    parser: Parser,
    node_visitor: NodeVisitor,
    tree: Option<Tree>,
}

impl TreeSitterCommentsParser {
    fn new(language: &Language, node_visitor: NodeVisitor) -> Self {
        let mut parser = Parser::new();
        parser
            .set_language(language)
            .expect("Error setting Tree-sitter language");
        Self {
            parser,
            node_visitor,
            tree: None,
        }
    }
}

impl CommentsParser for TreeSitterCommentsParser {
    fn parse<'a>(&'a mut self, source_code: &'a str) -> impl Iterator<Item = Comment> + 'a {
        let tree = self.parser.parse(source_code, None).unwrap();
        self.tree = Some(tree);
        // It is safe to unwrap here because we just set self.tree
        CommentsIterator::new(self.tree.as_ref().unwrap(), &self.node_visitor, source_code)
    }
}

struct CommentsIterator<'source> {
    cursor: TreeCursor<'source>,
    node_visitor: &'source NodeVisitor,
    source_code: &'source str,
    start_visited: bool,
}

impl<'source> CommentsIterator<'source> {
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
        }
    }

    fn comment_from_current_node(&self) -> Option<Comment> {
        let node = self.cursor.node();
        let comment_text = (self.node_visitor)(&node, self.source_code)?;
        Some(comment_from_node(&node, comment_text))
    }
}

impl<'source> Iterator for CommentsIterator<'source> {
    type Item = Comment;

    /// Traverses the tree-sitter AST via DFS and extracts comments.
    fn next(&mut self) -> Option<Self::Item> {
        if !self.start_visited {
            self.start_visited = true;
            if let Some(comment) = self.comment_from_current_node() {
                return Some(comment);
            }
        }

        loop {
            if self.cursor.goto_first_child() {
                if let Some(comment) = self.comment_from_current_node() {
                    return Some(comment);
                }
                continue;
            }

            if self.cursor.goto_next_sibling() {
                if let Some(comment) = self.comment_from_current_node() {
                    return Some(comment);
                }
                continue;
            }

            loop {
                if !self.cursor.goto_parent() {
                    return None;
                }
                if self.cursor.goto_next_sibling() {
                    if let Some(comment) = self.comment_from_current_node() {
                        return Some(comment);
                    }
                    break;
                }
            }
        }
    }
}

#[derive(Debug, PartialEq)]
pub(crate) struct Comment {
    // Position range of the comment in the source.
    pub(crate) position_range: Range<Position>,
    // Byte offset (i.e. position) of the comment in the source.
    pub(crate) source_range: Range<usize>,
    // The `comment_string` is expected to be the content of the comment with all language specific
    // comment symbols like "//", "/**", "#", etc replaced with the corresponding number of
    // whitespaces ("  " for "//", "   " for "/**", etc.) so that the length of the comment is
    // preserved.
    pub(crate) comment_text: String,
}

/// Builds a [`Comment`] spanning `node`, converting tree-sitter's 0-based rows/columns to the
/// 1-based positions used throughout.
fn comment_from_node(node: &Node, comment_text: String) -> Comment {
    Comment {
        position_range: Position::new(
            node.start_position().row + 1,
            node.start_position().column + 1,
        )
            ..Position::new(node.end_position().row + 1, node.end_position().column + 1),
        source_range: node.start_byte()..node.end_byte(),
        comment_text,
    }
}

/// Shifts a `comment` that was parsed from the sub-region spanned by `region_node` into the
/// coordinates of the full source. A region can start mid-line, so comments on its first line
/// also need their columns shifted.
fn offset_comment(comment: &mut Comment, region_node: &Node) {
    if comment.position_range.start.line == 1 {
        comment.position_range.start.character += region_node.start_position().column;
    }
    if comment.position_range.end.line == 1 {
        comment.position_range.end.character += region_node.start_position().column;
    }
    comment.position_range.start.line += region_node.start_position().row;
    comment.position_range.end.line += region_node.start_position().row;
    comment.source_range.start += region_node.start_byte();
    comment.source_range.end += region_node.start_byte();
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

/// C-style comments parser for a query that returns both line and block comments.
fn c_style_comments_parser(
    language: &Language,
    comment_node_kind: &'static str,
) -> TreeSitterCommentsParser {
    TreeSitterCommentsParser::new(
        language,
        Box::new(move |node, source_code| {
            if node.kind() != comment_node_kind {
                return None;
            }
            let comment = source_code.get(node.byte_range()).unwrap();
            Some(if comment.starts_with("//") {
                comment.replacen("//", "  ", 1)
            } else {
                c_style_multiline_comment_processor(comment)
            })
        }),
    )
}

/// C-style comments parser for the separate line and block comment queries.
fn c_style_line_and_block_comments_parser(
    language: &Language,
    line_comment_node_kind: &'static str,
    block_comment_node_kind: &'static str,
) -> TreeSitterCommentsParser {
    TreeSitterCommentsParser::new(
        language,
        Box::new(move |node, source_code| {
            let kind = node.kind();
            if kind == line_comment_node_kind {
                Some(source_code[node.byte_range()].replacen("//", "  ", 1))
            } else if kind == block_comment_node_kind {
                Some(c_style_multiline_comment_processor(
                    &source_code[node.byte_range()],
                ))
            } else {
                None
            }
        }),
    )
}

/// C-style comments parser that additionally blanks the full `///` doc-comment marker, for
/// languages where `///` is the primary documentation style (e.g. Dart, C#).
fn c_style_and_doc_comments_parser(
    language: &Language,
    comment_node_kind: &'static str,
) -> TreeSitterCommentsParser {
    TreeSitterCommentsParser::new(
        language,
        Box::new(move |node, source_code| {
            if node.kind() != comment_node_kind {
                return None;
            }
            let comment = &source_code[node.byte_range()];
            Some(if comment.starts_with("///") {
                comment.replacen("///", "   ", 1)
            } else if comment.starts_with("//") {
                comment.replacen("//", "  ", 1)
            } else {
                c_style_multiline_comment_processor(comment)
            })
        }),
    )
}

/// Like [`c_style_line_and_block_comments_parser`], but additionally blanks the full `///`
/// doc-comment marker, for languages where `///` is the primary documentation style
/// (e.g. Swift).
fn c_style_and_doc_line_and_block_comments_parser(
    language: &Language,
    line_comment_node_kind: &'static str,
    block_comment_node_kind: &'static str,
) -> TreeSitterCommentsParser {
    TreeSitterCommentsParser::new(
        language,
        Box::new(move |node, source_code| {
            let kind = node.kind();
            if kind == line_comment_node_kind {
                let comment = &source_code[node.byte_range()];
                Some(if comment.starts_with("///") {
                    comment.replacen("///", "   ", 1)
                } else {
                    comment.replacen("//", "  ", 1)
                })
            } else if kind == block_comment_node_kind {
                Some(c_style_multiline_comment_processor(
                    &source_code[node.byte_range()],
                ))
            } else {
                None
            }
        }),
    )
}

/// C-style comments parser that also handles the legacy JavaScript HTML-like comments
/// (Annex B of the ECMAScript spec): single lines starting with `<!--` or `-->`.
fn c_style_and_html_comments_parser(
    language: &Language,
    comment_node_kind: &'static str,
    html_comment_node_kind: &'static str,
) -> TreeSitterCommentsParser {
    TreeSitterCommentsParser::new(
        language,
        Box::new(move |node, source_code| {
            let kind = node.kind();
            if kind == comment_node_kind {
                let comment = &source_code[node.byte_range()];
                Some(if comment.starts_with("//") {
                    comment.replacen("//", "  ", 1)
                } else {
                    c_style_multiline_comment_processor(comment)
                })
            } else if kind == html_comment_node_kind {
                let comment = &source_code[node.byte_range()];
                Some(if comment.starts_with("<!--") {
                    comment.replacen("<!--", "    ", 1)
                } else {
                    comment.replacen("-->", "   ", 1)
                })
            } else {
                None
            }
        }),
    )
}

/// Comments parser for languages that support `#` line comments in addition to the C-style
/// `//` and `/* */` comments.
/// Blanks the leading `//` or `#` line-comment marker, or the `/* */` block-comment delimiters,
/// of a hash-or-C-style comment, preserving the comment's length.
fn hash_and_c_style_comment_text(comment: &str) -> String {
    if comment.starts_with("//") {
        comment.replacen("//", "  ", 1)
    } else if comment.starts_with("#") {
        comment.replacen("#", " ", 1)
    } else {
        c_style_multiline_comment_processor(comment)
    }
}

fn hash_and_c_style_comments_parser(
    language: &Language,
    comment_node_kind: &'static str,
) -> TreeSitterCommentsParser {
    TreeSitterCommentsParser::new(
        language,
        Box::new(move |node, source_code| {
            if node.kind() != comment_node_kind {
                return None;
            }
            Some(hash_and_c_style_comment_text(
                &source_code[node.byte_range()],
            ))
        }),
    )
}

/// Python-style comments parser.
fn python_style_comments_parser(
    language: &Language,
    comment_node_kind: &'static str,
) -> TreeSitterCommentsParser {
    TreeSitterCommentsParser::new(
        language,
        Box::new(move |node, source_code| {
            if node.kind() != comment_node_kind {
                return None;
            }
            let comment = &source_code[node.byte_range()];
            Some(if comment.starts_with('#') {
                comment.replacen('#', " ", 1)
            } else {
                // A comment form without a leading `#`: keep it intact rather than blanking a
                // `#` that belongs to its content.
                comment.to_string()
            })
        }),
    )
}

/// XML-style comments parser.
fn xml_style_comments_parser(
    language: &Language,
    comment_node_kind: &'static str,
) -> TreeSitterCommentsParser {
    TreeSitterCommentsParser::new(
        language,
        Box::new(move |node, source_code| {
            if node.kind() == comment_node_kind {
                let comment = &source_code[node.byte_range()];
                let open_idx = comment.find("<!--").expect("open comment tag is expected");
                let close_idx = comment.rfind("-->").expect("close comment tag is expected");
                let mut result = String::with_capacity(comment.len());
                result.push_str(&comment[..open_idx]);
                // Replace "<!--" with spaces.
                result.push_str("    ");
                result.push_str(&comment[open_idx + 4..close_idx]);
                // Replace "-->" with spaces.
                result.push_str("   ");
                result.push_str(&comment[close_idx + 3..]);
                Some(result)
            } else {
                None
            }
        }),
    )
}

fn c_style_multiline_comment_processor(comment: &str) -> String {
    let mut result = String::with_capacity(comment.len());
    let open_idx = comment.find("/*").expect("expected '/*' in a comment");
    let close_idx = comment.rfind("*/").expect("expected '*/' in a comment");
    // Add everything before the "/*"
    result.push_str(&comment[..open_idx]);
    // Replace "/*" with spaces.
    result.push_str("  ");
    let content = &comment[open_idx + 2..close_idx];
    for line in content.split_inclusive('\n') {
        let mut decorative_star_found = false;

        // Find the index of the first non-whitespace character
        if let Some(first_non_whitespace_idx) = line.find(|c: char| !c.is_whitespace()) {
            // Check if that first non-whitespace character is a '*'
            if line[first_non_whitespace_idx..].starts_with('*') {
                decorative_star_found = true;
                // Add leading whitespace.
                result.push_str(&line[..first_non_whitespace_idx]);
                // Replace "*" with a space.
                result.push(' ');
                // Add the rest of the line.
                result.push_str(&line[first_non_whitespace_idx + 1..]);
            }
        }
        if !decorative_star_found {
            // Not a decorative '*', or all whitespace. Add unchanged.
            result.push_str(line);
        }
    }
    // Replace "*/" with spaces.
    result.push_str("  ");
    // Add everything after the "*/".
    result.push_str(&comment[close_idx + 2..]);

    result
}
