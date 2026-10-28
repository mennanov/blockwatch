use crate::language_parsers::{
    CommentsParser, LanguageParser, LanguageParserImpl, TreeSitterCommentsParser,
    c_style_multiline_comment_processor, comment_visitor, node_text,
};
use crate::symbols::code::{self, CodeLanguage, CodeSymbolsParser, Definition, Value};
use tree_sitter::Node;

/// Returns a [`LanguageParser`] for Rust, with its symbols.
pub(crate) fn parser() -> anyhow::Result<impl LanguageParser> {
    Ok(
        LanguageParserImpl::new(comments_parser()?).with_symbols(CodeSymbolsParser::new(
            &tree_sitter_rust::LANGUAGE.into(),
            Rust,
        )?),
    )
}

/// Rust, whose top-level items are constants, statics, functions, types, traits, macros and
/// modules.
struct Rust;

impl CodeLanguage for Rust {
    /// The top-level items under `root`, the root of a Rust file's syntax tree, in document order.
    /// `source` is the text of the file.
    fn definitions<'tree>(&self, root: Node<'tree>, source: &str) -> Vec<Definition<'tree>> {
        let mut cursor = root.walk();
        root.named_children(&mut cursor)
            .filter_map(|item| {
                let value = match item.kind() {
                    "const_item" | "static_item" => item.child_by_field_name("value"),
                    "enum_item" | "struct_item" | "union_item" | "type_item" | "trait_item"
                    | "function_item" | "macro_definition" => None,
                    // `mod file;` only says which file holds the module.
                    "mod_item" if item.child_by_field_name("body").is_some() => None,
                    _ => return None,
                };
                // `const _` runs a check at compile time, and code can't refer to it.
                let name = item
                    .child_by_field_name("name")
                    .filter(|name| node_text(name, source) != "_")?;
                // The attributes above an item belong to it, even with comments between them.
                let start = std::iter::successors(item.prev_sibling(), Node::prev_sibling)
                    .take_while(|node| node.kind() == "attribute_item" || node.is_extra())
                    .filter(|node| node.kind() == "attribute_item")
                    .last()
                    .unwrap_or(item)
                    .start_byte();
                Some(Definition {
                    parents: Vec::new(),
                    names: vec![name],
                    range: start..item.end_byte(),
                    value: value.map(Value::Initializer),
                })
            })
            .collect()
    }

    /// Decodes a Rust string literal. Returns `None` for any other node, and for a byte string or a
    /// C string.
    ///
    /// # Errors
    /// Returns an error if an escape stands for no character.
    fn decode_string(&self, node: &Node, source: &str) -> anyhow::Result<Option<String>> {
        match node.kind() {
            // A byte string or a C string starts with its prefix, such as `br"` or `c"`.
            "string_literal" | "raw_string_literal"
                if !node_text(node, source).starts_with(['b', 'c']) =>
            {
                let mut text = String::new();
                // After a backslash at the end of a line, Rust skips the white space that starts
                // the next lines.
                let mut joined = false;
                let mut cursor = node.walk();
                // A raw string has no escapes, only its content.
                for part in node.named_children(&mut cursor) {
                    let written = node_text(&part, source);
                    match part.kind() {
                        "escape_sequence" if matches!(written, "\\\n" | "\\\r") => joined = true,
                        "escape_sequence" => {
                            joined = false;
                            text.push(escaped_character(written)?);
                        }
                        _ => {
                            let written = if joined {
                                written.trim_start_matches([' ', '\t', '\n', '\r'])
                            } else {
                                written
                            };
                            // Rust reads a CRLF line end in a file as a line feed.
                            text.push_str(&written.replace("\r\n", "\n"));
                        }
                    }
                }
                Ok(Some(text))
            }
            _ => Ok(None),
        }
    }
}

