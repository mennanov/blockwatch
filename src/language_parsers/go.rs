use crate::language_parsers;
use crate::language_parsers::{
    CommentsParser, LanguageParser, LanguageParserImpl, node_text, tree_sitter_parser,
};
use crate::symbols::code::{self, Definition, Value};
use crate::symbols::{Symbols, SymbolsParser};
use anyhow::{Context, anyhow};
use std::ops::Range;
use tree_sitter::Node;

/// Returns a [`LanguageParser`] for Go source files, with their symbols.
pub(super) fn parser() -> anyhow::Result<impl LanguageParser> {
    Ok(LanguageParserImpl::new(comments_parser()?).with_symbols(GoSymbolsParser::new()?))
}

/// Returns a [`LanguageParser`] for `go.mod`, `go.sum` and `go.work`. They have Go's comments,
/// but not its declarations, so they have no symbols.
pub(super) fn module_files_parser() -> anyhow::Result<impl LanguageParser> {
    Ok(LanguageParserImpl::new(comments_parser()?))
}

/// Finds the top-level names of a Go file: constants, variables, types and functions.
struct GoSymbolsParser {
    parser: tree_sitter::Parser,
}

impl GoSymbolsParser {
    /// # Errors
    /// Returns an error if the Go grammar cannot be loaded.
    fn new() -> anyhow::Result<Self> {
        let parser = tree_sitter_parser(&tree_sitter_go::LANGUAGE.into())?;
        Ok(Self { parser })
    }
}

impl SymbolsParser for GoSymbolsParser {
    fn parse(&mut self, source: &str) -> anyhow::Result<Symbols> {
        let tree = self
            .parser
            .parse(source, None)
            .context("failed to parse syntax tree")?;
        let definitions = definitions(tree.root_node(), source);
        Ok(code::symbols(&tree, source, &definitions, decode_string))
    }
}

/// The top-level definitions under `root`, the root of a Go file's syntax tree, in document
/// order. `source` is the text of the file.
fn definitions<'tree>(root: Node<'tree>, source: &str) -> Vec<Definition<'tree>> {
    let mut cursor = root.walk();
    root.named_children(&mut cursor)
        .flat_map(|node| match node.kind() {
            "const_declaration" | "var_declaration" | "type_declaration" => {
                declaration_definitions(node, source)
            }
            "function_declaration" => function_definition(node, source).into_iter().collect(),
            // A method belongs to its type, so it gets a path under the type, not one of its own.
            _ => Vec::new(),
        })
        .collect()
}

/// The definitions of `declaration`, a `const`, `var` or `type` declaration, with one for each
/// name it declares.
fn declaration_definitions<'tree>(
    declaration: Node<'tree>,
    source: &str,
) -> Vec<Definition<'tree>> {
    let mut cursor = declaration.walk();
    let children: Vec<Node> = declaration.children(&mut cursor).collect();
    // A `var` group keeps its lines in a node of their own.
    let list = children
        .iter()
        .copied()
        .find(|child| child.kind() == "var_spec_list");
    // In a group, such as `const ( ... )`, each line declares its own names. So a change to one
    // line does not touch the names on the others.
    let grouped = list.is_some() || children.iter().any(|child| child.kind() == "(");
    let specs = match list {
        Some(list) => list.named_children(&mut cursor).collect(),
        None => children,
    };
    specs
        .into_iter()
        .filter(|spec| {
            matches!(
                spec.kind(),
                "const_spec" | "var_spec" | "type_spec" | "type_alias"
            )
        })
        .flat_map(|spec| {
            let range = if grouped {
                spec.byte_range()
            } else {
                declaration.byte_range()
            };
            spec_definitions(spec, range, source)
        })
        .collect()
}

