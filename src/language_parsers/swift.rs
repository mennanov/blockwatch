use crate::language_parsers::{
    CommentsParser, LanguageParser, LanguageParserImpl,
    c_style_and_doc_line_and_block_comments_parser, node_text,
};
use crate::symbols::code::{self, CodeLanguage, CodeSymbolsParser, Definition, child};
use anyhow::{Context, anyhow, bail};
use tree_sitter::Node;

/// Returns a [`LanguageParser`] for Swift, with its symbols.
pub(super) fn parser() -> anyhow::Result<impl LanguageParser> {
    Ok(
        LanguageParserImpl::new(comments_parser()?).with_symbols(CodeSymbolsParser::new(
            &tree_sitter_swift::LANGUAGE.into(),
            Swift,
        )?),
    )
}

/// Swift, whose files hold properties, functions and types, and whose types and extensions hold
/// their members.
struct Swift;

impl CodeLanguage for Swift {
    /// The definitions under `root`, the root of a Swift file's syntax tree, and the members of
    /// its types and extensions, in document order. A type comes before its members. The members
    /// of an extension are members of the type it extends.
    fn definitions<'tree>(&self, root: Node<'tree>, _source: &str) -> Vec<Definition<'tree>> {
        code::nested_definitions(root, push_members)
    }

    /// Decodes a Swift string: one in `"` quotes, a multi-line one in `"""` quotes, or a raw one
    /// with `#` around either. Returns `None` for any other node, and for a string with an
    /// interpolation, such as `"hi \(name)"`.
    ///
    /// # Errors
    /// Returns an error if an escape is not one of Swift's, if a string that is not multi-line
    /// holds a line break, or if a multi-line string breaks one of Swift's rules for its lines.
    fn decode_string(&self, node: &Node, source: &str) -> anyhow::Result<Option<String>> {
        let kinds = [
            "line_string_literal",
            "multi_line_string_literal",
            "raw_string_literal",
        ];
        if !kinds.contains(&node.kind()) || node.child_by_field_name("interpolation").is_some() {
            return Ok(None);
        }
        let literal = node_text(node, source);
        // A raw string has `#` around its quotes, and its escapes have as many.
        let hashes = literal.len() - literal.trim_start_matches('#').len();
        let escape = format!("\\{}", "#".repeat(hashes));
        let quoted = literal.trim_matches('#');
        let multi_line = quoted
            .strip_prefix(MULTI_LINE_QUOTES)
            .and_then(|content| content.strip_suffix(MULTI_LINE_QUOTES));
        let text = match multi_line {
            Some(content) => multi_line_text(content)?,
            None => {
                let content = quoted
                    .strip_prefix('"')
                    .and_then(|content| content.strip_suffix('"'))
                    .context("a string is between its quotes")?;
                // The grammar lets a string run on to the next line, so a string missing its
                // closing quote takes in the next line too.
                if content.contains(['\r', '\n']) {
                    bail!("Swift allows a line break only in a multi-line string");
                }
                content.to_string()
            }
        };
        unescape(&text, &escape).map(Some)
    }

    const NAME_QUOTE: Option<char> = Some('`');
}

/// The quotes that open and close a multi-line string.
const MULTI_LINE_QUOTES: &str = "\"\"\"";