/// The character that `escape`, such as `\n` or `\u{e9}`, stands for.
///
/// # Errors
/// Returns an error if `escape` is not a Rust escape, or its code is not a character.
fn escaped_character(escape: &str) -> anyhow::Result<char> {
    match escape {
        r"\n" => Ok('\n'),
        r"\r" => Ok('\r'),
        r"\t" => Ok('\t'),
        r"\0" => Ok('\0'),
        r"\\" => Ok('\\'),
        r"\'" => Ok('\''),
        r#"\""# => Ok('"'),
        _ => {
            // An escape that Rust doesn't have, such as `\d`, has no digits, so it stands for no
            // character.
            let digits = escape
                .strip_prefix(r"\x")
                .or_else(|| escape.strip_prefix(r"\u{")?.strip_suffix('}'))
                .unwrap_or_default();
            code::char_from_digits(escape, digits, 16)
        }
    }
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let rust_language = tree_sitter_rust::LANGUAGE.into();
    let parser = TreeSitterCommentsParser::new(
        &rust_language,
        comment_visitor(|node, source_code| match node.kind() {
            "line_comment" => {
                let comment = node_text(node, source_code);
                Some(if comment.starts_with("///") {
                    comment.replacen("///", "   ", 1)
                } else if comment.starts_with("//!") {
                    comment.replacen("//!", "   ", 1)
                } else if comment.starts_with("//") {
                    comment.replacen("//", "  ", 1)
                } else {
                    comment.to_string()
                })
            }
            "block_comment" => Some(c_style_multiline_comment_processor(node_text(
                node,
                source_code,
            ))),
            _ => None,
        }),
    )?;
    Ok(parser)
}

#[cfg(test)]
mod symbols_tests {
    use super::*;
    use crate::symbols::code::test_utils::{
        decode_error, symbol, symbols, syntax_error, unresolved, value, values,
    };
    use crate::symbols::{Symbols, SymbolsParser};

    /// The symbols of `source`, a Rust file.
    fn rust(source: &str) -> anyhow::Result<Symbols> {
        CodeSymbolsParser::new(&tree_sitter_rust::LANGUAGE.into(), Rust)?.parse(source)
    }