/// The definitions of `spec`, one for each name it declares, each at `range`.
fn spec_definitions<'tree>(
    spec: Node<'tree>,
    range: Range<usize>,
    source: &str,
) -> Vec<Definition<'tree>> {
    let mut cursor = spec.walk();
    // The field also holds the commas between the names.
    let names: Vec<Node> = spec
        .children_by_field_name("name", &mut cursor)
        .filter(|name| name.is_named())
        .collect();
    let values: Vec<Node> = match spec.child_by_field_name("value") {
        Some(list) => {
            let mut cursor = list.walk();
            list.named_children(&mut cursor)
                .filter(|value| !value.is_extra())
                .collect()
        }
        None => Vec::new(),
    };
    // Values pair with names by position. In `var x, y = f()`, one call gives both names, so
    // neither has a value of its own.
    let paired = values.len() == names.len();
    names
        .iter()
        .enumerate()
        // `_` declares nothing that code can refer to.
        .filter(|(_, name)| node_text(name, source) != "_")
        .map(|(index, name)| Definition {
            names: vec![*name],
            range: range.clone(),
            value: paired.then(|| Value::Node(values[index])),
        })
        .collect()
}

/// The definition of `function`, or `None` when no code can refer to it by name: `init` and `_`.
fn function_definition<'tree>(function: Node<'tree>, source: &str) -> Option<Definition<'tree>> {
    let name = function.child_by_field_name("name")?;
    match node_text(&name, source) {
        "init" | "_" => None,
        _ => Some(Definition {
            names: vec![name],
            range: function.byte_range(),
            value: None,
        }),
    }
}

/// Decodes a Go string literal, following Go's escapes. Returns `None` for any other node.
///
/// # Errors
/// Returns an error if an escape is not one Go has, stands for no character, or makes bytes that
/// are not UTF-8 text.
fn decode_string(node: &Node, source: &str) -> anyhow::Result<Option<String>> {
    match node.kind() {
        "interpreted_string_literal" => decode_interpreted_string(node, source).map(Some),
        "raw_string_literal" => {
            let text = node_text(node, source);
            let content = text
                .strip_prefix('`')
                .and_then(|text| text.strip_suffix('`'))
                .context("a raw string is between backquotes")?;
            // Go drops each carriage return in a raw string, so a file saved with CRLF line
            // endings gives the same value.
            Ok(Some(content.replace('\r', "")))
        }
        _ => Ok(None),
    }
}

/// Decodes `node`, a Go string in double quotes.
///
/// # Errors
/// Returns an error if an escape is not one Go has, stands for no character, or makes bytes that
/// are not UTF-8 text.
fn decode_interpreted_string(node: &Node, source: &str) -> anyhow::Result<String> {
    // `\x` and octal escapes give bytes, not characters, so the text is built as bytes. Two of
    // them can make one character together: `\xc3\xa9` is `é`.
    let mut bytes = Vec::new();
    let mut cursor = node.walk();
    for part in node.named_children(&mut cursor) {
        let text = node_text(&part, source);
        match part.kind() {
            "escape_sequence" => push_escape(text, &mut bytes)?,
            _ => bytes.extend_from_slice(text.as_bytes()),
        }
    }
    String::from_utf8(bytes).map_err(|_| anyhow!("its bytes are not UTF-8 text"))
}

/// Adds the bytes that `escape`, such as `\n` or `\x41`, stands for to `bytes`.
///
/// # Errors
/// Returns an error if `escape` is not one Go has, or stands for no character.
fn push_escape(escape: &str, bytes: &mut Vec<u8>) -> anyhow::Result<()> {
    let not_an_escape = || anyhow!("`{escape}` is not an escape in a Go string");
    let body = escape.strip_prefix('\\').ok_or_else(not_an_escape)?;
    let mut chars = body.chars();
    let kind = chars.next().ok_or_else(not_an_escape)?;
    let digits = chars.as_str();
    let byte = match kind {
        'a' => 0x07,
        'b' => 0x08,
        'f' => 0x0c,
        'n' => b'\n',
        'r' => b'\r',
        't' => b'\t',
        'v' => 0x0b,
        '\\' => b'\\',
        '"' => b'"',
        // Go needs all three digits: `\07` doesn't compile, though the grammar reads it as an
        // escape.
        '0'..='7' if body.len() == 3 => u8::from_str_radix(body, 8).map_err(|_| not_an_escape())?,
        'x' => u8::from_str_radix(digits, 16).map_err(|_| not_an_escape())?,
        'u' | 'U' => {
            let code = u32::from_str_radix(digits, 16).map_err(|_| not_an_escape())?;
            let character = char::from_u32(code)
                .ok_or_else(|| anyhow!("`{escape}` stands for no character"))?;
            bytes.extend_from_slice(character.encode_utf8(&mut [0; 4]).as_bytes());
            return Ok(());
        }
        _ => return Err(not_an_escape()),
    };
    bytes.push(byte);
    Ok(())
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let go_language = tree_sitter_go::LANGUAGE.into();
    let parser = language_parsers::c_style_comments_parser(&go_language, "comment")?;
    Ok(parser)
}