/// The text of a multi-line string, before its escapes are read. `content` is what the string
/// holds between its quotes. The text is the lines between the line of the opening quotes and
/// the line of the closing quotes, each without the white space before the closing quotes,
/// joined by line feeds.
///
/// # Errors
/// Returns an error if text follows the opening quotes on their line, if text comes before the
/// closing quotes on their line, or if a line that is not empty doesn't start with the white
/// space before the closing quotes.
fn multi_line_text(content: &str) -> anyhow::Result<String> {
    // The compiler reads every line end of a file as a line feed.
    let content = content.replace("\r\n", "\n").replace('\r', "\n");
    let Some(("", rest)) = content.split_once('\n') else {
        bail!("Swift allows no text after the opening quotes of a multi-line string");
    };
    // With no line between the quotes, all of `rest` is the line of the closing quotes.
    let (lines, indent) = rest.rsplit_once('\n').unwrap_or(("", rest));
    if !indent
        .chars()
        .all(|character| matches!(character, ' ' | '\t'))
    {
        bail!("Swift allows no text before the closing quotes of a multi-line string");
    }
    let lines = lines
        .split('\n')
        .map(|line| match line {
            "" => Ok(line),
            _ => line.strip_prefix(indent).ok_or_else(|| {
                anyhow!(
                    "a line of the string doesn't start with the white space before its closing quotes"
                )
            }),
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(lines.join("\n"))
}

/// `text` with each escape replaced by what it stands for. `escape` starts an escape: a
/// backslash, and as many `#` as the string's quotes have.
///
/// # Errors
/// Returns an error if an escape is not one of Swift's.
fn unescape(text: &str, escape: &str) -> anyhow::Result<String> {
    let mut decoded = String::with_capacity(text.len());
    let mut rest = text;
    while let Some((before, after)) = rest.split_once(escape) {
        decoded.push_str(before);
        rest = push_escape(after, escape, &mut decoded)?;
    }
    decoded.push_str(rest);
    Ok(decoded)
}

/// Reads one escape from `text`, which starts right after its `escape`, and adds what it stands
/// for to `decoded`. Returns the text after the escape.
///
/// # Errors
/// Returns an error if the escape is not one of Swift's, or if it stands for no character.
fn push_escape<'text>(
    text: &'text str,
    escape: &str,
    decoded: &mut String,
) -> anyhow::Result<&'text str> {
    let mut characters = text.chars();
    // The text of a multi-line string ends with the line before the closing quotes.
    let Some(escaped) = characters.next() else {
        bail!("Swift allows no `\\` at the end of the line before the closing quotes");
    };
    let character = match escaped {
        '0' => '\0',
        't' => '\t',
        'n' => '\n',
        'r' => '\r',
        '"' | '\'' | '\\' => escaped,
        'u' => return push_unicode(characters.as_str(), escape, decoded),
        // A `\` at the end of a line of a multi-line string joins it to the next. White space
        // may come after it.
        ' ' | '\t' | '\n' => {
            return text
                .trim_start_matches([' ', '\t'])
                .strip_prefix('\n')
                .ok_or_else(|| anyhow!("`{escape}{escaped}` is not an escape in a Swift string"));
        }
        _ => bail!("`{escape}{escaped}` is not an escape in a Swift string"),
    };
    decoded.push(character);
    Ok(characters.as_str())
}

/// Reads the rest of a `\u{n}` escape from `text`, which starts right after its `u`, and adds the
/// character it stands for to `decoded`. Returns the text after the escape.
///
/// # Errors
/// Returns an error if the braces don't hold 1 to 8 hex digits, or if the digits stand for no
/// character.
fn push_unicode<'text>(
    text: &'text str,
    escape: &str,
    decoded: &mut String,
) -> anyhow::Result<&'text str> {
    let Some(braced) = text.strip_prefix('{') else {
        bail!("`{escape}u` is not an escape in a Swift string");
    };
    let end = braced
        .find(|character: char| !character.is_ascii_hexdigit())
        .unwrap_or(braced.len());
    let (digits, rest) = braced.split_at(end);
    let rest = rest.strip_prefix('}');
    let closing = if rest.is_some() { "}" } else { "" };
    let written = format!("{escape}u{{{digits}{closing}");
    let Some(rest) = rest.filter(|_| (1..=8).contains(&digits.len())) else {
        bail!("`{written}` is not an escape in a Swift string");
    };
    decoded.push(code::char_from_digits(&written, digits, 16)?);
    Ok(rest)
}

/// Adds the definitions of the members of `body`, the root of a file or the body of a type or
/// extension, to `definitions`. `parents` are the names of the types that `body` belongs to. Adds
/// the body of each type and extension in `body` to `bodies`, with its own parents.
fn push_members<'tree>(
    body: Node<'tree>,
    parents: Vec<Node<'tree>>,
    definitions: &mut Vec<Definition<'tree>>,
    bodies: &mut Vec<(Node<'tree>, Vec<Node<'tree>>)>,
) {
    let mut cursor = body.walk();
    for member in body.named_children(&mut cursor) {
        let name = match member.kind() {
            // The grammar calls a struct, an enum, an actor and an extension a class too.
            "class_declaration" | "protocol_declaration" => {
                push_type(member, &parents, definitions, bodies);
                continue;
            }
            "property_declaration" => {
                push_parts(member, &parents, definitions, |pattern| {
                    pattern.child_by_field_name("bound_identifier")
                });
                continue;
            }
            "enum_entry" => {
                push_parts(member, &parents, definitions, Some);
                continue;
            }
            "protocol_property_declaration" => member
                .child_by_field_name("name")
                .and_then(|pattern| pattern.child_by_field_name("bound_identifier")),
            // An operator function, such as `static func +++`, has no name to refer to.
            "function_declaration" | "protocol_function_declaration" => member
                .child_by_field_name("name")
                .filter(|name| name.kind() == "simple_identifier"),
            "typealias_declaration" | "associatedtype_declaration" => {
                member.child_by_field_name("name")
            }
            "macro_declaration" => child(member, "simple_identifier"),
            // Code never refers to an `init`, a `deinit` or a `subscript` by name, an operator or
            // a precedence group is not a name, and a script's top-level statements only hold
            // local definitions.
            _ => continue,
        };
        definitions.extend(
            name.map(|name| Definition::new(parents.to_vec(), name, member.byte_range(), None)),
        );
    }
}

