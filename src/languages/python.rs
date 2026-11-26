use crate::languages::code_symbols::{
    self, CodeLanguage, CodeSymbolsParser, Definition, NestedWalk, Value,
};
use crate::languages::{
    CommentsParser, LanguageParser, LanguageParserImpl, node_text, python_style_comments_parser,
};
use anyhow::{anyhow, bail};
use std::borrow::Cow;
use tree_sitter::Node;

/// Returns a [`LanguageParser`] for Python, with its symbols.
pub(super) fn parser() -> impl LanguageParser {
    LanguageParserImpl::new(comments_parser()).with_symbols(CodeSymbolsParser::new(
        &tree_sitter_python::LANGUAGE.into(),
        Python,
    ))
}

/// Python or Starlark, whose files hold variables, functions, classes, type aliases and dict
/// entries, and whose classes hold their members.
pub(super) struct Python;

impl CodeLanguage for Python {
    /// The top-level definitions under `root`, the root of a Python or Starlark file's syntax
    /// tree, with class members and dict keys under them, in document order. A class or dict comes
    /// before its members. `source` is the text of the file.
    fn definitions<'tree>(&self, root: Node<'tree>, source: &str) -> Vec<Definition<'tree>> {
        code_symbols::nested_definitions(root, |body, parents, walk| {
            self.push_members(body, parents, source, walk);
        })
    }

    /// Decodes a Python string, or several written one after the other, which Python joins. Returns
    /// `None` for any other node, for bytes, and for an f-string with a field, such as `f"{x}"`.
    ///
    /// # Errors
    /// Returns an error if an escape gives a character by its name, stands for no character, or
    /// is cut short, such as `\x4`.
    fn decode_string(&self, node: &Node, source: &str) -> anyhow::Result<Option<String>> {
        let mut cursor = node.walk();
        match node.kind() {
            "concatenated_string" => {
                code_symbols::decode_concatenated(node, source, &|part, source| {
                    self.decode_string(part, source)
                })
            }
            "string" => {
                // The start holds the prefix and the opening quotes, such as `rb"`.
                let start = node
                    .child(0)
                    .map_or_default(|start| node_text(&start, source));
                let parts: Vec<Node> = node.named_children(&mut cursor).collect();
                if start.contains(['b', 'B'])
                    || parts.iter().any(|part| part.kind() == "interpolation")
                {
                    return Ok(None);
                }
                let is_raw = start.contains(['r', 'R']);
                let mut text = String::new();
                for part in parts.iter().filter(|part| part.kind() == "string_content") {
                    push_content(part, source, is_raw, &mut text)?;
                }
                Ok(Some(text))
            }
            _ => Ok(None),
        }
    }

    /// Decodes a string dict key into its path segment, and keeps identifiers as written.
    ///
    /// # Panics
    /// Panics if `name` is a string that [`Self::decode_string`] cannot decode.
    fn name_segment<'s>(&self, name: &Node, source: &'s str) -> Cow<'s, str> {
        match self.decode_string(name, source) {
            Ok(Some(text)) => Cow::Owned(text),
            Ok(None) => Cow::Borrowed(node_text(name, source)),
            // `push_members` keeps only keys that `decode_string` decoded.
            Err(error) => unreachable!("undecodable string key reached name_segment: {error:#}"),
        }
    }

    // Dict keys can be single- or double-quoted, so skipped syntax errors check both.
    const NAME_QUOTES: &'static [char] = &['"', '\''];
}

