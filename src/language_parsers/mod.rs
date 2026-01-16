mod bash;
mod c;
mod c_sharp;
mod cpp;
mod css;
mod go;
mod html;
mod java;
mod javascript;
mod kotlin;
mod makefile;
mod markdown;
mod php;
mod python;
mod ruby;
// pub(crate) visibility is needed by the unit tests in block_parser.rs
pub(crate) mod rust;
mod sql;
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
use std::string::ToString;
use tree_sitter::{Language, Node, Parser, Query, QueryCursor, StreamingIterator};

pub(crate) type LanguageParser = Rc<RefCell<Box<dyn BlocksParser>>>;

/// Returns a map of all available language parsers by their file extensions.
pub fn language_parsers() -> anyhow::Result<HashMap<OsString, LanguageParser>> {
    let bash_parser = Rc::new(RefCell::new(
        Box::new(bash::parser()?) as Box<dyn BlocksParser>
    ));
    let c_parser = Rc::new(RefCell::new(Box::new(c::parser()?) as Box<dyn BlocksParser>));
    let c_sharp_parser = Rc::new(RefCell::new(
        Box::new(c_sharp::parser()?) as Box<dyn BlocksParser>
    ));
    let cpp_parser = Rc::new(RefCell::new(
        Box::new(cpp::parser()?) as Box<dyn BlocksParser>
    ));
    let css_parser = Rc::new(RefCell::new(
        Box::new(css::parser()?) as Box<dyn BlocksParser>
    ));
    let go_parser = Rc::new(RefCell::new(
        Box::new(go::parser()?) as Box<dyn BlocksParser>
    ));
    let html_parser = Rc::new(RefCell::new(
        Box::new(html::parser()?) as Box<dyn BlocksParser>
    ));
    let java_parser = Rc::new(RefCell::new(
        Box::new(java::parser()?) as Box<dyn BlocksParser>
    ));
    let js_parser = Rc::new(RefCell::new(
        Box::new(javascript::parser()?) as Box<dyn BlocksParser>
    ));
    let kotlin_parser = Rc::new(RefCell::new(
        Box::new(kotlin::parser()?) as Box<dyn BlocksParser>
    ));
    let makefile_parser = Rc::new(RefCell::new(
        Box::new(makefile::parser()?) as Box<dyn BlocksParser>
    ));
    let markdown_parser = Rc::new(RefCell::new(
        Box::new(markdown::parser()?) as Box<dyn BlocksParser>
    ));
    let php_parser = Rc::new(RefCell::new(
        Box::new(php::parser()?) as Box<dyn BlocksParser>
    ));
    let python_parser = Rc::new(RefCell::new(
        Box::new(python::parser()?) as Box<dyn BlocksParser>
    ));
    let ruby_parser = Rc::new(RefCell::new(
        Box::new(ruby::parser()?) as Box<dyn BlocksParser>
    ));
    let rust_parser = Rc::new(RefCell::new(
        Box::new(rust::parser()?) as Box<dyn BlocksParser>
    ));
    let sql_parser = Rc::new(RefCell::new(
        Box::new(sql::parser()?) as Box<dyn BlocksParser>
    ));
    let swift_parser = Rc::new(RefCell::new(
        Box::new(swift::parser()?) as Box<dyn BlocksParser>
    ));
    let toml_parser = Rc::new(RefCell::new(
        Box::new(toml::parser()?) as Box<dyn BlocksParser>
    ));
    let typescript_parser = Rc::new(RefCell::new(
        Box::new(typescript::parser()?) as Box<dyn BlocksParser>
    ));
    let typescript_tsx_parser = Rc::new(RefCell::new(
        Box::new(tsx::parser()?) as Box<dyn BlocksParser>
    ));
    let xml_parser = Rc::new(RefCell::new(
        Box::new(xml::parser()?) as Box<dyn BlocksParser>
    ));
    let yaml_parser = Rc::new(RefCell::new(
        Box::new(yaml::parser()?) as Box<dyn BlocksParser>
    ));
    Ok(HashMap::from([
        // <block affects="README.md:supported-grammar, src/blocks.rs:supported-extensions" keep-sorted="asc">
        ("Makefile".into(), Rc::clone(&makefile_parser)),
        ("bash".into(), Rc::clone(&bash_parser)),
        ("c".into(), c_parser),
        ("cc".into(), Rc::clone(&cpp_parser)),
        ("cpp".into(), Rc::clone(&cpp_parser)),
        ("cs".into(), c_sharp_parser),
        ("css".into(), css_parser),
        ("d.ts".into(), Rc::clone(&typescript_parser)),
        ("go".into(), Rc::clone(&go_parser)),
        ("go.mod".into(), Rc::clone(&go_parser)),
        ("go.sum".into(), Rc::clone(&go_parser)),
        ("go.work".into(), go_parser),
        ("h".into(), cpp_parser),
        ("htm".into(), Rc::clone(&html_parser)),
        ("html".into(), html_parser),
        ("java".into(), java_parser),
        ("js".into(), Rc::clone(&js_parser)),
        ("jsx".into(), js_parser),
        ("kt".into(), Rc::clone(&kotlin_parser)),
        ("kts".into(), kotlin_parser),
        ("makefile".into(), Rc::clone(&makefile_parser)),
        ("markdown".into(), Rc::clone(&markdown_parser)),
        ("md".into(), markdown_parser),
        ("mk".into(), makefile_parser),
        ("php".into(), Rc::clone(&php_parser)),
        ("phtml".into(), php_parser),
        ("py".into(), Rc::clone(&python_parser)),
        ("pyi".into(), python_parser),
        ("rb".into(), ruby_parser),
        ("rs".into(), rust_parser),
        ("sh".into(), bash_parser),
        ("sql".into(), sql_parser),
        ("swift".into(), swift_parser),
        ("toml".into(), toml_parser),
        ("ts".into(), typescript_parser),
        ("tsx".into(), typescript_tsx_parser),
        ("xml".into(), xml_parser),
        ("yaml".into(), Rc::clone(&yaml_parser)),
        ("yml".into(), yaml_parser),
        // </block>
    ]))
}

