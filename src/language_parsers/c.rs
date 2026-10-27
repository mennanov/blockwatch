use crate::language_parsers;
use crate::language_parsers::{
    CommentsParser, LanguageParser, LanguageParserImpl, node_text, tree_sitter_parser,
};
use crate::symbols::code::{self, Definition, Value};
use crate::symbols::{ScalarValue, Symbols, SymbolsParser};
use crate::{Position, character_column_at};
use anyhow::{Context, anyhow};
use std::borrow::Cow;
use tree_sitter::{Language, Node};

/// Returns a [`LanguageParser`] for C, with its symbols.
pub(super) fn parser() -> anyhow::Result<impl LanguageParser> {
    Ok(LanguageParserImpl::new(comments_parser()?)
        .with_symbols(CSymbolsParser::new(&tree_sitter_c::LANGUAGE.into())?))
}

/// Finds the top-level names of a C or C++ file: macros, variables, functions, types, and the
/// enumerators of a plain `enum`.
pub(super) struct CSymbolsParser {
    parser: tree_sitter::Parser,
}

impl CSymbolsParser {
    /// A parser for files in `language`, the C or the C++ grammar.
    ///
    /// # Errors
    /// Returns an error if the grammar cannot be loaded.
    pub(super) fn new(language: &Language) -> anyhow::Result<Self> {
        let parser = tree_sitter_parser(language)?;
        Ok(Self { parser })
    }
}

impl SymbolsParser for CSymbolsParser {
    fn parse(&mut self, source: &str) -> anyhow::Result<Symbols> {
        let tree = self
            .parser
            .parse(fixed_directive_line_ends(source).as_ref(), None)
            .context("failed to parse syntax tree")?;
        let definitions = definitions(tree.root_node(), source, &mut self.parser)?;
        Ok(code::symbols(&tree, source, &definitions, &decode_string))
    }
}

/// `source` with the end of each line of a preprocessor directive changed, where the grammar
/// would read on into the next line. A `/` at the end, other than the `/` of a `*/`, becomes
/// `_`. The spaces and tabs before the line end move after it. Every other byte keeps its place.
fn fixed_directive_line_ends(source: &str) -> Cow<'_, str> {
    // The grammar reads a `/` and the line end after it as part of a macro's body, so
    // `#define A 1 // https://a.b/` would take in the next line, and the macro on it. A space
    // after an empty macro, as in `#define GUARD_H `, does the same.
    let mut text = Cow::Borrowed(source);
    let mut in_directive = false;
    let mut start = 0;
    for line in source.split_inclusive('\n') {
        let content = line.trim_end_matches(['\r', '\n']);
        let code = content.trim_end_matches([' ', '\t']);
        in_directive = in_directive || code.trim_start().starts_with('#');
        if in_directive && code.ends_with('/') && !code.ends_with("*/") {
            let slash = start + code.len() - 1;
            text.to_mut().replace_range(slash..=slash, "_");
        }
        let spaces = content.strip_prefix(code).unwrap_or_default();
        let line_end = line.strip_prefix(content).unwrap_or_default();
        if in_directive && !spaces.is_empty() {
            let spaces_start = start + code.len();
            text.to_mut().replace_range(
                spaces_start..start + line.len(),
                &format!("{line_end}{spaces}"),
            );
        }
        // A backslash at the end of a line continues the directive on the next one.
        in_directive = in_directive && code.ends_with('\\');
        start += line.len();
    }
    text
}

/// Where the walk is, for the children of a node.
enum Level {
    /// At the top level, where each child's definition starts where the child does.
    Top,
    /// At the top level, inside a wrapper such as `template <...>` or `extern "C"`, where a
    /// definition starts with the wrapper.
    Wrapped(usize),
    /// Below the top level, where only macros count.
    Nested,
}

/// The definitions in `root`, the root of a C or C++ file's syntax tree, in document order.
/// `source` is the text of the file, and `parser` parses its language.
///
/// # Errors
/// Returns an error if `parser` fails to parse the body of a macro.
fn definitions<'tree>(
    root: Node<'tree>,
    source: &str,
    parser: &mut tree_sitter::Parser,
) -> anyhow::Result<Vec<Definition<'tree>>> {
    let mut definitions = Vec::new();
    // The walk keeps its own stack, as deeply nested source could overflow the call stack. Each
    // node comes with where its definition starts, or `None` below the top level.
    let mut pending = vec![(root, Some(root.start_byte()))];
    let mut cursor = root.walk();
    while let Some((node, start)) = pending.pop() {
        let level = match (node.kind(), start) {
            ("preproc_def" | "preproc_function_def", _) => {
                definitions.extend(macro_definition(node, source, parser)?);
                continue;
            }
            // The preprocessor keeps a macro until the end of the file, wherever it is defined.
            // So the walk enters every node, but only these keep the top level.
            (_, None) => Level::Nested,
            (
                "translation_unit" | "declaration_list" | "preproc_if" | "preproc_ifdef"
                | "preproc_else" | "preproc_elif" | "preproc_elifdef",
                Some(_),
            ) => Level::Top,
            ("template_declaration" | "linkage_specification", Some(start)) => {
                Level::Wrapped(start)
            }
            (_, Some(start)) => {
                definitions.extend(top_level_definitions(node, start, source));
                Level::Nested
            }
        };
        let first = pending.len();
        pending.extend(node.children(&mut cursor).map(|child| {
            let start = match level {
                Level::Top => Some(child.start_byte()),
                Level::Wrapped(start) => Some(start),
                Level::Nested => None,
            };
            (child, start)
        }));
        // The stack takes the last child first, so the children go on it in reverse.
        pending[first..].reverse();
    }
    // A macro inside a type comes after the type's declarators in the walk.
    definitions.sort_by_key(|definition| definition.range.start);
    Ok(definitions)
}