/// Adds the definition of `declaration`, a type in the type at `parents`, to `definitions`, and
/// adds its body to `bodies`. An extension adds no definition, and its body gets the names of the
/// type it extends as parents. An extension of a type without a name, such as `[Int]`, adds
/// nothing.
fn push_type<'tree>(
    declaration: Node<'tree>,
    parents: &[Node<'tree>],
    definitions: &mut Vec<Definition<'tree>>,
    bodies: &mut Vec<(Node<'tree>, Vec<Node<'tree>>)>,
) {
    let Some(name) = declaration.child_by_field_name("name") else {
        return;
    };
    let is_extension = declaration
        .child_by_field_name("declaration_kind")
        .is_some_and(|kind| kind.kind() == "extension");
    let names = if is_extension {
        // Code reads an extension's members through the type, as `Config.extra`. A dotted type,
        // `Outer.Inner`, gives one segment for each part.
        if name.kind() != "user_type" {
            return;
        }
        let mut cursor = name.walk();
        name.named_children(&mut cursor)
            .filter(|part| part.kind() == "type_identifier")
            .collect()
    } else {
        definitions.push(Definition::new(
            parents.to_vec(),
            name,
            declaration.byte_range(),
            None,
        ));
        vec![name]
    };
    let parents = [parents, &names].concat();
    bodies.extend(
        declaration
            .child_by_field_name("body")
            .map(|body| (body, parents)),
    );
}