/// Parses comment strings from a source code.
pub(crate) trait CommentsParser {
    /// Returns a `Vec` of `Comment`s.
    // TODO: Return an iterator instead of a Vec.
    fn parse(&mut self, source_code: &str) -> anyhow::Result<Vec<Comment>>;
}

type CaptureProcessor = Box<dyn Fn(usize, &str, &Node) -> anyhow::Result<Option<String>>>;

struct TreeSitterCommentsParser {
    parser: Parser,
    queries: Vec<(Query, Option<CaptureProcessor>)>,
}

impl TreeSitterCommentsParser {
    fn new(language: &Language, queries: Vec<(Query, Option<CaptureProcessor>)>) -> Self {
        let mut parser = Parser::new();
        parser
            .set_language(language)
            .expect("Error setting Tree-sitter language");
        Self { parser, queries }
    }
}

impl CommentsParser for TreeSitterCommentsParser {
    fn parse(&mut self, source_code: &str) -> anyhow::Result<Vec<Comment>> {
        let tree = self.parser.parse(source_code, None).unwrap();
        let root_node = tree.root_node();
        let mut blocks = vec![];
        for (query, post_processor) in self.queries.iter() {
            let mut query_cursor = QueryCursor::new();
            let mut matches = query_cursor.matches(query, root_node, source_code.as_bytes());
            while let Some(query_match) = matches.next() {
                for capture in query_match.captures {
                    let node = capture.node;
                    let start_position = Position::new(
                        node.start_position().row + 1,
                        node.start_position().column + 1,
                    );
                    let end_position =
                        Position::new(node.end_position().row + 1, node.end_position().column + 1);
                    let start_byte = node.start_byte();
                    let end_byte = node.end_byte();
                    let comment_text = &source_code[node.start_byte()..node.end_byte()];
                    if let Some(processor) = post_processor {
                        if let Some(out) = processor(capture.index as usize, comment_text, &node)? {
                            blocks.push(Comment {
                                position_range: start_position..end_position,
                                source_range: start_byte..end_byte,
                                comment_text: out,
                            });
                        }
                    } else {
                        blocks.push(Comment {
                            position_range: start_position..end_position,
                            source_range: start_byte..end_byte,
                            comment_text: comment_text.to_string(),
                        });
                    }
                }
            }
        }

        blocks.sort_by(|comment1, comment2| {
            comment1
                .source_range
                .start
                .cmp(&comment2.source_range.start)
        });
        Ok(blocks)
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

/// C-style comments parser for a query that returns both line and block comments.
fn c_style_comments_parser(language: &Language, query: Query) -> TreeSitterCommentsParser {
    TreeSitterCommentsParser::new(
        language,
        vec![(
            query,
            Some(Box::new(|_, comment, _node| {
                let result = if comment.starts_with("//") {
                    comment.replacen("//", "  ", 1)
                } else {
                    c_style_multiline_comment_processor(comment)
                };
                Ok(Some(result))
            })),
        )],
    )
}

/// C-style comments parser for the separate line and block comment queries.
fn c_style_line_and_block_comments_parser(
    language: &Language,
    line_comment_query: Query,
    block_comment_query: Query,
) -> TreeSitterCommentsParser {
    TreeSitterCommentsParser::new(
        language,
        vec![
            (
                line_comment_query,
                Some(Box::new(|_, comment, _node| {
                    Ok(Some(comment.replacen("//", "  ", 1)))
                })),
            ),
            (
                block_comment_query,
                Some(Box::new(|_, comment, _node| {
                    Ok(Some(c_style_multiline_comment_processor(comment)))
                })),
            ),
        ],
    )
}

/// Python-style comments parser.
fn python_style_comments_parser(
    language: &Language,
    comment_query: Query,
) -> TreeSitterCommentsParser {
    TreeSitterCommentsParser::new(
        language,
        vec![(
            comment_query,
            Some(Box::new(|_, comment, _node| {
                Ok(Some(comment.replacen("#", " ", 1)))
            })),
        )],
    )
}

/// XML-style comments parser.
fn xml_style_comments_parser(
    language: &Language,
    comment_query: Query,
) -> TreeSitterCommentsParser {
    TreeSitterCommentsParser::new(
        language,
        vec![(
            comment_query,
            Some(Box::new(|_, comment, _node| {
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
                Ok(Some(result))
            })),
        )],
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