/// The definitions that `node`, a node at the top level, makes. Each one starts at `start`.
fn top_level_definitions<'tree>(
    node: Node<'tree>,
    start: usize,
    source: &str,
) -> Vec<Definition<'tree>> {
    match node.kind() {
        "declaration" => declaration_definitions(node, start),
        "type_definition" => typedef_definitions(node, start, source),
        "function_definition" => function_definition(node, start).into_iter().collect(),
        "struct_specifier" | "union_specifier" | "class_specifier" | "enum_specifier" => {
            type_definitions(node, start)
        }
        "alias_declaration" => node
            .child_by_field_name("name")
            .map(|name| Definition {
                names: vec![name],
                range: start..node.end_byte(),
                value: None,
            })
            .into_iter()
            .collect(),
        _ => Vec::new(),
    }
}

/// The definitions of `declaration`, such as `static const int E = 7;`, which starts at `start`:
/// the type it defines, if any, and each name it declares.
fn declaration_definitions(declaration: Node<'_>, start: usize) -> Vec<Definition<'_>> {
    let mut definitions: Vec<Definition> = declaration
        .child_by_field_name("type")
        .into_iter()
        .flat_map(|specifier| type_definitions(specifier, specifier.start_byte()))
        .collect();
    let mut cursor = declaration.walk();
    let is_extern = declaration.children(&mut cursor).any(|child| {
        child.kind() == "storage_class_specifier"
            && child
                .child(0)
                .is_some_and(|keyword| keyword.kind() == "extern")
    });
    let declarators: Vec<Node> = declaration
        .children_by_field_name("declarator", &mut cursor)
        .collect();
    // In `int a = 1, b = 2;`, each name has its own part, so a change to one does not touch the
    // other.
    let alone = declarators.len() == 1;
    for declarator in declarators {
        let Some((name, is_function)) = declared_name(declarator) else {
            continue;
        };
        let value = match declarator.kind() {
            "init_declarator" => declarator.child_by_field_name("value"),
            _ => None,
        };
        // Without a value, a function or an `extern` variable is defined elsewhere.
        if value.is_none() && (is_function || is_extern) {
            continue;
        }
        definitions.push(Definition {
            names: vec![name],
            range: if alone {
                start..declaration.end_byte()
            } else {
                declarator.byte_range()
            },
            value: value.map(Value::Node),
        });
    }
    definitions
}

/// The definitions of `typedef`, which starts at `start`. Its names, such as `foo` and `foo_t`
/// in `typedef struct foo {...} foo_t;`, make one definition. The enumerators of a plain `enum`
/// make their own.
fn typedef_definitions<'tree>(
    typedef: Node<'tree>,
    start: usize,
    source: &str,
) -> Vec<Definition<'tree>> {
    let specifier = typedef.child_by_field_name("type");
    let mut names: Vec<Node> = specifier.and_then(defined_type_name).into_iter().collect();
    let mut cursor = typedef.walk();
    names.extend(
        typedef
            .children_by_field_name("declarator", &mut cursor)
            .filter_map(declared_name)
            .map(|(name, _)| name),
    );
    // `typedef struct foo {...} foo;` writes one name twice, for one type.
    let mut seen = Vec::new();
    names.retain(|name| {
        let text = node_text(name, source);
        let first = !seen.contains(&text);
        seen.push(text);
        first
    });
    let mut definitions = Vec::new();
    if !names.is_empty() {
        definitions.push(Definition {
            names,
            range: start..typedef.end_byte(),
            value: None,
        });
    }
    definitions.extend(specifier.into_iter().flat_map(enumerators));
    definitions
}