    #[test]
    fn file_with_every_shape_parse_symbols_returns_every_symbol_in_order() -> anyhow::Result<()> {
        let source = r#"//! Doc.
#![allow(dead_code)]

/// Doc.
pub(crate) const KEY: &str = "L$x";
const MAX_MESSAGE_SIZE: usize = 20 * 1024 * 1024;
static NAMES: [&str; 2] = ["a", "b"];
static mut COUNTER: u32 = 0;
const WRAPPED: u8 = (32);
const _: () = ();

/// Doc.
#[cfg(unix)]
/// Doc.
#[cfg_attr(test, allow(dead_code))]
const PLATFORM: &str = "unix";
#[cfg(windows)]
const PLATFORM: &str = "windows";

use std::fmt;
extern crate alloc;

#[derive(Debug)]
pub enum LogLevel {
    Error,
}
struct Unit;
struct Pair<T>(T, T);
union Bits {
    a: u8,
}
type Alias<T> = Vec<T>;
trait Named {}
fn run() {}
pub async unsafe fn fetch() {}
const fn size() -> usize {
    1
}
macro_rules! square {
    ($x:expr) => {
        $x * $x
    };
}
#[cfg(test)]
mod tests {
    const INNER: u8 = 1;
}
mod file;
impl Unit {
    fn new() {}
}
impl Named for Unit {}
extern "C" {
    fn external();
    static EXTERNAL: u8;
}
lazy_static! {
    static ref LAZY: u8 = 1;
}
"#;

        // <block affects="docs/symbols.md:rust-paths, src/skill.md#/blockwatch/Symbols/Rust">
        assert_eq!(
            symbols(&rust(source)?, source),
            vec![
                symbol(
                    "/KEY",
                    r#"pub(crate) const KEY: &str = "L$x";"#,
                    Some("L$x")
                ),
                symbol(
                    "/MAX_MESSAGE_SIZE",
                    "const MAX_MESSAGE_SIZE: usize = 20 * 1024 * 1024;",
                    Some("20 * 1024 * 1024")
                ),
                symbol(
                    "/NAMES",
                    r#"static NAMES: [&str; 2] = ["a", "b"];"#,
                    Some(r#"["a", "b"]"#)
                ),
                symbol("/COUNTER", "static mut COUNTER: u32 = 0;", Some("0")),
                symbol("/WRAPPED", "const WRAPPED: u8 = (32);", Some("32")),
                symbol(
                    "/PLATFORM",
                    "#[cfg(unix)]\n/// Doc.\n#[cfg_attr(test, allow(dead_code))]\nconst PLATFORM: &str = \"unix\";",
                    Some("unix")
                ),
                symbol(
                    "/PLATFORM",
                    "#[cfg(windows)]\nconst PLATFORM: &str = \"windows\";",
                    Some("windows")
                ),
                symbol(
                    "/LogLevel",
                    "#[derive(Debug)]\npub enum LogLevel {\n    Error,\n}",
                    None
                ),
                symbol("/Unit", "struct Unit;", None),
                symbol("/Pair", "struct Pair<T>(T, T);", None),
                symbol("/Bits", "union Bits {\n    a: u8,\n}", None),
                symbol("/Alias", "type Alias<T> = Vec<T>;", None),
                symbol("/Named", "trait Named {}", None),
                symbol("/run", "fn run() {}", None),
                symbol("/fetch", "pub async unsafe fn fetch() {}", None),
                symbol("/size", "const fn size() -> usize {\n    1\n}", None),
                symbol(
                    "/square",
                    "macro_rules! square {\n    ($x:expr) => {\n        $x * $x\n    };\n}",
                    None
                ),
                symbol(
                    "/tests",
                    "#[cfg(test)]\nmod tests {\n    const INNER: u8 = 1;\n}",
                    None
                ),
            ]
        );
        // </block>
        Ok(())
    }

    #[test]
    fn string_kinds_parse_symbols_returns_decoded_values() -> anyhow::Result<()> {
        let source = r##"const ESCAPES: &str = "a\n\r\t\\\0\'\"\x41\u{e9}\u{1F600}";
const SPLIT: &str = "a\
    b";
const SPLIT_BLANK: &str = "a\

	  b";
const SPLIT_ESCAPE: &str = "a\
    \t  b";
const RAW: &str = r"a\n";
const RAW_HASH: &str = r#"a "b" \n"#;
const BYTES: &[u8] = b"a\n";
const RAW_BYTES: &[u8] = br"a";
const BYTE: u8 = b'a';
const C_STRING: &CStr = c"a";
const RAW_C_STRING: &CStr = cr"a";
const CHAR: char = 'a';
const CONCAT: &str = concat!("a", "b");
"##;

        assert_eq!(
            values(&rust(source)?, source),
            vec![
                value("/ESCAPES", "a\n\r\t\\\0'\"Aé😀"),
                // A backslash at the end of a line skips the white space after it, line ends too.
                value("/SPLIT", "ab"),
                value("/SPLIT_BLANK", "ab"),
                // An escape ends the white space to skip.
                value("/SPLIT_ESCAPE", "a\t  b"),
                value("/RAW", r"a\n"),
                value("/RAW_HASH", r#"a "b" \n"#),
                value("/BYTES", r#"b"a\n""#),
                value("/RAW_BYTES", r#"br"a""#),
                value("/BYTE", "b'a'"),
                value("/C_STRING", r#"c"a""#),
                value("/RAW_C_STRING", r#"cr"a""#),
                value("/CHAR", "'a'"),
                value("/CONCAT", r#"concat!("a", "b")"#),
            ]
        );
        Ok(())
    }

    #[test]
    fn crlf_line_endings_parse_symbols_reads_line_feeds() -> anyhow::Result<()> {
        // Rust reads every line end in a file as a line feed.
        let source = "const TEXT: &str = \"a\r\nb\";\r\nconst RAW: &str = r\"a\r\nb\";\r\nconst SPLIT: &str = \"a\\\r\n\r\n  b\";\r\n";

        assert_eq!(
            values(&rust(source)?, source),
            vec![
                value("/TEXT", "a\nb"),
                value("/RAW", "a\nb"),
                value("/SPLIT", "ab"),
            ]
        );
        Ok(())
    }

    #[test]
    fn escape_that_is_not_text_parse_symbols_makes_the_name_unreadable() -> anyhow::Result<()> {
        let source = r#"const UNKNOWN: &str = "\d";
const SURROGATE: &str = "\u{D800}";
const TOO_BIG: &str = "\u{110000}";
const FINE: &str = "ok";
"#;

        assert_eq!(
            symbols(&rust(source)?, source),
            vec![symbol("/FINE", r#"const FINE: &str = "ok";"#, Some("ok"))]
        );
        assert_eq!(
            unresolved(&rust(source)?, source, "/UNKNOWN")?,
            decode_error(1, 23, r"`\d` stands for no character")
        );
        assert_eq!(
            unresolved(&rust(source)?, source, "/SURROGATE")?,
            decode_error(2, 25, r"`\u{D800}` stands for no character")
        );
        assert_eq!(
            unresolved(&rust(source)?, source, "/TOO_BIG")?,
            decode_error(3, 23, r"`\u{110000}` stands for no character")
        );
        Ok(())
    }

    #[test]
    fn syntax_errors_parse_symbols_makes_the_names_near_them_unreadable() -> anyhow::Result<()> {
        let source = r#"const PORTS: u8 = 8;
fn helper( {
    1
}
const BROKEN: u8 = = 1;
const AFTER: u8 = 3;
"#;

        assert_eq!(
            symbols(&rust(source)?, source),
            vec![
                symbol("/PORTS", "const PORTS: u8 = 8;", Some("8")),
                symbol("/AFTER", "const AFTER: u8 = 3;", Some("3")),
            ]
        );
        assert_eq!(
            unresolved(&rust(source)?, source, "/helper")?,
            syntax_error(2, 11, "helper")
        );
        assert_eq!(
            unresolved(&rust(source)?, source, "/BROKEN")?,
            syntax_error(5, 20, "BROKEN")
        );
        Ok(())
    }

    #[test]
    fn span_that_swallows_definitions_parse_symbols_makes_their_names_unreadable()
    -> anyhow::Result<()> {
        // The unclosed `[` makes the parser read the lines after it as part of the array,
        // `AFTER` included.
        let source = r#"const PORTS: u8 = 8;
const CONFIG: [u8; 1] = [
    1,

const AFTER: u8 = 3;
"#;

        assert_eq!(
            symbols(&rust(source)?, source),
            vec![symbol("/PORTS", "const PORTS: u8 = 8;", Some("8"))]
        );
        for name in ["CONFIG", "AFTER"] {
            assert_eq!(
                unresolved(&rust(source)?, source, &format!("/{name}"))?,
                syntax_error(5, 1, name)
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Position, language_parsers::Comment};

    #[test]
    fn parses_comments_correctly() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser()?;

        let blocks: Vec<Comment> = comments_parser
            .parse(
                r#"
        //! This is a crate-level documentation comment.
        //! It provides an overview of the module or library.
        
        /// This function adds two numbers.
        ///
        /// Returns the sum of `a` and `b`.
        fn add(a: i32, b: i32) -> i32 {
            a + b
        }
        
        fn main() {
            // This is a single-line comment.
            println!("Hello, Rust!");
        
            /* 
               This is a block comment.
               It can span multiple lines. */
            
            let x = 10;
            let y = 20;
            
            println!("{} + {} = {}", x, y, add(x, y)); // Using the add function.
        }
        "#,
            )
            .collect();

        assert_eq!(
            blocks,
            vec![
                Comment {
                    position_range: Position::new(2, 9)..Position::new(3, 1),
                    source_range: 9..58,
                    comment_text: "    This is a crate-level documentation comment.\n".to_string()
                },
                Comment {
                    position_range: Position::new(3, 9)..Position::new(4, 1),
                    source_range: 66..120,
                    comment_text: "    It provides an overview of the module or library.\n".to_string()
                },
                Comment {
                    position_range: Position::new(5, 9)..Position::new(6, 1),
                    source_range: 137..173, // TODO: incorrect?
                    comment_text: "    This function adds two numbers.\n".to_string()
                },
                Comment {
                    position_range: Position::new(6, 9)..Position::new(7, 1),
                    source_range: 181..185,
                    comment_text: "   \n".to_string()
                },
                Comment {
                    position_range: Position::new(7, 9)..Position::new(8, 1),
                    source_range: 193..229,
                    comment_text: "    Returns the sum of `a` and `b`.\n".to_string()
                },
                Comment {
                    position_range: Position::new(13, 13)..Position::new(13, 46),
                    source_range: 338..371,
                    comment_text: "   This is a single-line comment.".to_string()
                },
                Comment {
                    position_range: Position::new(16, 13)..Position::new(18, 46),
                    source_range: 431..520,
                    comment_text: "   \n               This is a block comment.\n               It can span multiple lines.   "
                        .to_string()
                },
                Comment {
                    position_range: Position::new(23, 56)..Position::new(23, 82),
                    source_range: 650..676,
                    comment_text: "   Using the add function.".to_string()
                }
            ]
        );

        Ok(())
    }
}