impl Python {
    /// Adds the definitions in `body` to `walk`: the root of a file, a class body, a control-flow
    /// block or a dict literal. `parents` are the names of the classes and dicts that `body` sits
    /// in. Adds each nested class body, block and dict in `body` to `walk`, with its own parents.
    fn push_members<'tree>(
        &self,
        body: Node<'tree>,
        parents: &[Node<'tree>],
        source: &str,
        walk: &mut NestedWalk<'tree>,
    ) {
        let mut cursor = body.walk();
        if body.kind() == "dictionary" {
            for member in body.named_children(&mut cursor) {
                if member.kind() != "pair" {
                    continue;
                }
                let Some(key) = member
                    .child_by_field_name("key")
                    .map(code_symbols::without_parentheses)
                else {
                    continue;
                };
                // Only decoded string keys become symbols. Bytes, f-strings with fields,
                // undecodable escapes and non-string keys are skipped with everything under them.
                if !matches!(self.decode_string(&key, source), Ok(Some(_))) {
                    continue;
                }
                let value = member.child_by_field_name("value");
                let dict = dict_body(value);
                walk.add_definition_with_body(parents, key, member.byte_range(), value, dict);
            }
            return;
        }
        for member in body.named_children(&mut cursor) {
            match member.kind() {
                // Python gives the names in these bodies to the enclosing module or class, as it
                // does at the top level. A loop or a `match` binds a name each time it runs, so
                // its body doesn't count.
                "block" | "if_statement" | "elif_clause" | "else_clause" | "try_statement"
                | "except_clause" | "finally_clause" | "with_statement" => {
                    walk.add_body(member, parents.to_vec());
                }
                "expression_statement" => {
                    if let Some(assignment) = member
                        .named_child(0)
                        .filter(|child| child.kind() == "assignment")
                    {
                        push_assignment(member, assignment, parents, source, walk);
                    }
                }
                "function_definition" | "class_definition" => {
                    push_named(member, member, parents, source, walk);
                }
                "decorated_definition" => {
                    if let Some(definition) = member.child_by_field_name("definition") {
                        push_named(member, definition, parents, source, walk);
                    }
                }
                "type_alias_statement" => {
                    if let Some(name) = member
                        .child_by_field_name("left")
                        .and_then(|alias| alias.named_child(0))
                        // `type Pair[T] = ...` writes its name in a generic type.
                        .and_then(|alias| match alias.kind() {
                            "generic_type" => alias.named_child(0),
                            _ => Some(alias),
                        })
                        .filter(|name| is_name(name, source))
                    {
                        walk.add_definition(parents, name, member.byte_range(), None);
                    }
                }
                _ => {}
            }
        }
    }
}

/// Adds the definition of `statement`, which holds `assignment`, to `walk`, with `parents` as its
/// enclosing classes. In `A = B = 5`, both names share the definition. When the value is a dict
/// literal, adds the dict to `walk` under each assigned name.
fn push_assignment<'tree>(
    statement: Node<'tree>,
    mut assignment: Node<'tree>,
    parents: &[Node<'tree>],
    source: &str,
    walk: &mut NestedWalk<'tree>,
) {
    let mut names = Vec::new();
    let value = loop {
        names.extend(assignment.child_by_field_name("left"));
        match assignment.child_by_field_name("right") {
            Some(right) if right.kind() == "assignment" => assignment = right,
            // `X: int` declares a name without a value.
            value => break value,
        }
    };
    names.retain(|name| is_name(name, source));
    if names.is_empty() {
        return;
    }
    let dict = dict_body(value);
    walk.definitions.push(Definition {
        parents: parents.to_vec(),
        names: names.clone(),
        range: statement.byte_range(),
        value: value.map(Value::Initializer),
    });
    if let Some(dict) = dict {
        // `NestedWalk` reads the last body first, so reversing keeps the names' order.
        for name in names.into_iter().rev() {
            walk.add_body(dict, [parents, &[name]].concat());
        }
    }
}

/// Adds the definition of `target`, a function or a class, which `node` holds with its decorators,
/// to `walk`. For a class, adds its body to `walk` under its name.
fn push_named<'tree>(
    node: Node<'tree>,
    target: Node<'tree>,
    parents: &[Node<'tree>],
    source: &str,
    walk: &mut NestedWalk<'tree>,
) {
    let Some(name) = target
        .child_by_field_name("name")
        .filter(|name| is_name(name, source))
    else {
        return;
    };
    let body = target
        .child_by_field_name("body")
        .filter(|_| target.kind() == "class_definition");
    walk.add_definition_with_body(parents, name, node.byte_range(), None, body);
}