/// The definition of `function`, which starts at `start`, or `None` when its name is not an
/// identifier, such as `Foo::bar` or `operator==`.
fn function_definition(function: Node<'_>, start: usize) -> Option<Definition<'_>> {
    let (name, _) = declared_name(function.child_by_field_name("declarator")?)?;
    Some(Definition {
        names: vec![name],
        range: start..function.end_byte(),
        value: None,
    })
}

/// The definitions of `specifier`, a struct, union, class or enum type that starts at `start`:
/// the type's name when it has a body, and the enumerators of a plain `enum`.
fn type_definitions(specifier: Node<'_>, start: usize) -> Vec<Definition<'_>> {
    let mut definitions: Vec<Definition> = defined_type_name(specifier)
        .map(|name| Definition {
            names: vec![name],
            range: start..specifier.end_byte(),
            value: None,
        })
        .into_iter()
        .collect();
    definitions.extend(enumerators(specifier));
    definitions
}

/// The name of the type that `specifier` defines, or `None` when it has no body, such as
/// `struct foo` in `struct foo *p;`, or no plain name.
fn defined_type_name(specifier: Node<'_>) -> Option<Node<'_>> {
    specifier.child_by_field_name("body")?;
    specifier
        .child_by_field_name("name")
        .filter(|name| name.kind() == "type_identifier")
}

/// The definitions of the enumerators of `specifier`, when it is a plain `enum`. C makes them
/// global, so code refers to them without the type.
fn enumerators(specifier: Node<'_>) -> Vec<Definition<'_>> {
    let mut cursor = specifier.walk();
    // C++ keeps the enumerators of an `enum class` or `enum struct` inside the type, as `Mode::A`.
    let scoped = specifier
        .children(&mut cursor)
        .any(|child| matches!(child.kind(), "class" | "struct"));
    let body = match specifier.child_by_field_name("body") {
        Some(body) if specifier.kind() == "enum_specifier" && !scoped => body,
        _ => return Vec::new(),
    };
    body.named_children(&mut cursor)
        .filter(|child| child.kind() == "enumerator")
        .filter_map(|enumerator| {
            Some(Definition {
                names: vec![enumerator.child_by_field_name("name")?],
                range: enumerator.byte_range(),
                value: enumerator.child_by_field_name("value").map(Value::Node),
            })
        })
        .collect()
}

/// The identifier that `declarator` declares, such as `p` in `*p = 0`, and whether it declares a
/// function. Returns `None` for a name that is not an identifier, such as `Foo::bar`.
fn declared_name(mut declarator: Node<'_>) -> Option<(Node<'_>, bool)> {
    let mut is_function = false;
    loop {
        match declarator.kind() {
            "identifier" | "type_identifier" => return Some((declarator, is_function)),
            "function_declarator" => is_function = true,
            // In `(*f)(int)`, the parentheses make `f` a pointer to a function. In
            // `f [[noreturn]] (void)`, the attribute leaves `f` a function.
            "parenthesized_declarator" | "attributed_declarator" => {}
            "init_declarator"
            | "pointer_declarator"
            | "array_declarator"
            | "reference_declarator" => is_function = false,
            _ => return None,
        }
        declarator = match declarator.child_by_field_name("declarator") {
            Some(inner) => inner,
            // `(*f)`, `&r` and `x [[maybe_unused]]` don't give their inner declarator a field. It
            // is their first child.
            None => {
                let mut cursor = declarator.walk();
                declarator
                    .named_children(&mut cursor)
                    .find(|child| !child.is_extra())?
            }
        };
    }
}

/// The definition of `node`, a `#define`, or `None` when the grammar found no name. `parser`
/// parses the language of `source`.
///
/// # Errors
/// Returns an error if `parser` fails to parse the macro's body.
fn macro_definition<'tree>(
    node: Node<'tree>,
    source: &str,
    parser: &mut tree_sitter::Parser,
) -> anyhow::Result<Option<Definition<'tree>>> {
    let Some(name) = node.child_by_field_name("name") else {
        return Ok(None);
    };
    let value = match node.kind() {
        // A macro with parameters is code, not a value.
        "preproc_function_def" => None,
        _ => Some(macro_value(node, name, source, parser)?),
    };
    // The node holds the line end after the macro.
    let end = node.start_byte() + node_text(&node, source).trim_end().len();
    Ok(Some(Definition {
        names: vec![name],
        range: node.start_byte()..end,
        value,
    }))
}

/// The value of `node`, a `#define` of `name` without parameters, in `source`: its body, read as
/// the value of a variable. `parser` parses the language of `source`.
///
/// # Errors
/// Returns an error if `parser` fails to parse the body.
fn macro_value<'tree>(
    node: Node<'tree>,
    name: Node<'tree>,
    source: &str,
    parser: &mut tree_sitter::Parser,
) -> anyhow::Result<Value<'tree>> {
    let Some(body) = node.child_by_field_name("value") else {
        return Ok(Value::Read(Ok(ScalarValue {
            text: String::new(),
            byte_range: name.end_byte()..name.end_byte(),
        })));
    };
    // The grammar keeps the body as one token. As the value of a variable, its strings,
    // parentheses and comments parse as they do in code.
    const PREFIX: &str = "int x = ";
    let body_text = node_text(&body, source);
    let text = format!("{PREFIX}{body_text}\n;");
    let tree = parser
        .parse(&text, None)
        .context("failed to parse syntax tree")?;
    let root = tree.root_node();
    let in_file = |at: usize| body.start_byte() + at - PREFIX.len();
    // The body holds a `//` comment at its end.
    let mut written = body_text.trim_end();
    if let Some(comment) = root
        .descendant_for_byte_range(
            PREFIX.len() + written.len() - 1,
            PREFIX.len() + written.len(),
        )
        .filter(|last| last.kind() == "comment")
    {
        #[expect(
            clippy::string_slice,
            reason = "tree-sitter nodes start on whole characters"
        )]
        let before = body_text[..comment.start_byte() - PREFIX.len()].trim_end();
        written = before;
    }
    // A backslash can continue the body onto a line that holds only a comment.
    while let Some(continued) = written.strip_suffix('\\') {
        written = continued.trim_end();
    }
    // A body that is not one value, such as `static inline`, or that the grammar can't read,
    // such as an unclosed string, is kept as written.
    let initializer = root
        .child(0)
        .and_then(|declaration| declaration.child_by_field_name("declarator"))
        .and_then(|declarator| declarator.child_by_field_name("value"))
        .filter(|value| !root.has_error() && value.end_byte() == PREFIX.len() + written.len());
    let Some(initializer) = initializer else {
        return Ok(Value::Read(Ok(ScalarValue {
            text: written.to_string(),
            byte_range: body.start_byte()..body.start_byte() + written.len(),
        })));
    };
    let value = code::node_value(initializer, &text, &decode_string)
        .map(|value| ScalarValue {
            text: value.text,
            byte_range: in_file(value.byte_range.start)..in_file(value.byte_range.end),
        })
        .map_err(|(string, error)| {
            // The body starts on the macro's line, so the rows of `text` count from there.
            let line = body.start_position().row + string.start_position().row + 1;
            let at = in_file(string.start_byte());
            (Position::new(line, character_column_at(source, at)), error)
        });
    Ok(Value::Read(value))
}

