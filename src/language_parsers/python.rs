use crate::language_parsers::{
    CommentsParser, LanguageParser, LanguageParserImpl, node_text, python_style_comments_parser,
};
use crate::symbols::code::{CodeLanguage, CodeSymbolsParser, Definition, Value};
use anyhow::anyhow;
use tree_sitter::Node;

/// Returns a [`LanguageParser`] for Python, with its symbols.
pub(super) fn parser() -> anyhow::Result<impl LanguageParser> {
    Ok(
        LanguageParserImpl::new(comments_parser()?).with_symbols(CodeSymbolsParser::new(
            &tree_sitter_python::LANGUAGE.into(),
            Python,
        )?),
    )
}

/// Python or Starlark, whose top-level names are variables, functions, classes and type aliases.
pub(super) struct Python;

impl CodeLanguage for Python {
    /// The top-level definitions under `root`, the root of a Python or Starlark file's syntax
    /// tree, in document order. `source` is the text of the file.
    fn definitions<'tree>(&self, root: Node<'tree>, source: &str) -> Vec<Definition<'tree>> {
        let mut definitions = Vec::new();
        // The walk keeps its own stack, as deeply nested source could overflow the call stack.
        let mut pending = vec![root];
        let mut cursor = root.walk();
        while let Some(node) = pending.pop() {
            let definition = match node.kind() {
                // Python gives the names in these bodies to the module, as it does at the top
                // level. A loop or a `match` binds a name each time it runs, so its body doesn't
                // count.
                "module" | "block" | "if_statement" | "elif_clause" | "else_clause"
                | "try_statement" | "except_clause" | "finally_clause" | "with_statement" => {
                    let first = pending.len();
                    pending.extend(node.named_children(&mut cursor));
                    // The stack takes the last child first, so the children go on it in reverse.
                    pending[first..].reverse();
                    None
                }
                "expression_statement" => node
                    .named_child(0)
                    .filter(|child| child.kind() == "assignment")
                    .and_then(|assignment| assignment_definition(node, assignment, source)),
                "function_definition" | "class_definition" => named_definition(node, node, source),
                "decorated_definition" => node
                    .child_by_field_name("definition")
                    .and_then(|definition| named_definition(node, definition, source)),
                "type_alias_statement" => node
                    .child_by_field_name("left")
                    .and_then(|alias| alias.named_child(0))
                    // `type Pair[T] = ...` writes its name in a generic type.
                    .and_then(|alias| match alias.kind() {
                        "generic_type" => alias.named_child(0),
                        _ => Some(alias),
                    })
                    .and_then(|name| definition(node, vec![name], None, source)),
                _ => None,
            };
            definitions.extend(definition);
        }
        definitions
    }

    /// Decodes a Python string, or several written one after the other, which Python joins. Returns
    /// `None` for any other node, for bytes, and for an f-string with a field, such as `f"{x}"`.
    ///
    /// # Errors
    /// Returns an error if an escape gives a character by its name, or stands for no character.
    fn decode_string(&self, node: &Node, source: &str) -> anyhow::Result<Option<String>> {
        let mut cursor = node.walk();
        match node.kind() {
            "concatenated_string" => {
                let mut joined = String::new();
                for part in node.named_children(&mut cursor) {
                    if part.is_extra() {
                        continue;
                    }
                    match self.decode_string(&part, source)? {
                        Some(text) => joined.push_str(&text),
                        None => return Ok(None),
                    }
                }
                Ok(Some(joined))
            }
            "string" => {
                // The start holds the prefix and the opening quotes, such as `rb"`.
                let bytes = node
                    .child(0)
                    .is_some_and(|start| node_text(&start, source).contains(['b', 'B']));
                let parts: Vec<Node> = node.named_children(&mut cursor).collect();
                if bytes || parts.iter().any(|part| part.kind() == "interpolation") {
                    return Ok(None);
                }
                let mut text = String::new();
                for part in parts.iter().filter(|part| part.kind() == "string_content") {
                    push_content(part, source, &mut text)?;
                }
                Ok(Some(text))
            }
            _ => Ok(None),
        }
    }
}