/// The dict literal inside `value`, without parentheses around it, or `None` when `value` is not a
/// dict literal.
fn dict_body(value: Option<Node<'_>>) -> Option<Node<'_>> {
    value
        .map(code_symbols::without_parentheses)
        .filter(|value| value.kind() == "dictionary")
}

/// Whether `node` is an identifier that code can refer to.
fn is_name(node: &Node, source: &str) -> bool {
    // `_` names a value that the code throws away.
    node.kind() == "identifier" && node_text(node, source) != "_"
}

/// Adds the text of `content`, a part of a string between its quotes and its fields, to `text`.
/// `is_raw` tells whether the string is raw, such as `r"\d"`.
///
/// # Errors
/// Returns an error if an escape gives a character by its name, stands for no character, or is
/// cut short, such as `\x4`.
fn push_content(
    content: &Node,
    source: &str,
    is_raw: bool,
    text: &mut String,
) -> anyhow::Result<()> {
    let mut written_up_to = content.start_byte();
    let mut cursor = content.walk();
    for part in content.named_children(&mut cursor) {
        push_written(source, written_up_to..part.start_byte(), is_raw, text)?;
        let escape = node_text(&part, source);
        match part.kind() {
            "escape_sequence" => push_escape(escape, text)?,
            // An `escape_interpolation`, which only an f-string has: `{{` and `}}` stand for one
            // brace.
            _ => text.extend(escape.chars().next()),
        }
        written_up_to = part.end_byte();
    }
    push_written(source, written_up_to..content.end_byte(), is_raw, text)
}

/// Adds the text of `source` in `range`, which holds no escapes, to `text`. `is_raw` tells
/// whether the string is raw.
///
/// # Errors
/// Returns an error if a string that is not raw holds an escape that is cut short, such as `\x4`
/// or `\N` without a name.
fn push_written(
    source: &str,
    range: std::ops::Range<usize>,
    is_raw: bool,
    text: &mut String,
) -> anyhow::Result<()> {
    #[expect(
        clippy::string_slice,
        reason = "tree-sitter nodes start and end on whole characters"
    )]
    let written = &source[range];
    // The grammar reads only a whole escape as an escape. So a backslash left in the text starts
    // an escape that Python keeps as written, such as `\d`, or one that it rejects.
    // Starlark reads its strings the same way, so the messages name no language.
    let cut_short = |after: &str| match after.chars().next() {
        Some('x') => Some("`\\x` needs 2 hex digits"),
        Some('u') => Some("`\\u` needs 4 hex digits"),
        Some('U') => Some("`\\U` needs 8 hex digits"),
        Some('N') => Some("`\\N` needs a character name in braces"),
        _ => None,
    };
    if !is_raw && let Some(problem) = written.split('\\').skip(1).find_map(cut_short) {
        bail!("{problem}");
    }
    // Python reads a CRLF line end in a file as a line feed.
    text.push_str(&written.replace("\r\n", "\n"));
    Ok(())
}