/// Decodes a C or C++ string literal, or several written one after the other, which C joins.
/// Returns `None` for any other node, and for strings that are not narrow text, such as `L"w"`.
///
/// # Errors
/// Returns an error if an escape is not one C has, stands for no character, doesn't fit in a
/// byte, or makes bytes that are not UTF-8 text.
fn decode_string(node: &Node, source: &str) -> anyhow::Result<Option<String>> {
    let mut cursor = node.walk();
    match node.kind() {
        "concatenated_string" => code::decode_concatenated(node, source, &decode_string),
        // The prefix and the opening quote make one token, such as `u8"`.
        "string_literal" => match node.child(0).map(|open| open.kind()) {
            Some("\"" | "u8\"") => code::decode_bytes(node, source, push_escape).map(Some),
            _ => Ok(None),
        },
        "raw_string_literal" => match node.child(0).map(|open| open.kind()) {
            Some("R\"" | "u8R\"") => Ok(node
                .children(&mut cursor)
                .find(|child| child.kind() == "raw_string_content")
                .map(|content| node_text(&content, source).to_string())),
            _ => Ok(None),
        },
        _ => Ok(None),
    }
}

/// Adds the bytes that `escape`, such as `\n` or `\x41`, stands for to `bytes`. The grammar
/// gives `\x` one to four hex digits, `\u` four and `\U` eight.
///
/// # Errors
/// Returns an error if `escape` is not one C has, stands for no character, or doesn't fit in a
/// byte.
fn push_escape(escape: &str, bytes: &mut Vec<u8>) -> anyhow::Result<()> {
    let not_an_escape = || anyhow!("`{escape}` is not an escape in a C string");
    let byte = |digits: &str, radix: u32| {
        u32::from_str_radix(digits, radix)
            .ok()
            .and_then(|number| u8::try_from(number).ok())
            .ok_or_else(|| anyhow!("`{escape}` doesn't fit in a byte"))
    };
    let rest = escape.strip_prefix('\\').ok_or_else(not_an_escape)?;
    let mut characters = rest.chars();
    let first = characters.next().ok_or_else(not_an_escape)?;
    let digits = characters.as_str();
    match first {
        // The grammar also reads `\8` and `\9` as escapes.
        '0'..='9' if rest.chars().all(|c| matches!(c, '0'..='7')) => bytes.push(byte(rest, 8)?),
        'x' => bytes.push(byte(digits, 16)?),
        'u' | 'U' => {
            let character = code::char_from_digits(escape, digits, 16)?;
            bytes.extend_from_slice(character.encode_utf8(&mut [0; 4]).as_bytes());
        }
        // A backslash at the end of a line joins the next line to it.
        '\n' => {}
        _ => bytes.push(match first {
            'a' => 0x07,
            'b' => 0x08,
            'f' => 0x0c,
            'n' => b'\n',
            'r' => b'\r',
            't' => b'\t',
            'v' => 0x0b,
            '\\' => b'\\',
            '\'' => b'\'',
            '"' => b'"',
            '?' => b'?',
            _ => return Err(not_an_escape()),
        }),
    }
    Ok(())
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let cpp_language = tree_sitter_c::LANGUAGE.into();
    let parser = language_parsers::c_style_comments_parser(&cpp_language, "comment")?;
    Ok(parser)
}

#[cfg(test)]
mod symbols_tests {
    use super::*;
    use crate::symbol_path::SymbolPath;
    use crate::symbols::ResolveError;
    use crate::symbols::code::test_utils::{
        decode_error, symbol, symbols, syntax_error, unresolved, value, values,
    };

    /// The symbols of `source`, a C file.
    fn c(source: &str) -> anyhow::Result<Symbols> {
        CSymbolsParser::new(&tree_sitter_c::LANGUAGE.into())?.parse(source)
    }

    /// The symbols of `source`, a C++ file.
    fn cpp(source: &str) -> anyhow::Result<Symbols> {
        CSymbolsParser::new(&tree_sitter_cpp::LANGUAGE.into())?.parse(source)
    }