/// The definition of `statement`, which holds `assignment`, or `None` when it assigns to no name,
/// as `a, b = 1, 2` and `obj.x = 1` do. In `A = B = 5`, both names share the definition.
fn assignment_definition<'tree>(
    statement: Node<'tree>,
    mut assignment: Node<'tree>,
    source: &str,
) -> Option<Definition<'tree>> {
    let mut names = Vec::new();
    loop {
        names.extend(assignment.child_by_field_name("left"));
        match assignment.child_by_field_name("right") {
            Some(right) if right.kind() == "assignment" => assignment = right,
            // `X: int` declares a name without a value.
            value => return definition(statement, names, value.map(Value::Node), source),
        }
    }
}

/// The definition of `function`, a function or a class, which `node` holds with its decorators.
fn named_definition<'tree>(
    node: Node<'tree>,
    function: Node<'tree>,
    source: &str,
) -> Option<Definition<'tree>> {
    let name = function.child_by_field_name("name")?;
    definition(node, vec![name], None, source)
}

/// The definition of `names` at `node`, or `None` when none of them is a name code can refer to.
fn definition<'tree>(
    node: Node<'tree>,
    mut names: Vec<Node<'tree>>,
    value: Option<Value<'tree>>,
    source: &str,
) -> Option<Definition<'tree>> {
    // `_` names a value that the code throws away.
    names.retain(|name| name.kind() == "identifier" && node_text(name, source) != "_");
    (!names.is_empty()).then(|| Definition {
        names,
        range: node.byte_range(),
        value,
    })
}

/// Adds the text of `content`, a part of a string between its quotes and its fields, to `text`.
///
/// # Errors
/// Returns an error if an escape gives a character by its name, or stands for no character.
fn push_content(content: &Node, source: &str, text: &mut String) -> anyhow::Result<()> {
    let mut written_up_to = content.start_byte();
    let mut cursor = content.walk();
    for part in content.named_children(&mut cursor) {
        push_written(source, written_up_to..part.start_byte(), text);
        let escape = node_text(&part, source);
        match part.kind() {
            "escape_sequence" => push_escape(escape, text)?,
            // An `escape_interpolation`, which only an f-string has: `{{` and `}}` stand for one
            // brace.
            _ => text.extend(escape.chars().next()),
        }
        written_up_to = part.end_byte();
    }
    push_written(source, written_up_to..content.end_byte(), text);
    Ok(())
}

/// Adds the text of `source` in `range`, which holds no escapes, to `text`.
fn push_written(source: &str, range: std::ops::Range<usize>, text: &mut String) {
    #[expect(
        clippy::string_slice,
        reason = "tree-sitter nodes start and end on whole characters"
    )]
    let written = &source[range];
    // Python reads a CRLF line end in a file as a line feed.
    text.push_str(&written.replace("\r\n", "\n"));
}

/// Adds the character that `escape`, such as `\n` or `\x41`, stands for to `text`. An escape
/// Python doesn't know, such as `\d`, stays as written.
///
/// # Errors
/// Returns an error if `escape` gives a character by its name, or stands for no character.
fn push_escape(escape: &str, text: &mut String) -> anyhow::Result<()> {
    // The grammar starts each escape with a backslash and at least one more character.
    let mut characters = escape.chars();
    characters.next();
    let rest = characters.as_str();
    let first = characters.next().unwrap_or_default();
    let digits = characters.as_str();
    let code = |digits: &str, radix: u32| {
        u32::from_str_radix(digits, radix)
            .ok()
            .and_then(char::from_u32)
            .ok_or_else(|| anyhow!("`{escape}` stands for no character"))
    };
    match first {
        // An octal escape takes up to three digits, so `\18` is `\1` and `8`. The grammar also
        // reads `\8` as an escape, which Python keeps as written.
        '0'..='7' => {
            let octal = rest.chars().take_while(|c| matches!(c, '0'..='7')).count();
            #[expect(clippy::string_slice, reason = "the digits are ASCII")]
            let (digits, after) = (&rest[..octal], &rest[octal..]);
            text.push(code(digits, 8)?);
            text.push_str(after);
        }
        'x' | 'u' | 'U' => text.push(code(digits, 16)?),
        'N' => {
            return Err(anyhow!(
                "`{escape}` gives a character by its name, which blockwatch can't look up"
            ));
        }
        // A backslash at the end of a line joins the next line to it.
        '\r' | '\n' => {}
        'a' => text.push('\u{7}'),
        'b' => text.push('\u{8}'),
        'f' => text.push('\u{c}'),
        'n' => text.push('\n'),
        'r' => text.push('\r'),
        't' => text.push('\t'),
        'v' => text.push('\u{b}'),
        '\\' | '\'' | '"' => text.push(first),
        _ => text.push_str(escape),
    }
    Ok(())
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let python_language = tree_sitter_python::LANGUAGE.into();
    let parser = python_style_comments_parser(&python_language, "comment")?;
    Ok(parser)
}