#[cfg(test)]
mod symbols_tests {
    use super::*;
    use crate::symbols::code::test_utils::{self, decode_error, symbol, syntax_error};

    /// Each symbol of `source`, as [`test_utils::symbols`] lists it.
    fn symbols(source: &str) -> anyhow::Result<Vec<(String, String, Option<String>)>> {
        Ok(test_utils::symbols(
            &parser()?.parse_symbols(source)?,
            source,
        ))
    }

    /// Why a reference to `path` in `source` does not resolve.
    fn unresolved(source: &str, path: &str) -> anyhow::Result<String> {
        test_utils::unresolved(&parser()?.parse_symbols(source)?, source, path)
    }

    #[test]
    fn file_with_every_go_shape_parse_symbols_returns_every_symbol_in_order() -> anyhow::Result<()>
    {
        let source = r#"package p

import "fmt"

// Doc.
const ExprStatusBits = 4

const (
	DNSPortsMax = 8
	A, B = 1, "x"
	C, D = 1, /* note */ 2
	Mode0 Mode = iota
	Mode1
)

const X Mode = 3
var x, y = f()
var Raw = `a
b`
var R = 'r'
var Z int
var (
	V1 = (20 * 1024)
)
var _ = 1
type Server struct{ a int }
type Alias = Server
type List[T any] []T
type (
	T1 int
)
func Helper() {}
func (s *Server) Start() {}
func init() {}
func _() {}
"#;

        // <block affects="docs/symbols.md:go-paths, src/skill.md#/blockwatch/Symbols/Go">
        assert_eq!(
            symbols(source)?,
            vec![
                symbol("/ExprStatusBits", "const ExprStatusBits = 4", Some("4")),
                symbol("/DNSPortsMax", "DNSPortsMax = 8", Some("8")),
                symbol("/A", r#"A, B = 1, "x""#, Some("1")),
                symbol("/B", r#"A, B = 1, "x""#, Some("x")),
                symbol("/C", "C, D = 1, /* note */ 2", Some("1")),
                symbol("/D", "C, D = 1, /* note */ 2", Some("2")),
                symbol("/Mode0", "Mode0 Mode = iota", Some("iota")),
                symbol("/Mode1", "Mode1", None),
                symbol("/X", "const X Mode = 3", Some("3")),
                symbol("/x", "var x, y = f()", None),
                symbol("/y", "var x, y = f()", None),
                symbol("/Raw", "var Raw = `a\nb`", Some("a\nb")),
                symbol("/R", "var R = 'r'", Some("'r'")),
                symbol("/Z", "var Z int", None),
                symbol("/V1", "V1 = (20 * 1024)", Some("20 * 1024")),
                symbol("/Server", "type Server struct{ a int }", None),
                symbol("/Alias", "type Alias = Server", None),
                symbol("/List", "type List[T any] []T", None),
                symbol("/T1", "T1 int", None),
                symbol("/Helper", "func Helper() {}", None),
            ]
        );
        // </block>
        Ok(())
    }

    #[test]
    fn string_kinds_parse_symbols_returns_decoded_values() -> anyhow::Result<()> {
        let source = r#"package p

var Escapes = "a\a\b\f\n\r\t\v\\\"\101\x41\u00e9\U0001F600\xc3\xa9"
var Raw = `a\n`
"#;

        let values: Vec<(String, Option<String>)> = symbols(source)?
            .into_iter()
            .map(|(path, _, value)| (path, value))
            .collect();

        assert_eq!(
            values,
            vec![
                (
                    "/Escapes".to_string(),
                    Some("a\u{7}\u{8}\u{c}\n\r\t\u{b}\\\"AAé😀é".to_string())
                ),
                ("/Raw".to_string(), Some(r"a\n".to_string())),
            ]
        );
        Ok(())
    }

    #[test]
    fn raw_string_with_carriage_returns_parse_symbols_removes_them() -> anyhow::Result<()> {
        let source = "package p\nvar Raw = `a\r\nb`\n";

        assert_eq!(
            symbols(source)?,
            vec![symbol("/Raw", "var Raw = `a\r\nb`", Some("a\nb"))]
        );
        Ok(())
    }

    #[test]
    fn escape_that_is_not_text_parse_symbols_makes_the_name_unreadable() -> anyhow::Result<()> {
        let source = r#"package p

var NotText = "\xff"
var NoEscape = "\'"
var Surrogate = "\ud800"
var ShortOctal = "\07"
var Fine = "ok"
"#;

        assert_eq!(
            symbols(source)?,
            vec![symbol("/Fine", r#"var Fine = "ok""#, Some("ok"))]
        );
        assert_eq!(
            unresolved(source, "/NotText")?,
            decode_error(3, 15, "its bytes are not UTF-8 text")
        );
        assert_eq!(
            unresolved(source, "/NoEscape")?,
            decode_error(4, 16, r"`\'` is not an escape in a Go string")
        );
        assert_eq!(
            unresolved(source, "/Surrogate")?,
            decode_error(5, 17, r"`\ud800` stands for no character")
        );
        assert_eq!(
            unresolved(source, "/ShortOctal")?,
            decode_error(6, 18, r"`\07` is not an escape in a Go string")
        );
        Ok(())
    }

    #[test]
    fn syntax_errors_parse_symbols_makes_the_names_near_them_unreadable() -> anyhow::Result<()> {
        // The parser skips the `+` in `F`, and the `B =` after `X =`. So `X` would read the value
        // of `B`.
        let source = r#"package p

const A = 1

func F() int { return 1 + }

const (
	C = 3
	X =
	B = 2
)
"#;

        assert_eq!(
            symbols(source)?,
            vec![
                symbol("/A", "const A = 1", Some("1")),
                symbol("/C", "C = 3", Some("3")),
            ]
        );
        assert_eq!(unresolved(source, "/F")?, syntax_error(5, 25, "F"));
        assert_eq!(unresolved(source, "/X")?, syntax_error(10, 2, "X"));
        assert_eq!(unresolved(source, "/B")?, syntax_error(10, 2, "B"));
        Ok(())
    }

    #[test]
    fn span_that_swallows_definitions_parse_symbols_makes_their_names_unreadable()
    -> anyhow::Result<()> {
        // The unclosed `{` makes the parser skip everything after it, `Y` and `B` included.
        let source = r#"package p

const A = 1

var X = []int{1, 2
const Y = 2

const B = 3
"#;

        assert_eq!(
            symbols(source)?,
            vec![symbol("/A", "const A = 1", Some("1"))]
        );
        for name in ["X", "Y", "B"] {
            assert_eq!(
                unresolved(source, &format!("/{name}"))?,
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
    fn parses_go_comments_correctly() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser()?;

        let blocks: Vec<Comment> = comments_parser
            .parse(
                r#"
// This is a single line comment in Go
package main

/*
This is a multi-line comment
spanning several lines
*/

import "fmt"

func main() {
    fmt.Println("Hello, World!") // Inline comment
    // Another single line comment
}
/// Triple slash comment.
"#,
            )
            .collect();

        assert_eq!(
            blocks,
            vec![
                Comment {
                    position_range: Position::new(2, 1)..Position::new(2, 39),
                    source_range: 1..39,
                    comment_text: "   This is a single line comment in Go".to_string()
                },
                Comment {
                    position_range: Position::new(5, 1)..Position::new(8, 3),
                    source_range: 54..111,
                    comment_text: "  \nThis is a multi-line comment\nspanning several lines\n  "
                        .to_string()
                },
                Comment {
                    position_range: Position::new(13, 34)..Position::new(13, 51),
                    source_range: 174..191,
                    comment_text: "   Inline comment".to_string()
                },
                Comment {
                    position_range: Position::new(14, 5)..Position::new(14, 35),
                    source_range: 196..226,
                    comment_text: "   Another single line comment".to_string()
                },
                Comment {
                    position_range: Position::new(16, 1)..Position::new(16, 26),
                    source_range: 229..254,
                    comment_text: "  / Triple slash comment.".to_string()
                },
            ]
        );

        Ok(())
    }

    #[test]
    fn parses_go_mod_comments_correctly() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser()?;

        let blocks: Vec<Comment> = comments_parser
            .parse(
                r#"
module example.com/my/module

go 1.21

// This is a comment in go.mod
require (
    github.com/some/dependency v1.2.3 // Inline comment
)

/*
Multi-line
comment
*/
// Another comment
exclude github.com/bad/dependency v0.0.0
"#,
            )
            .collect();

        assert_eq!(
            blocks,
            vec![
                Comment {
                    position_range: Position::new(6, 1)..Position::new(6, 31),
                    source_range: 40..70,
                    comment_text: "   This is a comment in go.mod".to_string()
                },
                Comment {
                    position_range: Position::new(8, 39)..Position::new(8, 56),
                    source_range: 119..136,
                    comment_text: "   Inline comment".to_string()
                },
                Comment {
                    position_range: Position::new(11, 1)..Position::new(14, 3),
                    source_range: 140..164,
                    comment_text: "  \nMulti-line\ncomment\n  ".to_string()
                },
                Comment {
                    position_range: Position::new(15, 1)..Position::new(15, 19),
                    source_range: 165..183,
                    comment_text: "   Another comment".to_string()
                },
            ]
        );

        Ok(())
    }

    #[test]
    fn parses_go_work_comments_correctly() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser()?;

        let blocks: Vec<Comment> = comments_parser
            .parse(
                r#"
go 1.21

// This is a comment in go.work
use (
    ./module1
    ./module2 // Inline comment
)

/*
Multi-line
workspace comment
*/
// Another comment
use ./module3
"#,
            )
            .collect();

        assert_eq!(
            blocks,
            vec![
                Comment {
                    position_range: Position::new(4, 1)..Position::new(4, 32),
                    source_range: 10..41,
                    comment_text: "   This is a comment in go.work".to_string()
                },
                Comment {
                    position_range: Position::new(7, 15)..Position::new(7, 32),
                    source_range: 76..93,
                    comment_text: "   Inline comment".to_string()
                },
                Comment {
                    position_range: Position::new(10, 1)..Position::new(13, 3),
                    source_range: 97..131,
                    comment_text: "  \nMulti-line\nworkspace comment\n  ".to_string()
                },
                Comment {
                    position_range: Position::new(14, 1)..Position::new(14, 19),
                    source_range: 132..150,
                    comment_text: "   Another comment".to_string()
                },
            ]
        );

        Ok(())
    }

    #[test]
    fn parses_go_sum_comments_correctly() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser()?;

        let blocks: Vec<Comment> = comments_parser
            .parse(
                r#"
// This is a comment in go.sum
github.com/example/pkg v1.0.0 h1:abc123
github.com/example/pkg v1.0.0/go.mod h1:def456 // Hash comment

/*
Multi-line comment
in go.sum
*/
// Final comment
github.com/another/pkg v2.0.0 h1:xyz789
"#,
            )
            .collect();

        assert_eq!(
            blocks,
            vec![
                Comment {
                    position_range: Position::new(2, 1)..Position::new(2, 31),
                    source_range: 1..31,
                    comment_text: "   This is a comment in go.sum".to_string()
                },
                Comment {
                    position_range: Position::new(4, 48)..Position::new(4, 63),
                    source_range: 119..134,
                    comment_text: "   Hash comment".to_string()
                },
                Comment {
                    position_range: Position::new(6, 1)..Position::new(9, 3),
                    source_range: 136..170,
                    comment_text: "  \nMulti-line comment\nin go.sum\n  ".to_string()
                },
                Comment {
                    position_range: Position::new(10, 1)..Position::new(10, 17),
                    source_range: 171..187,
                    comment_text: "   Final comment".to_string()
                },
            ]
        );

        Ok(())
    }
}