/// Adds the character that `escape`, such as `\n` or `\x41`, stands for to `text`. An escape
/// Python doesn't know, such as `\d`, stays as written.
///
/// # Errors
/// Returns an error if `escape` gives a character by its name, or stands for no character.
fn push_escape(escape: &str, text: &mut String) -> anyhow::Result<()> {
    // The grammar starts each escape with a backslash.
    let mut characters = escape.chars();
    characters.next();
    let rest = characters.as_str();
    let first = code_symbols::escaped_character(&mut characters)?;
    let digits = characters.as_str();
    match first {
        // An octal escape takes up to three digits, so `\18` is `\1` and `8`. The grammar also
        // reads `\8` as an escape, which Python keeps as written.
        '0'..='7' => {
            let octal = rest.chars().take_while(|c| matches!(c, '0'..='7')).count();
            #[expect(clippy::string_slice, reason = "the digits are ASCII")]
            let (digits, after) = (&rest[..octal], &rest[octal..]);
            text.push(code_symbols::char_from_digits(escape, digits, 8)?);
            text.push_str(after);
        }
        'x' | 'u' | 'U' => text.push(code_symbols::char_from_digits(escape, digits, 16)?),
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

fn comments_parser() -> impl CommentsParser {
    let python_language = tree_sitter_python::LANGUAGE.into();
    // Sphinx reads a comment that starts with `#:` as the doc of the attribute below it.
    python_style_comments_parser(&python_language, "comment", &["#:", "#"])
}

#[cfg(test)]
mod symbols_tests {
    use super::*;
    use crate::languages::code_symbols::test_utils::{
        decode_error, symbol, symbols, syntax_error, unresolved, value, values,
    };
    use crate::symbols::{Symbols, SymbolsParser};

    /// The symbols of `source`, a Python file.
    fn python(source: &str) -> anyhow::Result<Symbols> {
        CodeSymbolsParser::new(&tree_sitter_python::LANGUAGE.into(), Python).parse(source)
    }

    /// The symbols of `source`, a Starlark file.
    fn starlark(source: &str) -> anyhow::Result<Symbols> {
        CodeSymbolsParser::new(&tree_sitter_starlark::LANGUAGE.into(), Python).parse(source)
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

        // <block affects="docs/symbols.md:python-paths">
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
                symbol("/PRESETS/a", r#""a": "b""#, Some("b")),
                symbol("/WRAPPED", "WRAPPED = (32)", Some("32")),
                symbol("/X", "X = 1", Some("1")),
                symbol("/Y", "Y = 2", Some("2")),
                symbol("/__all__", r#"__all__ = ["load"]"#, Some(r#"["load"]"#)),
                symbol("/Alias", "type Alias = int", None),
                symbol("/Pair", "type Pair[T] = tuple[T, T]", None),
                symbol("/load", "def load(path):\n    return path", None),
                symbol("/fetch", "async def fetch():\n    pass", None),
                symbol("/Point", "@dataclass\nclass Point:\n    x: int = 0", None),
                symbol("/Point/x", "x: int = 0", Some("0")),
                symbol("/MenuItem", "class MenuItem(Enum):\n    RED = 1", None),
                symbol("/MenuItem/RED", "RED = 1", Some("1")),
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
    fn class_members_and_nested_classes_parse_symbols_returns_every_member_in_order()
    -> anyhow::Result<()> {
        let source = r#"class Color(StrEnum):
    RED = "red"
    GREEN = "green"


@dataclass(frozen=True)
class Record:
    label: str
    version: int = 1
    A = B = 2
    _ = 3
    a, b = 4, 5
    X = 6; Y = 7
    DEFAULTS = {"mode": "fast"}
    type Kind = str

    def __init__(self):
        self.instance_only = 1
        local_var = 2
        class LocalClass:
            HIDDEN = 3

    @classmethod
    def from_dict(cls, data):
        return cls()

    async def run(self):
        pass

    @dataclass
    class Meta:
        revision: int = 2

    if TYPE_CHECKING:
        COND = 1
    elif OTHER:
        ELIF_COND = 2
    else:
        COND = 3

    try:
        TRIED = 1
    except ImportError:
        EXCEPTED = 2
    else:
        TRY_ELSE = 3
    finally:
        FINALLY = 4

    try:
        GROUP_TRIED = 1
    except* ValueError:
        GROUP_EXCEPTED = 2

    with open("f"):
        WITH_NAME = 1

    for i in range(2):
        LOOPED = i

    while ready:
        WHILE_NAME = 1

    match mode:
        case 1:
            MATCHED = 1


class _:
    SKIPPED = 1
"#;

        assert_eq!(
            symbols(&python(source)?, source),
            vec![
                symbol(
                    "/Color",
                    "class Color(StrEnum):\n    RED = \"red\"\n    GREEN = \"green\"",
                    None
                ),
                symbol("/Color/RED", r#"RED = "red""#, Some("red")),
                symbol("/Color/GREEN", r#"GREEN = "green""#, Some("green")),
                symbol(
                    "/Record",
                    source
                        .split("\n\n\n")
                        .nth(1)
                        .expect("Record is the second block"),
                    None
                ),
                symbol("/Record/label", "label: str", None),
                symbol("/Record/version", "version: int = 1", Some("1")),
                symbol("/Record/A", "A = B = 2", Some("2")),
                symbol("/Record/B", "A = B = 2", Some("2")),
                symbol("/Record/X", "X = 6", Some("6")),
                symbol("/Record/Y", "Y = 7", Some("7")),
                symbol(
                    "/Record/DEFAULTS",
                    r#"DEFAULTS = {"mode": "fast"}"#,
                    Some(r#"{"mode": "fast"}"#)
                ),
                symbol("/Record/DEFAULTS/mode", r#""mode": "fast""#, Some("fast")),
                symbol("/Record/Kind", "type Kind = str", None),
                symbol(
                    "/Record/__init__",
                    "def __init__(self):\n        self.instance_only = 1\n        local_var = 2\n        class LocalClass:\n            HIDDEN = 3",
                    None
                ),
                symbol(
                    "/Record/from_dict",
                    "@classmethod\n    def from_dict(cls, data):\n        return cls()",
                    None
                ),
                symbol("/Record/run", "async def run(self):\n        pass", None),
                symbol(
                    "/Record/Meta",
                    "@dataclass\n    class Meta:\n        revision: int = 2",
                    None
                ),
                symbol("/Record/Meta/revision", "revision: int = 2", Some("2")),
                symbol("/Record/COND", "COND = 1", Some("1")),
                symbol("/Record/ELIF_COND", "ELIF_COND = 2", Some("2")),
                symbol("/Record/COND", "COND = 3", Some("3")),
                symbol("/Record/TRIED", "TRIED = 1", Some("1")),
                symbol("/Record/EXCEPTED", "EXCEPTED = 2", Some("2")),
                symbol("/Record/TRY_ELSE", "TRY_ELSE = 3", Some("3")),
                symbol("/Record/FINALLY", "FINALLY = 4", Some("4")),
                symbol("/Record/GROUP_TRIED", "GROUP_TRIED = 1", Some("1")),
                symbol("/Record/GROUP_EXCEPTED", "GROUP_EXCEPTED = 2", Some("2")),
                symbol("/Record/WITH_NAME", "WITH_NAME = 1", Some("1")),
            ]
        );
        Ok(())
    }

    #[test]
    fn dict_keys_parse_symbols_returns_decoded_string_keys_and_nested_dicts() -> anyhow::Result<()>
    {
        let source = r#"PRESETS: dict[str, str] = {
    "fast-mode": "fast",
    'safe-mode': 'safe',
}

NESTED = ({
    "db": ({
        "host": "localhost",
        "port": (5432),
    }),
    """triple""": 1,
    r"raw\n": 2,
    u"unicode": 3,
    f"brace{{}}": 4,
    ("joined-"
     "key"): 5,
    "esc\"quote": 6,
    "a/b": 7,
    "": 8,
    "_": 9,
})

A = B = {"k": {"x": 1}, "m": 2}

SKIPPED = {
    b"bytes": 1,
    f"{var}": 2,
    123: 3,
    True: 4,
    None: 5,
    IDENT: 6,
    ("a",): 7,
    ("a", "b"): 8,
    **OTHER,
    "kept": 9,
}

COMP = {k: v for k, v in items}
SET = {"a", "b"}
EMPTY = {}
_ = {"ignored": 1}
"#;

        assert_eq!(
            values(&python(source)?, source),
            vec![
                value(
                    "/PRESETS",
                    "{\n    \"fast-mode\": \"fast\",\n    'safe-mode': 'safe',\n}"
                ),
                value("/PRESETS/fast-mode", "fast"),
                value("/PRESETS/safe-mode", "safe"),
                value(
                    "/NESTED",
                    "{\n    \"db\": ({\n        \"host\": \"localhost\",\n        \"port\": (5432),\n    }),\n    \"\"\"triple\"\"\": 1,\n    r\"raw\\n\": 2,\n    u\"unicode\": 3,\n    f\"brace{{}}\": 4,\n    (\"joined-\"\n     \"key\"): 5,\n    \"esc\\\"quote\": 6,\n    \"a/b\": 7,\n    \"\": 8,\n    \"_\": 9,\n}"
                ),
                value(
                    "/NESTED/db",
                    "{\n        \"host\": \"localhost\",\n        \"port\": (5432),\n    }"
                ),
                value("/NESTED/db/host", "localhost"),
                value("/NESTED/db/port", "5432"),
                value("/NESTED/triple", "1"),
                value(r"/NESTED/raw\n", "2"),
                value("/NESTED/unicode", "3"),
                value("/NESTED/brace{}", "4"),
                value("/NESTED/joined-key", "5"),
                value("/NESTED/esc%22quote", "6"),
                value("/NESTED/a~1b", "7"),
                value("/NESTED/", "8"),
                value("/NESTED/_", "9"),
                value("/A", r#"{"k": {"x": 1}, "m": 2}"#),
                value("/B", r#"{"k": {"x": 1}, "m": 2}"#),
                value("/A/k", r#"{"x": 1}"#),
                value("/B/k", r#"{"x": 1}"#),
                value("/A/k/x", "1"),
                value("/B/k/x", "1"),
                value("/A/m", "2"),
                value("/B/m", "2"),
                value(
                    "/SKIPPED",
                    "{\n    b\"bytes\": 1,\n    f\"{var}\": 2,\n    123: 3,\n    True: 4,\n    None: 5,\n    IDENT: 6,\n    (\"a\",): 7,\n    (\"a\", \"b\"): 8,\n    **OTHER,\n    \"kept\": 9,\n}"
                ),
                value("/SKIPPED/kept", "9"),
                value("/COMP", "{k: v for k, v in items}"),
                value("/SET", r#"{"a", "b"}"#),
                value("/EMPTY", "{}"),
            ]
        );
        Ok(())
    }

    #[test]
    fn duplicate_dict_keys_parse_symbols_reports_ambiguous() -> anyhow::Result<()> {
        let source = r#"D = {
    "a": 1,
    'a': 2,
    "\x61": 3,
}
"#;

        assert_eq!(
            unresolved(&python(source)?, source, "/D/a")?,
            "ambiguous symbol, defined at 2:5, 3:5, 4:5"
        );
        Ok(())
    }

    #[test]
    fn undecodable_dict_key_or_value_parse_symbols_keeps_other_entries_readable()
    -> anyhow::Result<()> {
        let source = r#"D = {
    "\N{EM DASH}": {"nested": 1},
    "\ud800": 2,
    "\x4": 3,
    "bad_val": "\ud800",
    "ok": "fine",
}
"#;

        let parsed = python(source)?;
        assert_eq!(
            values(&parsed, source),
            vec![
                value(
                    "/D",
                    "{\n    \"\\N{EM DASH}\": {\"nested\": 1},\n    \"\\ud800\": 2,\n    \"\\x4\": 3,\n    \"bad_val\": \"\\ud800\",\n    \"ok\": \"fine\",\n}"
                ),
                value("/D/ok", "fine"),
            ]
        );
        assert_eq!(
            unresolved(&parsed, source, "/D/bad_val")?,
            decode_error(5, 16, r"`\ud800` stands for no character")
        );
        Ok(())
    }

    #[test]
    fn syntax_error_in_class_or_dict_parse_symbols_makes_affected_paths_unreadable()
    -> anyhow::Result<()> {
        let source = r#"class Broken:
    BAD = [1, for]
    OK = 2

CONFIG = {
    "fast-mode": "fast",
    "bad": [1, for],
    "double-broken" "missing-colon",
    'single-broken' 'missing-colon',
}
"#;

        let parsed = python(source)?;
        assert_eq!(
            values(&parsed, source),
            vec![value("/Broken/OK", "2"), value("/CONFIG/fast-mode", "fast"),]
        );
        assert_eq!(
            unresolved(&parsed, source, "/Broken")?,
            syntax_error(2, 15, "Broken")
        );
        assert_eq!(
            unresolved(&parsed, source, "/Broken/BAD")?,
            syntax_error(2, 15, "BAD")
        );
        assert_eq!(
            unresolved(&parsed, source, "/CONFIG")?,
            syntax_error(7, 16, "CONFIG")
        );
        assert_eq!(
            unresolved(&parsed, source, "/CONFIG/bad")?,
            syntax_error(7, 16, "bad")
        );
        assert_eq!(
            unresolved(&parsed, source, "/CONFIG/double-broken")?,
            syntax_error(8, 5, "double-broken")
        );
        assert_eq!(
            unresolved(&parsed, source, "/CONFIG/single-broken")?,
            syntax_error(8, 5, "single-broken")
        );
        Ok(())
    }

    #[test]
    fn error_with_quotes_parse_symbols_leaves_unrelated_empty_key_readable() -> anyhow::Result<()> {
        // Error spans ending on a quote, holding `"""..."""`, or holding adjacent quotes must not
        // add `""` to `skipped`.
        let source = r#"GOOD = {"": "ok"}
BAD = {"k": 1, "a" "b"}
BAD_SINGLE = {'k': 1, 'c''d'}
BAD_TRIPLE = {"k": 1, """e""" """f"""}
def broken(:
    """Docstring."""
    pass
"#;

        let parsed = python(source)?;
        assert_eq!(
            parsed
                .resolve(&crate::symbols::SymbolPath::parse("/GOOD/")?)
                .map(|s| s.value.as_ref().map(|v| v.text.as_str())),
            Ok(Some("ok"))
        );
        assert_eq!(
            unresolved(&parsed, source, "/BAD/a")?,
            syntax_error(2, 16, "a")
        );
        assert_eq!(
            unresolved(&parsed, source, "/BAD_SINGLE/c")?,
            syntax_error(3, 23, "c")
        );
        assert_eq!(
            unresolved(&parsed, source, "/BAD_TRIPLE/e")?,
            syntax_error(4, 23, "e")
        );
        Ok(())
    }

    #[test]
    fn starlark_dict_parse_symbols_returns_nested_keys() -> anyhow::Result<()> {
        let source = r#"CONFIG = {
    "env": "prod",
    "limits": {
        "cpu": "4",
    },
}
"#;

        assert_eq!(
            values(&starlark(source)?, source),
            vec![
                value(
                    "/CONFIG",
                    "{\n    \"env\": \"prod\",\n    \"limits\": {\n        \"cpu\": \"4\",\n    },\n}"
                ),
                value("/CONFIG/env", "prod"),
                value("/CONFIG/limits", "{\n        \"cpu\": \"4\",\n    }"),
                value("/CONFIG/limits/cpu", "4"),
            ]
        );
        Ok(())
    }

    #[test]
    fn deeply_nested_classes_and_dicts_parse_symbols_do_not_overflow_stack() -> anyhow::Result<()> {
        // Each nested class or dict has a path one segment longer than the one around it, and a
        // recursive walk would overflow the call stack.
        let class_depth = 500;
        let classes: String = (0..class_depth)
            .map(|i| format!("{}class C:\n", " ".repeat(i)))
            .chain(std::iter::once(format!(
                "{}pass\n",
                " ".repeat(class_depth)
            )))
            .collect();
        assert_eq!(python(&classes)?.symbols.len(), class_depth);

        let dict_depth = 3_000;
        let dicts = format!(
            "D = {}{{\"k\": 1}}{}\n",
            "{\"a\": ".repeat(dict_depth - 1),
            "}".repeat(dict_depth - 1)
        );
        // D plus `dict_depth - 1` "a" keys plus 1 "k" key.
        assert_eq!(python(&dicts)?.symbols.len(), dict_depth + 1);
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
    fn truncated_escape_parse_symbols_makes_the_name_unreadable() -> anyhow::Result<()> {
        // The grammar reads none of these as an escape, but Python rejects each one. A raw string
        // has no escapes.
        let source = r#"SHORT_X = "\x4"
SHORT_U = "a\u00e"
SIGNED = "\U+0000041"
NAMELESS = f"\N"
UNCLOSED = "\N{x"
RAW = r"\x4\N"
UPPER_RAW = R"\u12"
"#;

        assert_eq!(
            symbols(&python(source)?, source),
            vec![
                symbol("/RAW", r#"RAW = r"\x4\N""#, Some(r"\x4\N")),
                symbol("/UPPER_RAW", r#"UPPER_RAW = R"\u12""#, Some(r"\u12")),
            ]
        );
        assert_eq!(
            unresolved(&python(source)?, source, "/SHORT_X")?,
            decode_error(1, 11, r"`\x` needs 2 hex digits")
        );
        assert_eq!(
            unresolved(&python(source)?, source, "/SHORT_U")?,
            decode_error(2, 11, r"`\u` needs 4 hex digits")
        );
        assert_eq!(
            unresolved(&python(source)?, source, "/SIGNED")?,
            decode_error(3, 10, r"`\U` needs 8 hex digits")
        );
        assert_eq!(
            unresolved(&python(source)?, source, "/NAMELESS")?,
            decode_error(4, 12, r"`\N` needs a character name in braces")
        );
        assert_eq!(
            unresolved(&python(source)?, source, "/UNCLOSED")?,
            decode_error(5, 12, r"`\N` needs a character name in braces")
        );
        Ok(())
    }

    #[test]
    fn syntax_errors_parse_symbols_makes_the_names_near_them_unreadable() -> anyhow::Result<()> {
        let source = r"PORTS = 8
def helper(:
    return 1
BROKEN = = 1
AFTER = 3
";

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
    use crate::{languages::Comment, position::Position};

    #[test]
    fn parses_comments_correctly() {
        let mut comments_parser = comments_parser();

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
## A section heading repeats the marker.
#: Sphinx reads this marker as the doc of the attribute below it.
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
                Comment {
                    position_range: Position::new(11, 1)..Position::new(11, 41),
                    source_range: 192..232,
                    comment_text: "   A section heading repeats the marker.".to_string()
                },
                Comment {
                    position_range: Position::new(12, 1)..Position::new(12, 66),
                    source_range: 233..298,
                    comment_text:
                        "   Sphinx reads this marker as the doc of the attribute below it."
                            .to_string()
                },
            ]
        );
    }
}