#[cfg(test)]
mod symbols_tests {
    use super::*;
    use crate::symbols::code::test_utils::{
        decode_error, symbol, symbols, syntax_error, unresolved, value, values,
    };
    use crate::symbols::{Symbols, SymbolsParser};

    /// The symbols of `source`, a Python file.
    fn python(source: &str) -> anyhow::Result<Symbols> {
        CodeSymbolsParser::new(&tree_sitter_python::LANGUAGE.into(), Python)?.parse(source)
    }

    /// The symbols of `source`, a Starlark file.
    fn starlark(source: &str) -> anyhow::Result<Symbols> {
        CodeSymbolsParser::new(&tree_sitter_starlark::LANGUAGE.into(), Python)?.parse(source)
    }

    #[test]
    fn file_with_every_python_shape_parse_symbols_returns_every_symbol_in_order()
    -> anyhow::Result<()> {
        let source = r#""""Doc."""
import os
from typing import Final

# Doc.
MAX_PORTS = 8
PYTHON_VERSION = "3.12"
TIMEOUT: Final[int] = 30
PENDING: int
A = B = 5
IMAGE = os.environ.get("X", "golang:1.25-alpine")
PRESETS: dict[str, str] = {
    "a": "b",
}
WRAPPED = (32)
X = 1; Y = 2
a, b = 1, 2
obj.attr = 1
items["k"] = 1
__all__ = ["load"]
__all__ += ["Point"]
_ = 3
type Alias = int
type Pair[T] = tuple[T, T]


def load(path):
    return path


async def fetch():
    pass


@dataclass
class Point:
    x: int = 0


class MenuItem(Enum):
    RED = 1


if TYPE_CHECKING:
    MODE = 1
elif OTHER:
    ELIF_NAME = 2
else:
    MODE = 2

try:
    TRIED = 1
except ImportError as error:
    EXCEPTED = 2
else:
    TRY_ELSE = 3
finally:
    FINALLY = 4

with open("f") as handle:
    WITH_NAME = 1

if A:
    if B:
        NESTED = 1

for i in range(3):
    LOOPED = i

while ready:
    WHILE_NAME = 1

match mode:
    case 1:
        MATCHED = 1
"#;

        // <block affects="docs/symbols.md:python-paths, src/skill.md#/blockwatch/Symbols/Python and Starlark">
        assert_eq!(
            symbols(&python(source)?, source),
            vec![
                symbol("/MAX_PORTS", "MAX_PORTS = 8", Some("8")),
                symbol(
                    "/PYTHON_VERSION",
                    r#"PYTHON_VERSION = "3.12""#,
                    Some("3.12")
                ),
                symbol("/TIMEOUT", "TIMEOUT: Final[int] = 30", Some("30")),
                symbol("/PENDING", "PENDING: int", None),
                symbol("/A", "A = B = 5", Some("5")),
                symbol("/B", "A = B = 5", Some("5")),
                symbol(
                    "/IMAGE",
                    r#"IMAGE = os.environ.get("X", "golang:1.25-alpine")"#,
                    Some(r#"os.environ.get("X", "golang:1.25-alpine")"#)
                ),
                symbol(
                    "/PRESETS",
                    "PRESETS: dict[str, str] = {\n    \"a\": \"b\",\n}",
                    Some("{\n    \"a\": \"b\",\n}")
                ),
                symbol("/WRAPPED", "WRAPPED = (32)", Some("32")),
                symbol("/X", "X = 1", Some("1")),
                symbol("/Y", "Y = 2", Some("2")),
                symbol("/__all__", r#"__all__ = ["load"]"#, Some(r#"["load"]"#)),
                symbol("/Alias", "type Alias = int", None),
                symbol("/Pair", "type Pair[T] = tuple[T, T]", None),
                symbol("/load", "def load(path):\n    return path", None),
                symbol("/fetch", "async def fetch():\n    pass", None),
                symbol("/Point", "@dataclass\nclass Point:\n    x: int = 0", None),
                symbol("/MenuItem", "class MenuItem(Enum):\n    RED = 1", None),
                symbol("/MODE", "MODE = 1", Some("1")),
                symbol("/ELIF_NAME", "ELIF_NAME = 2", Some("2")),
                symbol("/MODE", "MODE = 2", Some("2")),
                symbol("/TRIED", "TRIED = 1", Some("1")),
                symbol("/EXCEPTED", "EXCEPTED = 2", Some("2")),
                symbol("/TRY_ELSE", "TRY_ELSE = 3", Some("3")),
                symbol("/FINALLY", "FINALLY = 4", Some("4")),
                symbol("/WITH_NAME", "WITH_NAME = 1", Some("1")),
                symbol("/NESTED", "NESTED = 1", Some("1")),
            ]
        );
        // </block>
        Ok(())
    }

    #[test]
    fn file_with_every_starlark_shape_parse_symbols_returns_every_symbol_in_order()
    -> anyhow::Result<()> {
        let source = r#"load("@rules_python//python:defs.bzl", "py_library")

PYTHON_VERSION = "3.12"
DEPS = [
    "//a",
]

def _impl(ctx):
    return []

my_rule = rule(implementation = _impl)

cc_library(
    name = "lib",
)
"#;

        assert_eq!(
            symbols(&starlark(source)?, source),
            vec![
                symbol(
                    "/PYTHON_VERSION",
                    r#"PYTHON_VERSION = "3.12""#,
                    Some("3.12")
                ),
                symbol(
                    "/DEPS",
                    "DEPS = [\n    \"//a\",\n]",
                    Some("[\n    \"//a\",\n]")
                ),
                symbol("/_impl", "def _impl(ctx):\n    return []", None),
                symbol(
                    "/my_rule",
                    "my_rule = rule(implementation = _impl)",
                    Some("rule(implementation = _impl)")
                ),
            ]
        );
        Ok(())
    }

    #[test]
    fn string_kinds_parse_symbols_returns_decoded_values() -> anyhow::Result<()> {
        let source = r#"ESCAPES = "a\a\b\f\n\r\t\v\\\'\"\101\x41\u00e9\U0001F600\xe9\400"
UNKNOWN = "\d+"
OCTAL_THEN_DIGIT = "\18"
EIGHT = "\8"
BRACES = "a{{b}}"
SPLIT = "a\
b"
TRIPLE = """a
b"""
JOINED = ("a"
    'b')
RAW = r"a\nb"
RAW_QUOTE = r'a\''
UPPER_RAW = R"a\n"
UNICODE = u"x"
FSTRING = f"a{{b}}"
UPPER_F = F"x"
RAW_F = rf"a\n{{"
FIELD = f"a{x}"
BYTES = b"a\n"
UPPER_BYTES = B"a"
COMMENTED = ("a"  # c
    "b")
WITH_FIELD = "a" f"{x}"
"#;

        assert_eq!(
            values(&python(source)?, source),
            vec![
                value("/ESCAPES", "a\u{7}\u{8}\u{c}\n\r\t\u{b}\\'\"AAé😀éĀ"),
                // Python keeps an escape it doesn't know as written.
                value("/UNKNOWN", r"\d+"),
                value("/OCTAL_THEN_DIGIT", "\u{1}8"),
                value("/EIGHT", r"\8"),
                // Only an f-string reads `{{` as one brace.
                value("/BRACES", "a{{b}}"),
                value("/SPLIT", "ab"),
                value("/TRIPLE", "a\nb"),
                value("/JOINED", "ab"),
                value("/RAW", r"a\nb"),
                value("/RAW_QUOTE", r"a\'"),
                value("/UPPER_RAW", r"a\n"),
                value("/UNICODE", "x"),
                value("/FSTRING", "a{b}"),
                value("/UPPER_F", "x"),
                value("/RAW_F", r"a\n{"),
                value("/FIELD", r#"f"a{x}""#),
                value("/BYTES", r#"b"a\n""#),
                value("/UPPER_BYTES", r#"B"a""#),
                value("/COMMENTED", "ab"),
                value("/WITH_FIELD", r#""a" f"{x}""#),
            ]
        );
        Ok(())
    }

    #[test]
    fn crlf_line_endings_parse_symbols_reads_line_feeds() -> anyhow::Result<()> {
        // Python reads every line end in a file as a line feed.
        let source =
            "TRIPLE = \"\"\"a\r\nb\"\"\"\r\nRAW = r\"\"\"a\r\nb\"\"\"\r\nSPLIT = \"a\\\r\nb\"\r\n";

        assert_eq!(
            values(&python(source)?, source),
            vec![
                value("/TRIPLE", "a\nb"),
                value("/RAW", "a\nb"),
                value("/SPLIT", "ab"),
            ]
        );
        Ok(())
    }

    #[test]
    fn escape_that_is_not_text_parse_symbols_makes_the_name_unreadable() -> anyhow::Result<()> {
        let source = r#"NAMED = "\N{EM DASH}"
SURROGATE = "\ud800"
TOO_BIG = "\U00110000"
JOINED = "a" "\ud800"
FINE = "ok"
"#;

        assert_eq!(
            symbols(&python(source)?, source),
            vec![symbol("/FINE", r#"FINE = "ok""#, Some("ok"))]
        );
        assert_eq!(
            unresolved(&python(source)?, source, "/NAMED")?,
            decode_error(
                1,
                9,
                r"`\N{EM DASH}` gives a character by its name, which blockwatch can't look up"
            )
        );
        assert_eq!(
            unresolved(&python(source)?, source, "/SURROGATE")?,
            decode_error(2, 13, r"`\ud800` stands for no character")
        );
        assert_eq!(
            unresolved(&python(source)?, source, "/TOO_BIG")?,
            decode_error(3, 11, r"`\U00110000` stands for no character")
        );
        assert_eq!(
            unresolved(&python(source)?, source, "/JOINED")?,
            decode_error(4, 10, r"`\ud800` stands for no character")
        );
        Ok(())
    }

    #[test]
    fn syntax_errors_parse_symbols_makes_the_names_near_them_unreadable() -> anyhow::Result<()> {
        let source = r#"PORTS = 8
def helper(:
    return 1
BROKEN = = 1
AFTER = 3
"#;

        assert_eq!(
            symbols(&python(source)?, source),
            vec![
                symbol("/PORTS", "PORTS = 8", Some("8")),
                symbol("/AFTER", "AFTER = 3", Some("3")),
            ]
        );
        assert_eq!(
            unresolved(&python(source)?, source, "/helper")?,
            syntax_error(2, 12, "helper")
        );
        assert_eq!(
            unresolved(&python(source)?, source, "/BROKEN")?,
            syntax_error(4, 10, "BROKEN")
        );
        Ok(())
    }

    #[test]
    fn span_that_swallows_definitions_parse_symbols_makes_their_names_unreadable()
    -> anyhow::Result<()> {
        // The unclosed `{` makes the parser skip the lines after it, `AFTER` included.
        let source = r#"PORTS = 8
CONFIG = {
    "a": 1,

AFTER = 3
"#;

        assert_eq!(
            symbols(&python(source)?, source),
            vec![symbol("/PORTS", "PORTS = 8", Some("8"))]
        );
        for name in ["CONFIG", "AFTER"] {
            assert_eq!(
                unresolved(&python(source)?, source, &format!("/{name}"))?,
                syntax_error(2, 8, name)
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
def main():
    # This is a single line comment
    print("Hello")  # This is an inline comment

# This is a multi-line
# comment that spans
# several lines

value = 42  # Comment after code
"#,
            )
            .collect();

        assert_eq!(
            blocks,
            vec![
                Comment {
                    position_range: Position::new(3, 5)..Position::new(3, 36),
                    source_range: 17..48,
                    comment_text: "  This is a single line comment".to_string()
                },
                Comment {
                    position_range: Position::new(4, 21)..Position::new(4, 48),
                    source_range: 69..96,
                    comment_text: "  This is an inline comment".to_string()
                },
                Comment {
                    position_range: Position::new(6, 1)..Position::new(6, 23),
                    source_range: 98..120,
                    comment_text: "  This is a multi-line".to_string()
                },
                Comment {
                    position_range: Position::new(7, 1)..Position::new(7, 21),
                    source_range: 121..141,
                    comment_text: "  comment that spans".to_string()
                },
                Comment {
                    position_range: Position::new(8, 1)..Position::new(8, 16),
                    source_range: 142..157,
                    comment_text: "  several lines".to_string()
                },
                Comment {
                    position_range: Position::new(10, 13)..Position::new(10, 33),
                    source_range: 171..191,
                    comment_text: "  Comment after code".to_string()
                },
            ]
        );

        Ok(())
    }
}