/// Adds a definition to `definitions` for each name that `declaration`, a property declaration or
/// an enum case in the type at `parents`, declares. `identifier` gives the identifier of the node
/// in the `name` field, or `None` when it has none, as a tuple pattern doesn't.
fn push_parts<'tree>(
    declaration: Node<'tree>,
    parents: &[Node<'tree>],
    definitions: &mut Vec<Definition<'tree>>,
    identifier: impl Fn(Node<'tree>) -> Option<Node<'tree>>,
) {
    // In `let a = 1, b = 2` and `case a = 1, b`, each name has its own part, so a change to one
    // does not touch the other. A part runs from its name to the next comma.
    let mut parts: Vec<Vec<(Option<&str>, Node)>> = Vec::new();
    let mut cursor = declaration.walk();
    for (index, node) in declaration.children(&mut cursor).enumerate() {
        let field = u32::try_from(index)
            .ok()
            .and_then(|index| declaration.field_name_for_child(index));
        if field == Some("name") {
            parts.push(Vec::new());
        }
        match parts.last_mut() {
            Some(part) if node.kind() != "," && !node.is_extra() => part.push((field, node)),
            _ => {}
        }
    }
    let alone = parts.len() == 1;
    for part in parts {
        let field = |name: &str| {
            part.iter()
                .find(|(field, _)| *field == Some(name))
                .map(|&(_, node)| node)
        };
        let Some(name) = field("name").and_then(&identifier) else {
            continue;
        };
        let range = match (alone, part.first(), part.last()) {
            (false, Some((_, first)), Some((_, last))) => first.start_byte()..last.end_byte(),
            _ => declaration.byte_range(),
        };
        let value = field("value")
            .or_else(|| field("raw_value"))
            .or_else(|| field("computed_value").and_then(computed_value));
        definitions.push(Definition::new(parents.to_vec(), name, range, value));
    }
}

/// The value of `property`, the body of a computed property: the `v` of `{ v }` or of
/// `{ return v }`. Returns `None` for a body with several statements, or with `get` or `set`.
fn computed_value(property: Node<'_>) -> Option<Node<'_>> {
    // Swift reads a body with one expression as a body that returns it.
    let statements = child(property, "statements")?;
    let mut cursor = statements.walk();
    let mut inner = statements
        .named_children(&mut cursor)
        .filter(|statement| !statement.is_extra());
    let (Some(statement), None) = (inner.next(), inner.next()) else {
        return None;
    };
    if statement.kind() == "control_transfer_statement" {
        statement.child_by_field_name("result")
    } else {
        Some(statement)
    }
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let swift_language = tree_sitter_swift::LANGUAGE.into();
    let parser = c_style_and_doc_line_and_block_comments_parser(
        &swift_language,
        "comment",
        "multiline_comment",
    )?;
    Ok(parser)
}

#[cfg(test)]
mod symbols_tests {
    use super::*;
    use crate::symbols::code::CodeSymbolsParser;
    use crate::symbols::code::test_utils::{
        decode_error, symbol, symbols, syntax_error, unresolved, value, values,
    };
    use crate::symbols::{Symbols, SymbolsParser};
    use anyhow::Context;

    /// The symbols of `source`, a Swift file.
    fn swift(source: &str) -> anyhow::Result<Symbols> {
        CodeSymbolsParser::new(&tree_sitter_swift::LANGUAGE.into(), Swift)?.parse(source)
    }

    #[test]
    fn file_with_every_shape_parse_symbols_returns_every_symbol_in_order() -> anyhow::Result<()> {
        let source = r#"import Foundation

/// Doc.
@available(iOS 13, *)
public let maxRetries = 20 * 1024
var counter: Int = (0)
let first = 1, second: Int = 2
let (left, right) = (1, 2)
var ready: Bool { counter > 0 }
var legacy: Bool {
    return counter > 0 // Positive.
}
var total: Int {
    get { counter }
    set { counter = newValue }
}
var steps: Int {
    let base = counter
    return base + 1
}
func top(a: Int) -> Int { a + 1 }
typealias Handler = (Int) -> Void
macro stringify<T>(_ value: T) -> (T, String) = #externalMacro(module: "M", type: "S")
infix operator +++
precedencegroup Loose {
    associativity: left
}

@MainActor
struct Config: Codable {
    static let origin = 0
    @Published var flag = false
    var name: String
    init() {}
    func run() {}
    func run(times: Int) {}
    subscript(i: Int) -> Int { i }
    static func +++ (a: Config, b: Config) -> Config { a }
    struct Limits {
        let id = 0x10
    }
}

final class Store<T> {
    class var shared: Int { 1 }
    deinit {}
}

enum Mode: Int {
    case red = 1, // Comes first.
        green
    case custom(String)
    static let all = [red]
}

protocol Named {
    associatedtype Item
    var label: String { get }
    func name() -> String
}

actor Counter {
    let id = 1
}
"#;
        // The text from the first `start` up to the first `end` after it.
        let text_of = |start: &str, end: &str| -> anyhow::Result<&str> {
            let start = source.find(start).context("no start")?;
            let end = start + source[start..].find(end).context("no end")?;
            Ok(&source[start..end])
        };

        // <block affects="docs/symbols.md:swift-paths, src/skill.md#/blockwatch/Symbols/Swift">
        assert_eq!(
            symbols(&swift(source)?, source),
            vec![
                symbol(
                    "/maxRetries",
                    "@available(iOS 13, *)\npublic let maxRetries = 20 * 1024",
                    Some("20 * 1024")
                ),
                symbol("/counter", "var counter: Int = (0)", Some("0")),
                // Each name of a declaration with several names covers only its own part.
                symbol("/first", "first = 1", Some("1")),
                symbol("/second", "second: Int = 2", Some("2")),
                symbol(
                    "/ready",
                    "var ready: Bool { counter > 0 }",
                    Some("counter > 0")
                ),
                symbol(
                    "/legacy",
                    "var legacy: Bool {\n    return counter > 0 // Positive.\n}",
                    Some("counter > 0")
                ),
                symbol("/total", text_of("var total", "\nvar steps")?, None),
                symbol("/steps", text_of("var steps", "\nfunc top")?, None),
                symbol("/top", "func top(a: Int) -> Int { a + 1 }", None),
                symbol("/Handler", "typealias Handler = (Int) -> Void", None),
                symbol("/stringify", text_of("macro", "\ninfix")?, None),
                symbol("/Config", text_of("@MainActor", "\n\nfinal class")?, None),
                symbol("/Config/origin", "static let origin = 0", Some("0")),
                symbol("/Config/flag", "@Published var flag = false", Some("false")),
                symbol("/Config/name", "var name: String", None),
                symbol("/Config/run", "func run() {}", None),
                symbol("/Config/run", "func run(times: Int) {}", None),
                symbol(
                    "/Config/Limits",
                    "struct Limits {\n        let id = 0x10\n    }",
                    None
                ),
                symbol("/Config/Limits/id", "let id = 0x10", Some("0x10")),
                symbol("/Store", text_of("final class", "\n\nenum")?, None),
                symbol("/Store/shared", "class var shared: Int { 1 }", Some("1")),
                symbol("/Mode", text_of("enum Mode", "\n\nprotocol")?, None),
                // Each name of a case covers only its own part. An implicit raw value is no
                // value.
                symbol("/Mode/red", "red = 1", Some("1")),
                symbol("/Mode/green", "green", None),
                symbol("/Mode/custom", "case custom(String)", None),
                symbol("/Mode/all", "static let all = [red]", Some("[red]")),
                symbol("/Named", text_of("protocol Named", "\n\nactor")?, None),
                symbol("/Named/Item", "associatedtype Item", None),
                symbol("/Named/label", "var label: String { get }", None),
                symbol("/Named/name", "func name() -> String", None),
                symbol("/Counter", "actor Counter {\n    let id = 1\n}", None),
                symbol("/Counter/id", "let id = 1", Some("1")),
            ]
        );
        // </block>
        Ok(())
    }

    #[test]
    fn extensions_parse_symbols_returns_their_members_under_the_type() -> anyhow::Result<()> {
        // `String` is not defined in the file, and `[Int]` is a type without a name.
        let source = r#"struct Config {
    let a = 1
}

extension Config: Equatable {
    static let b = 2

    enum Keys {
        static let c = 3
    }
}

extension Config.Keys {
    static let d = 4
}

extension String {
    var trimmed: String { self }
}

extension Array<Int> {
    var total: Int { 0 }
}

extension [Int] {
    var first: Int { 0 }
}
"#;

        assert_eq!(
            symbols(&swift(source)?, source),
            vec![
                symbol("/Config", "struct Config {\n    let a = 1\n}", None),
                symbol("/Config/a", "let a = 1", Some("1")),
                symbol("/Config/b", "static let b = 2", Some("2")),
                symbol(
                    "/Config/Keys",
                    "enum Keys {\n        static let c = 3\n    }",
                    None
                ),
                symbol("/Config/Keys/c", "static let c = 3", Some("3")),
                symbol("/Config/Keys/d", "static let d = 4", Some("4")),
                symbol(
                    "/String/trimmed",
                    "var trimmed: String { self }",
                    Some("self")
                ),
                symbol("/Array/total", "var total: Int { 0 }", Some("0")),
            ]
        );
        Ok(())
    }

    #[test]
    fn protocol_requirement_and_its_default_parse_symbols_returns_both() -> anyhow::Result<()> {
        let source = r#"protocol Named {
    func name() -> String
}

extension Named {
    func name() -> String { "" }
}
"#;

        assert_eq!(
            symbols(&swift(source)?, source),
            vec![
                symbol(
                    "/Named",
                    "protocol Named {\n    func name() -> String\n}",
                    None
                ),
                symbol("/Named/name", "func name() -> String", None),
                symbol("/Named/name", r#"func name() -> String { "" }"#, None),
            ]
        );
        Ok(())
    }

    #[test]
    fn copies_in_if_branches_parse_symbols_returns_each_copy() -> anyhow::Result<()> {
        let source = r#"#if os(iOS)
let platform = 1
#else
let platform = 2
#endif

struct Config {
    #if DEBUG
    static let level = 3
    #endif
}
"#;

        assert_eq!(
            values(&swift(source)?, source),
            [
                value("/platform", "1"),
                value("/platform", "2"),
                ("/Config".to_string(), None),
                value("/Config/level", "3"),
            ]
        );
        Ok(())
    }

    #[test]
    fn values_in_parentheses_parse_symbols_returns_them_without_the_parentheses()
    -> anyhow::Result<()> {
        // The grammar parses parentheses as a tuple. A tuple with a label or two values is a
        // tuple.
        let source = r#"let one = ((1))
let commented = (1 /* one */)
let labeled = (a: 1)
let pair = (1, 2)
let sum = (1) + (2)
"#;

        assert_eq!(
            values(&swift(source)?, source),
            [
                value("/one", "1"),
                value("/commented", "1"),
                value("/labeled", "(a: 1)"),
                value("/pair", "(1, 2)"),
                value("/sum", "(1) + (2)"),
            ]
        );
        Ok(())
    }

    #[test]
    fn names_in_backticks_parse_symbols_returns_them_without_the_backticks() -> anyhow::Result<()> {
        let source = r#"enum `Type` {
    case `default`

    static let `is` = 1
}
"#;

        assert_eq!(
            symbols(&swift(source)?, source),
            vec![
                symbol("/Type", source.trim_end(), None),
                symbol("/Type/default", "case `default`", None),
                symbol("/Type/is", "static let `is` = 1", Some("1")),
            ]
        );
        Ok(())
    }

    #[test]
    fn string_kinds_parse_symbols_returns_decoded_values() -> anyhow::Result<()> {
        let source = r###"let escapes = "a\t\0\\\"\'\r\n"
let unicode = "caf\u{e9} \u{1F600} \u{00000041}"
let notUnicode = "\\u{41}"
let interpolated = "hi \(name)"
let raw = #"C:\new "quoted" \#t\#u{41}"#
let rawTwice = ##"\#n \##t"##
let rawInterpolationText = #"\(x)"#
let rawInterpolated = #"hi \#(name)"#
let lines = """
    a
      b "quoted" \"""
    c\
     d\tz

    """
let rawLines = #"""
    a\n\#tb
    """#
let linesInterpolated = """
    \(x)
    """
let emptyLines = """
    """
let regex = /a+b/
let joined = "a" + "b"
"###;

        assert_eq!(
            values(&swift(source)?, source),
            [
                value("/escapes", "a\t\0\\\"'\r\n"),
                value("/unicode", "café 😀 A"),
                // The first backslash escapes the second, so no `\u` escape starts.
                value("/notUnicode", r"\u{41}"),
                value("/interpolated", r#""hi \(name)""#),
                // A raw string's escapes have as many `#` as its quotes.
                value("/raw", "C:\\new \"quoted\" \tA"),
                value("/rawTwice", "\\#n \t"),
                value("/rawInterpolationText", r"\(x)"),
                value("/rawInterpolated", r##"#"hi \#(name)"#"##),
                // Each line loses the white space before the closing quotes, and a `\` at the
                // end of a line joins it to the next.
                value("/lines", "a\n  b \"quoted\" \"\"\"\nc d\tz\n"),
                value("/rawLines", "a\\n\tb"),
                value("/linesInterpolated", "\"\"\"\n    \\(x)\n    \"\"\""),
                value("/emptyLines", ""),
                value("/regex", "/a+b/"),
                value("/joined", r#""a" + "b""#),
            ]
        );
        Ok(())
    }

    #[test]
    fn cr_and_crlf_line_endings_parse_symbols_reads_line_feeds() -> anyhow::Result<()> {
        // The compiler reads every line end of a multi-line string as a line feed.
        let source = "let lines = \"\"\"\r\n    a\r\n    b\r    c\r\n    \"\"\"\r\n";

        assert_eq!(
            values(&swift(source)?, source),
            [value("/lines", "a\nb\nc")]
        );
        Ok(())
    }

    #[test]
    fn white_space_after_a_line_joining_backslash_parse_symbols_joins_the_lines()
    -> anyhow::Result<()> {
        // The grammar itself rejects white space after the backslash in a string that is not raw.
        let source = "let lines = #\"\"\"\n    a\\# \t\n    b\n    \"\"\"#\n";

        assert_eq!(values(&swift(source)?, source), [value("/lines", "ab")]);
        Ok(())
    }

    #[test]
    fn white_space_indent_parse_symbols_removes_only_the_indent() -> anyhow::Result<()> {
        let source = concat!(
            "let lines = \"\"\"\n",
            "    a\n",
            "      \n",
            "\n",
            "    \"\"\"\n",
            "let tabs = \"\"\"\n",
            "\ta\n",
            "\t\"\"\"\n",
        );

        assert_eq!(
            values(&swift(source)?, source),
            [value("/lines", "a\n  \n"), value("/tabs", "a")]
        );
        Ok(())
    }

    #[test]
    fn string_that_swift_rejects_parse_symbols_makes_the_name_unreadable() -> anyhow::Result<()> {
        // The grammar itself rejects an unknown escape in a string that is not raw, so only a raw
        // string shows one here.
        let source = r###"let rawUnknown = #"\#q"#
let rawNoBrace = #"\#u41"#
let rawUnclosed = #"\#u{41"#
let rawEmpty = #"\#u{}"#
let nineDigits = "\u{000000041}"
let surrogate = "\u{D800}"
let tooBig = "\u{110000}"
let interpolated = "\u{D800} \(x)"
let fine = "ok"
"###;

        assert_eq!(
            values(&swift(source)?, source),
            [
                // A string with an interpolation is kept as written, so its escapes are not read.
                value("/interpolated", r#""\u{D800} \(x)""#),
                value("/fine", "ok"),
            ]
        );
        assert_eq!(
            unresolved(&swift(source)?, source, "/rawUnknown")?,
            decode_error(1, 18, r"`\#q` is not an escape in a Swift string")
        );
        assert_eq!(
            unresolved(&swift(source)?, source, "/rawNoBrace")?,
            decode_error(2, 18, r"`\#u` is not an escape in a Swift string")
        );
        assert_eq!(
            unresolved(&swift(source)?, source, "/rawUnclosed")?,
            decode_error(3, 19, r"`\#u{41` is not an escape in a Swift string")
        );
        assert_eq!(
            unresolved(&swift(source)?, source, "/rawEmpty")?,
            decode_error(4, 16, r"`\#u{}` is not an escape in a Swift string")
        );
        assert_eq!(
            unresolved(&swift(source)?, source, "/nineDigits")?,
            decode_error(5, 18, r"`\u{000000041}` is not an escape in a Swift string")
        );
        assert_eq!(
            unresolved(&swift(source)?, source, "/surrogate")?,
            decode_error(6, 17, r"`\u{D800}` stands for no character")
        );
        assert_eq!(
            unresolved(&swift(source)?, source, "/tooBig")?,
            decode_error(7, 14, r"`\u{110000}` stands for no character")
        );
        Ok(())
    }

    #[test]
    fn line_break_in_a_string_parse_symbols_makes_the_name_unreadable() -> anyhow::Result<()> {
        // Swift reads a carriage return alone as a line end too.
        let source = "let lf = \"a\nb\"\nlet cr = \"a\rb\"\n";

        assert_eq!(
            unresolved(&swift(source)?, source, "/lf")?,
            decode_error(
                1,
                10,
                "Swift allows a line break only in a multi-line string"
            )
        );
        assert_eq!(
            unresolved(&swift(source)?, source, "/cr")?,
            decode_error(
                3,
                10,
                "Swift allows a line break only in a multi-line string"
            )
        );
        Ok(())
    }

    #[test]
    fn multi_line_string_that_swift_rejects_parse_symbols_makes_the_name_unreadable()
    -> anyhow::Result<()> {
        let source = concat!(
            "let text = \"\"\"a\n",
            "    \"\"\"\n",
            // A space after the opening quotes.
            "let space = \"\"\" \n",
            "    \"\"\"\n",
            "let closing = \"\"\"\n",
            "    a\"\"\"\n",
            "let indent = \"\"\"\n",
            "    a\n",
            "  b\n",
            "    \"\"\"\n",
            // A line with only white space needs the indent too.
            "let blank = \"\"\"\n",
            "    a\n",
            "  \n",
            "    \"\"\"\n",
            // A tab is not the same white space as the closing line's spaces.
            "let tab = \"\"\"\n",
            "\ta\n",
            "    \"\"\"\n",
            "let last = \"\"\"\n",
            "    a\\\n",
            "    \"\"\"\n",
            "let fine = \"\"\"\n",
            "    ok\n",
            "    \"\"\"\n",
        );

        assert_eq!(values(&swift(source)?, source), [value("/fine", "ok")]);
        assert_eq!(
            unresolved(&swift(source)?, source, "/text")?,
            decode_error(
                1,
                12,
                "Swift allows no text after the opening quotes of a multi-line string"
            )
        );
        assert_eq!(
            unresolved(&swift(source)?, source, "/space")?,
            decode_error(
                3,
                13,
                "Swift allows no text after the opening quotes of a multi-line string"
            )
        );
        assert_eq!(
            unresolved(&swift(source)?, source, "/closing")?,
            decode_error(
                5,
                15,
                "Swift allows no text before the closing quotes of a multi-line string"
            )
        );
        assert_eq!(
            unresolved(&swift(source)?, source, "/indent")?,
            decode_error(
                7,
                14,
                "a line of the string doesn't start with the white space before its closing quotes"
            )
        );
        assert_eq!(
            unresolved(&swift(source)?, source, "/blank")?,
            decode_error(
                11,
                13,
                "a line of the string doesn't start with the white space before its closing quotes"
            )
        );
        assert_eq!(
            unresolved(&swift(source)?, source, "/tab")?,
            decode_error(
                15,
                11,
                "a line of the string doesn't start with the white space before its closing quotes"
            )
        );
        assert_eq!(
            unresolved(&swift(source)?, source, "/last")?,
            decode_error(
                18,
                12,
                r"Swift allows no `\` at the end of the line before the closing quotes"
            )
        );
        Ok(())
    }

    #[test]
    fn deeply_nested_types_parse_symbols_does_not_overflow_stack() -> anyhow::Result<()> {
        // Each type has a path one segment longer than the type around it, so the paths grow
        // with the square of the depth. This depth is enough to overflow a recursive walk.
        let depth = 3_000;
        let source = format!("{}{}", "struct S {\n".repeat(depth), "}\n".repeat(depth));

        let symbols = swift(&source)?;

        assert_eq!(symbols.symbols.len(), depth);
        Ok(())
    }

    #[test]
    fn syntax_error_parse_symbols_makes_the_definitions_that_hold_it_unreadable()
    -> anyhow::Result<()> {
        // The grammar can't parse the empty tuple as an argument, `(())`, and assumes a token is
        // missing.
        let source = r#"struct A {
    let x = 1
    func send() { call(()) }
    let y = 2
}

let after = 5
"#;

        assert_eq!(
            symbols(&swift(source)?, source),
            vec![
                symbol("/A/x", "let x = 1", Some("1")),
                symbol("/A/y", "let y = 2", Some("2")),
                symbol("/after", "let after = 5", Some("5")),
            ]
        );
        assert_eq!(
            unresolved(&swift(source)?, source, "/A")?,
            syntax_error(3, 25, "A")
        );
        assert_eq!(
            unresolved(&swift(source)?, source, "/A/send")?,
            syntax_error(3, 25, "send")
        );
        Ok(())
    }

    #[test]
    fn span_that_swallows_a_definition_parse_symbols_makes_its_name_unreadable()
    -> anyhow::Result<()> {
        // The unclosed `(` makes the parser skip `let w`.
        let source = r#"struct B {
    let z = (3
    let w = 4
}

let after = 5
"#;

        assert_eq!(
            symbols(&swift(source)?, source),
            vec![symbol("/after", "let after = 5", Some("5"))]
        );
        assert_eq!(
            unresolved(&swift(source)?, source, "/B/z")?,
            syntax_error(3, 5, "z")
        );
        assert_eq!(
            unresolved(&swift(source)?, source, "/B/w")?,
            syntax_error(3, 5, "w")
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Position, language_parsers::Comment};

    #[test]
    fn parses_swift_comments_correctly() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser()?;

        let blocks: Vec<Comment> = comments_parser
            .parse(
                r#"
            // This is a single-line comment in Swift.
            import Foundation
    
            /*
             * This is a multi-line comment.
             * It spans multiple lines in Swift.
             */
    
            func main() {
                print("Hello, Swift!") // Prints a message to the console.
    
                /* Another comment
                 * split into
                 * multiple lines.
                 */
                 
                return
            }
            /// A documentation comment.
            "#,
            )
            .collect();

        assert_eq!(
            blocks,
            vec![
                Comment {
                    position_range: Position::new(2, 13)..Position::new(2, 55),
                    source_range: 13..55,
                    comment_text: "   This is a single-line comment in Swift.".to_string()
                },
                Comment {
                    position_range: Position::new(5, 13)..Position::new(8, 16),
                    source_range: 103..215,
                    comment_text:
                        "  \n               This is a multi-line comment.\n               It spans multiple lines in Swift.\n               "
                            .to_string()
                },
                Comment {
                    position_range: Position::new(11, 40)..Position::new(11, 75),
                    source_range: 286..321,
                    comment_text: "   Prints a message to the console.".to_string()
                },
                Comment {
                    position_range: Position::new(13, 17)..Position::new(16, 20),
                    source_range: 343..446,
                    comment_text: "   Another comment\n                   split into\n                   multiple lines.\n                   ".to_string()
                },
                Comment {
                    position_range: Position::new(20, 13)..Position::new(20, 41),
                    source_range: 514..542,
                    comment_text: "    A documentation comment.".to_string()
                }
            ]
        );

        Ok(())
    }
}