    #[test]
    fn file_with_every_c_shape_parse_symbols_returns_every_symbol_in_order() -> anyhow::Result<()> {
        let source = r#"#include <stdio.h>

// Doc.
#define DNS_PORTS_MAX 8
#define SIZE 32 // with \0
#define FLAGS (1u << 4) /* bits */
#define NAME "ab" "cd"
#define MAX(a, b) ((a) > (b) ? (a) : (b))
#define NOTHING(x)
#define FOO_H
#define LONG 1 + \
  2
#define CONTINUED 1 // a \
still the comment
static const int E = 7;
int a = 1, b = 2;
const char *P = "p";
int arr[] = {1, 2};
int (*fp)(int) = 0;
int (*fp2)(int);
int z;
extern int h;
int g(void);
int (g2)(void);
typedef enum throttle_mode { T_A, T_B = 2 } throttle_mode_t;
typedef struct foo { int x; } foo;
typedef struct fwd fwd_t;
typedef int (*cb_t)(int);
enum { ANON = 5, NEXT };
struct S { int y; };
union U { int u; };
struct S2 { int y; } s2 = {1};
struct T {
#define IN_T 1
  int t;
} t1, t2;
int f(void) { return 0; }
static char *pf(void) {
#define INNER 1
  return 0;
}
#ifdef X
#define Y 1
int cond = 1;
#else
#define Y 2
#endif
"#;

        // <block affects="docs/symbols.md:c-paths, src/skill.md#/blockwatch/Symbols/C and C++">
        assert_eq!(
            symbols(&c(source)?, source),
            vec![
                symbol("/DNS_PORTS_MAX", "#define DNS_PORTS_MAX 8", Some("8")),
                symbol("/SIZE", r"#define SIZE 32 // with \0", Some("32")),
                symbol(
                    "/FLAGS",
                    "#define FLAGS (1u << 4) /* bits */",
                    Some("1u << 4")
                ),
                symbol("/NAME", r#"#define NAME "ab" "cd""#, Some("abcd")),
                symbol("/MAX", "#define MAX(a, b) ((a) > (b) ? (a) : (b))", None),
                symbol("/NOTHING", "#define NOTHING(x)", None),
                symbol("/FOO_H", "#define FOO_H", Some("")),
                symbol("/LONG", "#define LONG 1 + \\\n  2", Some("1 + \\\n  2")),
                symbol(
                    "/CONTINUED",
                    "#define CONTINUED 1 // a \\\nstill the comment",
                    Some("1")
                ),
                symbol("/E", "static const int E = 7;", Some("7")),
                symbol("/a", "a = 1", Some("1")),
                symbol("/b", "b = 2", Some("2")),
                symbol("/P", r#"const char *P = "p";"#, Some("p")),
                symbol("/arr", "int arr[] = {1, 2};", Some("{1, 2}")),
                symbol("/fp", "int (*fp)(int) = 0;", Some("0")),
                symbol("/fp2", "int (*fp2)(int);", None),
                symbol("/z", "int z;", None),
                symbol(
                    "/throttle_mode",
                    "typedef enum throttle_mode { T_A, T_B = 2 } throttle_mode_t;",
                    None
                ),
                symbol(
                    "/throttle_mode_t",
                    "typedef enum throttle_mode { T_A, T_B = 2 } throttle_mode_t;",
                    None
                ),
                symbol("/T_A", "T_A", None),
                symbol("/T_B", "T_B = 2", Some("2")),
                symbol("/foo", "typedef struct foo { int x; } foo;", None),
                symbol("/fwd_t", "typedef struct fwd fwd_t;", None),
                symbol("/cb_t", "typedef int (*cb_t)(int);", None),
                symbol("/ANON", "ANON = 5", Some("5")),
                symbol("/NEXT", "NEXT", None),
                symbol("/S", "struct S { int y; }", None),
                symbol("/U", "union U { int u; }", None),
                symbol("/S2", "struct S2 { int y; }", None),
                symbol("/s2", "struct S2 { int y; } s2 = {1};", Some("{1}")),
                symbol("/T", "struct T {\n#define IN_T 1\n  int t;\n}", None),
                symbol("/IN_T", "#define IN_T 1", Some("1")),
                symbol("/t1", "t1", None),
                symbol("/t2", "t2", None),
                symbol("/f", "int f(void) { return 0; }", None),
                symbol(
                    "/pf",
                    "static char *pf(void) {\n#define INNER 1\n  return 0;\n}",
                    None
                ),
                symbol("/INNER", "#define INNER 1", Some("1")),
                symbol("/Y", "#define Y 1", Some("1")),
                symbol("/cond", "int cond = 1;", Some("1")),
                symbol("/Y", "#define Y 2", Some("2")),
            ]
        );
        // </block>
        Ok(())
    }

    #[test]
    fn file_with_every_cpp_shape_parse_symbols_returns_every_symbol_in_order() -> anyhow::Result<()>
    {
        let source = r#"#include <string>

namespace ns {
int inner = 1;
#define NS_MACRO 1
}
enum class Mode { A, B };
enum Color { RED, GREEN = 2 };
class Widget { int m; };
struct Point { int x; };
using Alias = int;
template <class T> constexpr T pi = T(3);
template <class T> T id(T x) { return x; }
template <class T> class Box {};
extern "C" {
int k = 3;
}
extern "C" int c_fn(void) { return 0; }
constexpr auto F = 7;
int &ref = k;
std::string Str{"s"};
int Widget::count = 0;
void Widget::run() {}
bool operator==(Point a, Point b) { return true; }
[[nodiscard]] int attr(void) { return 0; }
int unused [[maybe_unused]] = 1;
void stop [[noreturn]] (void);
"#;

        assert_eq!(
            symbols(&cpp(source)?, source),
            vec![
                symbol("/NS_MACRO", "#define NS_MACRO 1", Some("1")),
                symbol("/Mode", "enum class Mode { A, B }", None),
                symbol("/Color", "enum Color { RED, GREEN = 2 }", None),
                symbol("/RED", "RED", None),
                symbol("/GREEN", "GREEN = 2", Some("2")),
                symbol("/Widget", "class Widget { int m; }", None),
                symbol("/Point", "struct Point { int x; }", None),
                symbol("/Alias", "using Alias = int;", None),
                symbol(
                    "/pi",
                    "template <class T> constexpr T pi = T(3);",
                    Some("T(3)")
                ),
                symbol("/id", "template <class T> T id(T x) { return x; }", None),
                symbol("/Box", "template <class T> class Box {}", None),
                symbol("/k", "int k = 3;", Some("3")),
                symbol("/c_fn", r#"extern "C" int c_fn(void) { return 0; }"#, None),
                symbol("/F", "constexpr auto F = 7;", Some("7")),
                symbol("/ref", "int &ref = k;", Some("k")),
                symbol("/Str", r#"std::string Str{"s"};"#, Some(r#"{"s"}"#)),
                symbol("/attr", "[[nodiscard]] int attr(void) { return 0; }", None),
                symbol("/unused", "int unused [[maybe_unused]] = 1;", Some("1")),
            ]
        );
        Ok(())
    }

    #[test]
    fn macro_values_parse_symbols_keeps_their_place_when_written_as_is() -> anyhow::Result<()> {
        let source = r#"#define PLAIN 8
#define COMMENTED 32 // c
#define WRAPPED ((1u << 4)) /* c */
#define SPLIT 1 + \
  2
#define STRING "a" \
  "b"
#define EMPTY
#define ONLY_COMMENT // c
#define EMPTY_PARENS ()
#define SPLIT_PARENS (1) + (2)
#define SEPARATED (1'000)
#define KEYWORDS static inline // c
#define UNCLOSED "abc
#define PAIR 1, y
#define SPLIT_COMMENT ("a") \
  // c
#define SPLIT_BLOCK (1u << 4) \
  /* c */
"#;

        // Each value with the text of the source where it is written. The two differ only when
        // the value is not written as is.
        let values: Vec<(String, String, &str)> = c(source)?
            .symbols
            .into_iter()
            .map(|symbol| {
                let value = symbol
                    .value
                    .expect("a macro without parameters has a value");
                (
                    symbol.path.to_string(),
                    value.text,
                    &source[value.byte_range],
                )
            })
            .collect();
        let value = |path: &str, text: &str, written: &'static str| {
            (path.to_string(), text.to_string(), written)
        };
        assert_eq!(
            values,
            vec![
                value("/PLAIN", "8", "8"),
                value("/COMMENTED", "32", "32"),
                value("/WRAPPED", "1u << 4", "1u << 4"),
                value("/SPLIT", "1 + \\\n  2", "1 + \\\n  2"),
                value("/STRING", "ab", "\"a\" \\\n  \"b\""),
                value("/EMPTY", "", ""),
                value("/ONLY_COMMENT", "", ""),
                value("/EMPTY_PARENS", "()", "()"),
                value("/SPLIT_PARENS", "(1) + (2)", "(1) + (2)"),
                value("/SEPARATED", "1'000", "1'000"),
                // A body that is not one value is kept as written.
                value("/KEYWORDS", "static inline", "static inline"),
                value("/UNCLOSED", "\"abc", "\"abc"),
                value("/PAIR", "1, y", "1, y"),
                value("/SPLIT_COMMENT", "a", "\"a\""),
                value("/SPLIT_BLOCK", "1u << 4", "1u << 4"),
            ]
        );
        Ok(())
    }

    #[test]
    fn string_kinds_parse_symbols_returns_decoded_values() -> anyhow::Result<()> {
        let source = r#"const char *Escapes = "a\a\b\f\n\r\t\v\\\'\"\?\101\7\x41\u00e9e\U0001F600\xc3\xa9\1234";
const char *Joined = "ab" /* c */ "cd";
const char *Split = "a\
b";
const char *Lines = "a"
    "b";
const char *Utf8 = u8"é" "x";
const char *Wide = L"w";
const char *Mixed = "a" L"b";
const char *WithMacro = "a" FMT;
char C = 'c';
#define MACRO_STRING ("x" "y") // z
#define MACRO_QUOTE "a // b" // c
#define MACRO_WIDE L"w"
#define MACRO_CHAR '"'
"#;

        assert_eq!(
            values(&c(source)?, source),
            vec![
                value(
                    "/Escapes",
                    "a\u{7}\u{8}\u{c}\n\r\t\u{b}\\'\"?A\u{7}Aée😀éS4"
                ),
                value("/Joined", "abcd"),
                value("/Split", "ab"),
                value("/Lines", "ab"),
                value("/Utf8", "éx"),
                value("/Wide", r#"L"w""#),
                value("/Mixed", r#""a" L"b""#),
                value("/WithMacro", r#""a" FMT"#),
                value("/C", "'c'"),
                value("/MACRO_STRING", "xy"),
                value("/MACRO_QUOTE", "a // b"),
                value("/MACRO_WIDE", r#"L"w""#),
                value("/MACRO_CHAR", r#"'"'"#),
            ]
        );
        Ok(())
    }

    #[test]
    fn raw_strings_parse_symbols_returns_the_text_of_narrow_ones() -> anyhow::Result<()> {
        let source = r#"const char *Raw = R"d(a"b)d";
const char *Utf8Raw = u8R"(é)";
const char *WideRaw = LR"(w)";
const char *Lines = R"(a
b)";
const char *Empty = R"()";
#define MACRO_RAW R"(x")"
"#;

        assert_eq!(
            values(&cpp(source)?, source),
            vec![
                value("/Raw", r#"a"b"#),
                value("/Utf8Raw", "é"),
                value("/WideRaw", r#"LR"(w)""#),
                value("/Lines", "a\nb"),
                value("/Empty", ""),
                value("/MACRO_RAW", r#"x""#),
            ]
        );
        Ok(())
    }

    #[test]
    fn crlf_line_endings_parse_symbols_reads_each_macro_to_its_end() -> anyhow::Result<()> {
        let source = "#define SPLIT 1 + \\\r\n  2\r\n#define COMMENTED \"x\" // c\r\n";

        assert_eq!(
            symbols(&c(source)?, source),
            vec![
                symbol(
                    "/SPLIT",
                    "#define SPLIT 1 + \\\r\n  2",
                    Some("1 + \\\r\n  2")
                ),
                symbol("/COMMENTED", "#define COMMENTED \"x\" // c", Some("x")),
            ]
        );
        Ok(())
    }

    #[test]
    fn escape_that_is_not_text_parse_symbols_makes_the_name_unreadable() -> anyhow::Result<()> {
        let source = r#"const char *Unknown = "\q";
const char *NotText = "\xff";
const char *TooBig = "\x100";
const char *BigOctal = "\777";
const char *Eight = "\8";
const char *Surrogate = "\ud800";
#define MACRO_UNKNOWN "\q"
#define MULTI "a" \
  "\q"
#define WRAPPED ( \
  "\q")
const char *Fine = "ok";
"#;

        assert_eq!(
            symbols(&c(source)?, source),
            vec![symbol("/Fine", r#"const char *Fine = "ok";"#, Some("ok"))]
        );
        assert_eq!(
            unresolved(&c(source)?, source, "/Unknown")?,
            decode_error(1, 23, r"`\q` is not an escape in a C string")
        );
        assert_eq!(
            unresolved(&c(source)?, source, "/NotText")?,
            decode_error(2, 23, "its bytes are not UTF-8 text")
        );
        assert_eq!(
            unresolved(&c(source)?, source, "/TooBig")?,
            decode_error(3, 22, r"`\x100` doesn't fit in a byte")
        );
        assert_eq!(
            unresolved(&c(source)?, source, "/BigOctal")?,
            decode_error(4, 24, r"`\777` doesn't fit in a byte")
        );
        assert_eq!(
            unresolved(&c(source)?, source, "/Eight")?,
            decode_error(5, 21, r"`\8` is not an escape in a C string")
        );
        assert_eq!(
            unresolved(&c(source)?, source, "/Surrogate")?,
            decode_error(6, 25, r"`\ud800` stands for no character")
        );
        assert_eq!(
            unresolved(&c(source)?, source, "/MACRO_UNKNOWN")?,
            decode_error(7, 23, r"`\q` is not an escape in a C string")
        );
        // The error shows where the joined strings start.
        assert_eq!(
            unresolved(&c(source)?, source, "/MULTI")?,
            decode_error(8, 15, r"`\q` is not an escape in a C string")
        );
        assert_eq!(
            unresolved(&c(source)?, source, "/WRAPPED")?,
            decode_error(11, 3, r"`\q` is not an escape in a C string")
        );
        Ok(())
    }

    #[test]
    fn comment_inside_a_macro_parse_symbols_makes_its_name_unreadable() -> anyhow::Result<()> {
        // The grammar ends the macro at the comment, and reads `+ 2` as code. So the macro would
        // read `1`.
        let source = r#"#define INSIDE 1 /* c */ + 2
#define AFTER 3
"#;

        assert_eq!(
            symbols(&c(source)?, source),
            vec![symbol("/AFTER", "#define AFTER 3", Some("3"))]
        );
        assert_eq!(
            unresolved(&c(source)?, source, "/INSIDE")?,
            syntax_error(1, 1, "INSIDE")
        );
        Ok(())
    }

    #[test]
    fn macro_without_a_line_end_at_the_end_of_the_file_parse_symbols_reads_it() -> anyhow::Result<()>
    {
        let source = "#define END 1";

        assert_eq!(
            symbols(&c(source)?, source),
            vec![symbol("/END", "#define END 1", Some("1"))]
        );
        Ok(())
    }

    #[test]
    fn line_end_that_the_grammar_misreads_parse_symbols_ends_the_macro_there() -> anyhow::Result<()>
    {
        // The grammar would read the line end after the `/` as part of the macro.
        let source = r#"#define URL 1 // https://example.com/
#define ROOT /
#define NEXT 2
#define SPLIT 3 + \
  4 // https://example.com/
#define CLOSED 5 /* c */
#define LAST 6
int ratio = 8 /
  2;
"#;

        assert_eq!(
            symbols(&c(source)?, source),
            vec![
                symbol("/URL", "#define URL 1 // https://example.com/", Some("1")),
                symbol("/ROOT", "#define ROOT /", Some("/")),
                symbol("/NEXT", "#define NEXT 2", Some("2")),
                symbol(
                    "/SPLIT",
                    "#define SPLIT 3 + \\\n  4 // https://example.com/",
                    Some("3 + \\\n  4")
                ),
                symbol("/CLOSED", "#define CLOSED 5 /* c */", Some("5")),
                symbol("/LAST", "#define LAST 6", Some("6")),
                symbol("/ratio", "int ratio = 8 /\n  2;", Some("8 /\n  2")),
            ]
        );
        // A space or tab after an empty macro would do the same. A space after a backslash
        // still continues the macro, as it does for a compiler.
        let source = "#define GUARD_H \n#define NEXT 2\n#define EMPTY(x)\t\n#define AFTER 3\n#define SPACED 1 + \\ \n  2 // https://example.com/\n#define LAST 4\n";
        assert_eq!(
            symbols(&c(source)?, source),
            vec![
                symbol("/GUARD_H", "#define GUARD_H", Some("")),
                symbol("/NEXT", "#define NEXT 2", Some("2")),
                symbol("/EMPTY", "#define EMPTY(x)", None),
                symbol("/AFTER", "#define AFTER 3", Some("3")),
                symbol(
                    "/SPACED",
                    "#define SPACED 1 + \\ \n  2 // https://example.com/",
                    Some("1 + \\ \n  2")
                ),
                symbol("/LAST", "#define LAST 4", Some("4")),
            ]
        );
        Ok(())
    }

    #[test]
    fn syntax_errors_parse_symbols_makes_the_names_near_them_unreadable() -> anyhow::Result<()> {
        // The grammar can't expand the macros of an eBPF program, such as `__uint` and
        // `__always_inline`. The constants around them still parse.
        let source = r#"#define DNS_PORTS_MAX 8

struct {
    __uint(type, BPF_MAP_TYPE_HASH);
    __uint(max_entries, DNS_PORTS_MAX);
} conn_map SEC(".maps");

static __always_inline int helper(int x) { return x; }

int after = 3;
"#;

        assert_eq!(
            symbols(&c(source)?, source),
            vec![
                symbol("/DNS_PORTS_MAX", "#define DNS_PORTS_MAX 8", Some("8")),
                symbol("/after", "int after = 3;", Some("3")),
            ]
        );
        assert_eq!(
            unresolved(&c(source)?, source, "/conn_map")?,
            syntax_error(4, 12, "conn_map")
        );
        assert_eq!(
            unresolved(&c(source)?, source, "/max_entries")?,
            syntax_error(5, 12, "max_entries")
        );
        assert_eq!(
            unresolved(&c(source)?, source, "/helper")?,
            syntax_error(8, 24, "helper")
        );
        Ok(())
    }

    #[test]
    fn span_that_swallows_definitions_parse_symbols_makes_their_names_unreadable()
    -> anyhow::Result<()> {
        // The unclosed `{` makes the parser skip everything after it, `y` and `b` included.
        let source = r#"int a = 1;
struct S { int m;
int y = 2;

int b = 3;
"#;

        assert_eq!(
            symbols(&c(source)?, source),
            vec![symbol("/a", "int a = 1;", Some("1"))]
        );
        for name in ["S", "y", "b"] {
            assert_eq!(
                unresolved(&c(source)?, source, &format!("/{name}"))?,
                syntax_error(2, 1, name)
            );
        }
        Ok(())
    }

    #[test]
    fn unclosed_function_parse_symbols_finds_no_definition_inside_it() -> anyhow::Result<()> {
        // The parser assumes a `}` at the end of the file, so `y` and `b` are local variables of
        // `f`, and no skipped span holds their names.
        let source = r#"int a = 1;
int f(void) {
int y = 2;

int b = 3;
"#;

        let symbols = c(source)?;

        assert!(matches!(
            symbols.resolve(&SymbolPath::parse("/b")?),
            Err(ResolveError::NotFound { .. })
        ));
        assert_eq!(
            unresolved(&c(source)?, source, "/f")?,
            syntax_error(5, 11, "f")
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Position, language_parsers::Comment};

    #[test]
    fn parses_c_comments_correctly() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser()?;

        let blocks: Vec<Comment> = comments_parser
            .parse(
                r#"
            // This is a single-line comment in C.
            #include <stdio.h>

            /*
             * This is a multi-line comment.
             * It spans multiple lines in C.
             */

            int main() {
                printf("Hello, C!\n"); // Prints a message to the console.

                /* Another comment
                 * split into
                 * multiple lines.
                 */

                return 0;
            }
/// Triple slash comment.
            "#,
            )
            .collect();

        assert_eq!(
            blocks,
            vec![
                Comment {
                    position_range: Position::new(2, 13)..Position::new(2, 51),
                    source_range: 13..51,
                    comment_text: "   This is a single-line comment in C.".to_string()
                },
                Comment {
                    position_range: Position::new(5, 13)..Position::new(8, 16),
                    source_range: 96..204,
                    comment_text:
                        "  \n               This is a multi-line comment.\n               It spans multiple lines in C.\n               "
                            .to_string()
                },
                Comment {
                    position_range: Position::new(11, 40)..Position::new(11, 75),
                    source_range: 270..305,
                    comment_text: "   Prints a message to the console.".to_string()
                },
                Comment {
                    position_range: Position::new(13, 17)..Position::new(16, 20),
                    source_range: 323..426,
                    comment_text: "   Another comment\n                   split into\n                   multiple lines.\n                   ".to_string()
                },
                Comment {
                    position_range: Position::new(20, 1)..Position::new(20, 26),
                    source_range: 468..493,
                    comment_text: "  / Triple slash comment.".to_string()
                },
            ]
        );

        Ok(())
    }
}
