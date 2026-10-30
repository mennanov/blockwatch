use crate::language_parsers;
use crate::language_parsers::{CommentsParser, LanguageParser, LanguageParserImpl, node_text};
use crate::symbols::code::{self, CodeLanguage, CodeSymbolsParser, Definition};
use anyhow::bail;
use std::borrow::Cow;
use std::iter::{self, Peekable};
use std::str::Chars;
use tree_sitter::Node;

/// Returns a [`LanguageParser`] for C#, with its symbols.
pub(super) fn parser() -> anyhow::Result<impl LanguageParser> {
    Ok(
        LanguageParserImpl::new(comments_parser()?).with_symbols(CodeSymbolsParser::new(
            &tree_sitter_c_sharp::LANGUAGE.into(),
            CSharp,
        )?),
    )
}

/// C#, whose namespaces hold its types, and whose types hold its fields, properties, methods and
/// nested types.
struct CSharp;

impl CodeLanguage for CSharp {
    /// The types under `root`, the root of a C# file's syntax tree, and their members, in
    /// document order. A type comes before its members. The names of the namespaces a type sits
    /// in are its first parents.
    fn definitions<'tree>(&self, root: Node<'tree>, _source: &str) -> Vec<Definition<'tree>> {
        let mut definitions = Vec::new();
        // The walk keeps its own stack, as deeply nested types could overflow the call stack.
        let mut bodies = vec![(root, Vec::new())];
        while let Some((body, parents)) = bodies.pop() {
            push_members(body, parents, &mut definitions, &mut bodies);
        }
        // The walk reads all of a body before the bodies in it.
        definitions.sort_by_key(|definition| definition.range.start);
        definitions
    }

    /// Decodes a C# string, or an interpolated string without a hole. Returns `None` for any other
    /// node, and for a string of bytes such as `"abc"u8`.
    ///
    /// # Errors
    /// Returns an error if an escape is not one of C#'s or stands for no character, if the string
    /// holds half of a character, or if a raw string on several lines breaks C#'s rules for its
    /// lines.
    fn decode_string(&self, node: &Node, source: &str) -> anyhow::Result<Option<String>> {
        let is_string = matches!(
            node.kind(),
            "string_literal"
                | "verbatim_string_literal"
                | "raw_string_literal"
                | "interpolated_string_expression"
        );
        let literal = node_text(node, source);
        // A string of bytes ends with `u8`. What a hole holds is only known when the program runs.
        if !is_string || !literal.ends_with('"') || holds_hole(*node) {
            return Ok(None);
        }
        // `$` makes a string interpolated and `@` makes it verbatim. A string can be both.
        let quoted = literal.trim_start_matches(['$', '@']);
        let (prefix, _) = literal.split_at(literal.len() - quoted.len());
        let verbatim = prefix.contains('@');
        if !verbatim && quoted.starts_with(RAW_QUOTES) {
            return raw_text(quoted).map(Some);
        }
        let content = quoted
            .strip_prefix('"')
            .and_then(|content| content.strip_suffix('"'))
            .unwrap_or(quoted);
        // No escape holds a brace, so the braces can be read before the escapes.
        let content = if prefix.contains('$') {
            Cow::Owned(content.replace("{{", "{").replace("}}", "}"))
        } else {
            Cow::Borrowed(content)
        };
        if verbatim {
            Ok(Some(content.replace("\"\"", "\"")))
        } else {
            code::decode_utf16(&content, push_escape).map(Some)
        }
    }
}

/// The shortest run of quotes that opens and closes a raw string.
const RAW_QUOTES: &str = "\"\"\"";

/// Whether `string` has a hole, such as the `{name}` of `$"Hi {name}"`.
fn holds_hole(string: Node) -> bool {
    let mut cursor = string.walk();
    string
        .named_children(&mut cursor)
        .any(|part| part.kind() == "interpolation")
}

/// The text of a raw string, written as `quoted` with its quotes and without its `$`.
///
/// # Errors
/// Returns an error if the string is on several lines and has text on the line of its opening
/// quotes or of its closing quotes, has no line between them, or has a line that doesn't start
/// with the white space before its closing quotes.
fn raw_text(quoted: &str) -> anyhow::Result<String> {
    // C# rejects a raw string that starts or ends with a quote, so every quote at its ends is part
    // of its delimiters.
    let content = quoted.trim_matches('"');
    let Some((opening, lines)) = content.split_once('\n') else {
        return Ok(content.to_string());
    };
    if !opening.chars().all(char::is_whitespace) {
        bail!("a raw string on several lines has text after its opening quotes");
    }
    // The line break before the closing quotes is not part of the text.
    let Some((lines, indent)) = lines.rsplit_once('\n') else {
        bail!("a raw string on several lines has no line between its quotes");
    };
    if !indent.chars().all(char::is_whitespace) {
        bail!("a raw string on several lines has text before its closing quotes");
    }
    let lines = lines.strip_suffix('\r').unwrap_or(lines);
    let mut text = String::with_capacity(lines.len());
    for line in lines.split_inclusive('\n') {
        let (line, end) = line.split_at(line.trim_end_matches(['\r', '\n']).len());
        match line.strip_prefix(indent) {
            Some(line) => text.push_str(line),
            // C# lets a line of only white space be shorter than the indentation.
            None if indent.starts_with(line) => {}
            None => bail!(
                "a line of a raw string doesn't start with the white space before its closing quotes"
            ),
        }
        text.push_str(end);
    }
    Ok(text)
}

/// Reads one escape from `characters`, which start right after its backslash, and adds the UTF-16
/// code units it stands for to `units`.
///
/// # Errors
/// Returns an error if the escape is not one of C#'s, or if it is a `\U` escape that stands for no
/// character.
fn push_escape(characters: &mut Peekable<Chars>, units: &mut Vec<u16>) -> anyhow::Result<()> {
    // The grammar puts a character after every backslash in a string.
    let escaped = characters.next().unwrap_or_default();
    let character = match escaped {
        '\'' | '"' | '\\' => escaped,
        '0' => '\0',
        'a' => '\u{7}',
        'b' => '\u{8}',
        'e' => '\u{1b}',
        'f' => '\u{c}',
        'n' => '\n',
        'r' => '\r',
        't' => '\t',
        'v' => '\u{b}',
        // `\x` takes 1 to 4 hex digits. The grammar makes `\u` take 4.
        'x' | 'u' => {
            units.push(u16::from_str_radix(&hex_digits(characters, 4), 16)?);
            return Ok(());
        }
        'U' => {
            let digits = hex_digits(characters, 8);
            code::char_from_digits(&format!("\\U{digits}"), &digits, 16)?
        }
        // Such as `\?`, which the grammar takes as an escape.
        _ => bail!("`\\{escaped}` is not an escape in a C# string"),
    };
    units.extend(character.encode_utf16(&mut [0; 2]).iter());
    Ok(())
}

/// Takes up to `count` hex digits from the start of `characters`.
fn hex_digits(characters: &mut Peekable<Chars>, count: usize) -> String {
    iter::from_fn(|| characters.next_if(char::is_ascii_hexdigit))
        .take(count)
        .collect()
}

/// Adds the definitions in `body` to `definitions`. `body` is the root of a file, the body of a
/// namespace or a type, or a branch of `#if`. `parents` are the names of the namespaces and types
/// that `body` sits in. Adds each body in `body` to `bodies`, with its own parents.
fn push_members<'tree>(
    body: Node<'tree>,
    mut parents: Vec<Node<'tree>>,
    definitions: &mut Vec<Definition<'tree>>,
    bodies: &mut Vec<(Node<'tree>, Vec<Node<'tree>>)>,
) {
    let mut cursor = body.walk();
    for member in body.named_children(&mut cursor) {
        match member.kind() {
            // `namespace Acme;` has no body. The declarations after it are in the namespace.
            "file_scoped_namespace_declaration" => {
                if let Some(name) = member.child_by_field_name("name") {
                    parents.extend(namespace_segments(name));
                }
            }
            "namespace_declaration" => bodies.extend(namespace_body(member, &parents)),
            // A branch holds its definitions and the next branch, as if the directives were not
            // there.
            "preproc_if" | "preproc_elif" | "preproc_else" => {
                bodies.push((member, parents.clone()));
            }
            "class_declaration"
            | "struct_declaration"
            | "interface_declaration"
            | "enum_declaration"
            | "record_declaration"
            | "delegate_declaration" => push_type(member, &parents, definitions, bodies),
            "field_declaration" | "event_field_declaration" => {
                push_fields(member, &parents, definitions);
            }
            "property_declaration" if !is_explicit(member) => {
                definitions.extend(member_definition(member, &parents, property_value(member)));
            }
            // A method is a function, so `=> v` is not its value.
            "method_declaration" | "event_declaration" if !is_explicit(member) => {
                definitions.extend(member_definition(member, &parents, None));
            }
            "enum_member_declaration" => {
                let value = member.child_by_field_name("value");
                definitions.extend(member_definition(member, &parents, value));
            }
            // Code never refers to a constructor, a finalizer or an explicit interface member by
            // name, and indexers and operators have none. Top-level statements are the body of
            // the program's `Main` method, so what they define is local to it.
            _ => {}
        }
    }
}

/// The body of `declaration`, a namespace with braces in the namespace at `parents`, with the
/// names of the namespaces it sits in. Returns `None` if the parser found no name or no body.
fn namespace_body<'tree>(
    declaration: Node<'tree>,
    parents: &[Node<'tree>],
) -> Option<(Node<'tree>, Vec<Node<'tree>>)> {
    let name = declaration.child_by_field_name("name")?;
    let body = declaration.child_by_field_name("body")?;
    Some((body, [parents, &namespace_segments(name)].concat()))
}

/// The parts of `name`, the name of a namespace such as `Acme.Billing`, outermost first.
fn namespace_segments(name: Node<'_>) -> Vec<Node<'_>> {
    let mut segments = Vec::new();
    let mut rest = Some(name);
    while let Some(node) = rest {
        if node.kind() == "qualified_name" {
            segments.extend(node.child_by_field_name("name"));
            rest = node.child_by_field_name("qualifier");
        } else {
            segments.push(node);
            rest = None;
        }
    }
    segments.reverse();
    segments
}

/// Adds the definition of `declaration`, a type in the namespace or type at `parents`, to
/// `definitions`, and its body to `bodies`.
fn push_type<'tree>(
    declaration: Node<'tree>,
    parents: &[Node<'tree>],
    definitions: &mut Vec<Definition<'tree>>,
    bodies: &mut Vec<(Node<'tree>, Vec<Node<'tree>>)>,
) {
    let Some(name) = declaration.child_by_field_name("name") else {
        return;
    };
    definitions.push(Definition::new(
        parents.to_vec(),
        name,
        declaration.byte_range(),
        None,
    ));
    if let Some(body) = declaration.child_by_field_name("body") {
        bodies.push((body, [parents, &[name]].concat()));
    }
}

/// Whether `member` implements a member of an interface explicitly, as
/// `void IDisposable.Dispose()` does.
fn is_explicit(member: Node) -> bool {
    let mut cursor = member.walk();
    member
        .children(&mut cursor)
        .any(|child| child.kind() == "explicit_interface_specifier")
}

/// The value of `property`: the `v` of `X { get; } = v;` or of `X => v;`. Returns `None` for a
/// property without one, such as `X { get; set; }`.
fn property_value(property: Node) -> Option<Node> {
    let value = property.child_by_field_name("value")?;
    // Code reads a property as it reads a field, so `X => v` has the value `v`.
    if value.kind() == "arrow_expression_clause" {
        value_after(value, "=>")
    } else {
        Some(value)
    }
}

/// Adds a definition to `definitions` for each name that `declaration`, a field or event
/// declaration in the type at `parents`, declares.
fn push_fields<'tree>(
    declaration: Node<'tree>,
    parents: &[Node<'tree>],
    definitions: &mut Vec<Definition<'tree>>,
) {
    let mut cursor = declaration.walk();
    let Some(variables) = declaration
        .named_children(&mut cursor)
        .find(|child| child.kind() == "variable_declaration")
    else {
        return;
    };
    let declarators: Vec<Node> = variables
        .named_children(&mut cursor)
        .filter(|child| child.kind() == "variable_declarator")
        .collect();
    // In `int a = 1, b = 2;`, each name has its own part, so a change to one does not touch the
    // other.
    let alone = declarators.len() == 1;
    for declarator in declarators {
        let Some(name) = declarator.child_by_field_name("name") else {
            continue;
        };
        let range = if alone {
            declaration.byte_range()
        } else {
            declarator.byte_range()
        };
        definitions.push(Definition::new(
            parents.to_vec(),
            name,
            range,
            value_after(declarator, "="),
        ));
    }
}

/// The value that follows the `token` in `node`, such as the `5` after the `=` in `x = 5`.
/// Returns `None` when `node` has no `token`.
fn value_after<'tree>(node: Node<'tree>, token: &str) -> Option<Node<'tree>> {
    // The grammar gives the value no field name. A comment after the token is not the value.
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .skip_while(|child| child.kind() != token)
        .find(|child| child.is_named() && !child.is_extra())
}

/// The definition of `member`, a declaration with one name, in the namespace or type at
/// `parents`. It covers the whole declaration and has the initializer `value`, if any. Returns
/// `None` if the parser found no name.
fn member_definition<'tree>(
    member: Node<'tree>,
    parents: &[Node<'tree>],
    value: Option<Node<'tree>>,
) -> Option<Definition<'tree>> {
    let name = member.child_by_field_name("name")?;
    Some(Definition::new(
        parents.to_vec(),
        name,
        member.byte_range(),
        value,
    ))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let c_sharp = tree_sitter_c_sharp::LANGUAGE.into();
    let parser = language_parsers::c_style_and_doc_comments_parser(&c_sharp, "comment")?;
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

    /// The symbols of `source`, a C# file.
    fn c_sharp(source: &str) -> anyhow::Result<Symbols> {
        CodeSymbolsParser::new(&tree_sitter_c_sharp::LANGUAGE.into(), CSharp)?.parse(source)
    }

    #[test]
    fn file_with_every_shape_parse_symbols_returns_every_symbol_in_order() -> anyhow::Result<()> {
        let source = r#"using System;

/// <summary>Doc.</summary>
[Serializable]
public class Config
{
    /// <summary>Doc.</summary>
    [Obsolete]
    public const int MaxRetries = 20 * 1024;
    static readonly int Low = 1, High = (2);
    static readonly int[] Sizes = { 1, 2 };
    public static readonly string Url = $"https://{Host}/x";
    bool ready;
    int @switch;

    public int Count { get; set; } = 3;
    public static int Limit => 8;
    public int Total
    {
        get { return 1; }
    }
    public bool Ready { get; set; }

    public event EventHandler Changed;
    public event EventHandler Closed
    {
        add { }
        remove { }
    }

    public Config() { }
    static Config() { }
    ~Config() { }
    public int this[int index] => index;
    public static Config operator +(Config a, Config b) => a;
    public static implicit operator int(Config c) => 0;

    void Run() { }
    void Run(int times) { }
    int Twice(int x) => x * 2;
    T First<T>(T[] items)
    {
        int Local() => 1;
        return items[0];
    }

    public enum Mode
    {
        Red = 1,
        [Obsolete]
        Green,
    }

    public record Point(int X, int Y);

    record struct Pair(int A, int B)
    {
        public int Sum => A + B;
    }

    struct Size(int width)
    {
        int Width = width;
    }

    interface INamed
    {
        string Name();
        int Order { get; }
    }

    delegate void Handler(int value);

    class Limits
    {
        const long Id = 0x10L;
    }

    class Cache { int Size; }
    class Cache<T> { T Last; }
}
"#;
        // The class `Config` runs from its attribute to the end of the file.
        let config = source[source.find("[Serializable]").context("no class")?..].trim_end();

        // <block affects="docs/symbols.md:c-sharp-paths, src/skill.md#/blockwatch/Symbols/C#">
        assert_eq!(
            symbols(&c_sharp(source)?, source),
            vec![
                symbol("/Config", config, None),
                symbol(
                    "/Config/MaxRetries",
                    "[Obsolete]\n    public const int MaxRetries = 20 * 1024;",
                    Some("20 * 1024")
                ),
                symbol("/Config/Low", "Low = 1", Some("1")),
                symbol("/Config/High", "High = (2)", Some("2")),
                symbol(
                    "/Config/Sizes",
                    "static readonly int[] Sizes = { 1, 2 };",
                    Some("{ 1, 2 }")
                ),
                symbol(
                    "/Config/Url",
                    r#"public static readonly string Url = $"https://{Host}/x";"#,
                    Some(r#"$"https://{Host}/x""#)
                ),
                symbol("/Config/ready", "bool ready;", None),
                symbol("/Config/@switch", "int @switch;", None),
                symbol(
                    "/Config/Count",
                    "public int Count { get; set; } = 3;",
                    Some("3")
                ),
                symbol("/Config/Limit", "public static int Limit => 8;", Some("8")),
                symbol(
                    "/Config/Total",
                    "public int Total\n    {\n        get { return 1; }\n    }",
                    None
                ),
                symbol("/Config/Ready", "public bool Ready { get; set; }", None),
                symbol(
                    "/Config/Changed",
                    "public event EventHandler Changed;",
                    None
                ),
                symbol(
                    "/Config/Closed",
                    "public event EventHandler Closed\n    {\n        add { }\n        remove { }\n    }",
                    None
                ),
                symbol("/Config/Run", "void Run() { }", None),
                symbol("/Config/Run", "void Run(int times) { }", None),
                symbol("/Config/Twice", "int Twice(int x) => x * 2;", None),
                symbol(
                    "/Config/First",
                    "T First<T>(T[] items)\n    {\n        int Local() => 1;\n        return items[0];\n    }",
                    None
                ),
                symbol(
                    "/Config/Mode",
                    "public enum Mode\n    {\n        Red = 1,\n        [Obsolete]\n        Green,\n    }",
                    None
                ),
                symbol("/Config/Mode/Red", "Red = 1", Some("1")),
                symbol("/Config/Mode/Green", "[Obsolete]\n        Green", None),
                symbol("/Config/Point", "public record Point(int X, int Y);", None),
                symbol(
                    "/Config/Pair",
                    "record struct Pair(int A, int B)\n    {\n        public int Sum => A + B;\n    }",
                    None
                ),
                symbol(
                    "/Config/Pair/Sum",
                    "public int Sum => A + B;",
                    Some("A + B")
                ),
                symbol(
                    "/Config/Size",
                    "struct Size(int width)\n    {\n        int Width = width;\n    }",
                    None
                ),
                symbol("/Config/Size/Width", "int Width = width;", Some("width")),
                symbol(
                    "/Config/INamed",
                    "interface INamed\n    {\n        string Name();\n        int Order { get; }\n    }",
                    None
                ),
                symbol("/Config/INamed/Name", "string Name();", None),
                symbol("/Config/INamed/Order", "int Order { get; }", None),
                symbol("/Config/Handler", "delegate void Handler(int value);", None),
                symbol(
                    "/Config/Limits",
                    "class Limits\n    {\n        const long Id = 0x10L;\n    }",
                    None
                ),
                symbol("/Config/Limits/Id", "const long Id = 0x10L;", Some("0x10L")),
                symbol("/Config/Cache", "class Cache { int Size; }", None),
                symbol("/Config/Cache/Size", "int Size;", None),
                symbol("/Config/Cache", "class Cache<T> { T Last; }", None),
                symbol("/Config/Cache/Last", "T Last;", None),
            ]
        );
        // </block>
        Ok(())
    }

    #[test]
    fn explicit_interface_members_parse_symbols_skips_them() -> anyhow::Result<()> {
        let source = r#"class Items : IEnumerator, INotifyPropertyChanged
{
    public object Current => null;
    object IEnumerator.Current => null;
    bool IEnumerator.MoveNext() => false;
    event PropertyChangedEventHandler INotifyPropertyChanged.PropertyChanged
    {
        add { }
        remove { }
    }
}
"#;

        assert_eq!(
            symbols(&c_sharp(source)?, source)[1..],
            [symbol(
                "/Items/Current",
                "public object Current => null;",
                Some("null")
            )]
        );
        Ok(())
    }

    #[test]
    fn comment_before_a_value_parse_symbols_returns_the_value_after_it() -> anyhow::Result<()> {
        let source = r#"class Config
{
    int Size = /* bytes */ 5;
    int Limit => /* items */ 6;
}
"#;

        assert_eq!(
            values(&c_sharp(source)?, source)[1..],
            [value("/Config/Size", "5"), value("/Config/Limit", "6")]
        );
        Ok(())
    }

    #[test]
    fn namespaces_parse_symbols_returns_each_part_of_their_names_as_a_segment() -> anyhow::Result<()>
    {
        let source = r#"class Top { }

namespace Acme.Billing
{
    class Config { }
}

namespace Acme
{
    namespace Shipping
    {
        class Config { }
    }
}
"#;

        assert_eq!(
            symbols(&c_sharp(source)?, source),
            vec![
                symbol("/Top", "class Top { }", None),
                symbol("/Acme/Billing/Config", "class Config { }", None),
                symbol("/Acme/Shipping/Config", "class Config { }", None),
            ]
        );
        Ok(())
    }

    #[test]
    fn file_scoped_namespace_parse_symbols_puts_every_type_after_it_under_its_name()
    -> anyhow::Result<()> {
        let source = r#"using System;

namespace Acme.Billing;

class Config { }

#if DEBUG
class Probe { }
#endif
"#;

        assert_eq!(
            symbols(&c_sharp(source)?, source),
            vec![
                symbol("/Acme/Billing/Config", "class Config { }", None),
                symbol("/Acme/Billing/Probe", "class Probe { }", None),
            ]
        );
        Ok(())
    }

    #[test]
    fn conditional_definitions_parse_symbols_returns_every_copy() -> anyhow::Result<()> {
        // The grammar reads an enum member in `#if` only without a comma after it.
        let source = r#"namespace Acme
{
#if NET8_0
    class Platform { }
#else
    class Platform { }
#endif

    class Config
    {
#if DEBUG
        const int Level = 1;
#elif TRACE
        const int Level = 2;
#else
        const int Level = 3;
#endif
    }

    enum Mode
    {
        Red,
#if DEBUG
        Debug
#endif
    }
}
"#;
        let config = &source[source.find("class Config").context("no class")?
            ..source.find("\n\n    enum").context("no enum")?];
        let mode = source[source.find("enum Mode").context("no enum")?..].trim_end();
        let mode = mode.strip_suffix("\n}").context("no namespace end")?;

        assert_eq!(
            symbols(&c_sharp(source)?, source),
            vec![
                symbol("/Acme/Platform", "class Platform { }", None),
                symbol("/Acme/Platform", "class Platform { }", None),
                symbol("/Acme/Config", config, None),
                symbol("/Acme/Config/Level", "const int Level = 1;", Some("1")),
                symbol("/Acme/Config/Level", "const int Level = 2;", Some("2")),
                symbol("/Acme/Config/Level", "const int Level = 3;", Some("3")),
                symbol("/Acme/Mode", mode, None),
                symbol("/Acme/Mode/Red", "Red", None),
                symbol("/Acme/Mode/Debug", "Debug", None),
            ]
        );
        Ok(())
    }

    #[test]
    fn top_level_statements_parse_symbols_skips_their_definitions() -> anyhow::Result<()> {
        let source = r#"using System;

const int Limit = 1;
int Twice(int x) => x * 2;
Console.WriteLine(Twice(Limit));

class Helper { }
"#;

        assert_eq!(
            symbols(&c_sharp(source)?, source),
            vec![symbol("/Helper", "class Helper { }", None)]
        );
        Ok(())
    }

    #[test]
    fn string_kinds_parse_symbols_returns_decoded_values() -> anyhow::Result<()> {
        let source = r#"class Strings
{
    const string Escapes = "\'\"\\\0\a\b\e\f\n\r\t\v";
    const string Hex = "\x41\x7e\x41BCD";
    const string Unicode = "caf\u00e9 \uD83D\uDE00 \U0001F600";
    const string Verbatim = @"""C:\temp"" ""quoted""";
    const string Raw = """He said "hi".""";
    const string MoreQuotes = """"a """ b"""";
    const string Braces = "{{}}";
    const string Interpolated = $"a\t{{b}}\x7B\x7B";
    const string InterpolatedVerbatim = $@"C:\{{x}} ""q""";
    const string VerbatimInterpolated = @$"x";
    const string InterpolatedRaw = $$"""{x}""";
    const string Hole = $"v{Major}";
    static ReadOnlySpan<byte> Bytes => "abc"u8;
    static ReadOnlySpan<byte> VerbatimBytes => @"a\b"u8;
    static ReadOnlySpan<byte> RawBytes => """x"""u8;
    const char Char = 'a';
    const string Joined = "a" + "b";
}
"#;

        assert_eq!(
            values(&c_sharp(source)?, source)[1..],
            [
                value(
                    "/Strings/Escapes",
                    "'\"\\\0\u{7}\u{8}\u{1b}\u{c}\n\r\t\u{b}"
                ),
                // `\x` takes up to 4 hex digits, so `\x41BCD` is U+41BC and then `D`.
                value("/Strings/Hex", "A~\u{41bc}D"),
                value("/Strings/Unicode", "café 😀 😀"),
                value("/Strings/Verbatim", r#""C:\temp" "quoted""#),
                value("/Strings/Raw", r#"He said "hi"."#),
                value("/Strings/MoreQuotes", r#"a """ b"#),
                value("/Strings/Braces", "{{}}"),
                // An escape that gives a brace is not one of a pair.
                value("/Strings/Interpolated", "a\t{b}{{"),
                value("/Strings/InterpolatedVerbatim", r#"C:\{x} "q""#),
                value("/Strings/VerbatimInterpolated", "x"),
                value("/Strings/InterpolatedRaw", "{x}"),
                value("/Strings/Hole", r#"$"v{Major}""#),
                value("/Strings/Bytes", r#""abc"u8"#),
                value("/Strings/VerbatimBytes", r#"@"a\b"u8"#),
                value("/Strings/RawBytes", r#""""x"""u8"#),
                value("/Strings/Char", "'a'"),
                value("/Strings/Joined", r#""a" + "b""#),
            ]
        );
        Ok(())
    }

    #[test]
    fn raw_strings_on_several_lines_parse_symbols_removes_the_white_space_before_the_closing_quotes()
    -> anyhow::Result<()> {
        let source = r#"class Raw
{
    const string Query = """
        SELECT *
          FROM t
        """;
    const string LastLineEmpty = """
        a

        """;
    const string Interpolated = $$"""
        {x}
        """;
}
"#;

        assert_eq!(
            values(&c_sharp(source)?, source)[1..],
            [
                // The line break before the closing quotes is not part of the value.
                value("/Raw/Query", "SELECT *\n  FROM t"),
                value("/Raw/LastLineEmpty", "a\n"),
                value("/Raw/Interpolated", "{x}"),
            ]
        );
        Ok(())
    }

    #[test]
    fn white_space_only_lines_in_raw_string_parse_symbols_keeps_what_is_past_the_indentation()
    -> anyhow::Result<()> {
        // The opening quotes have spaces after them. The line after `a` holds less white space
        // than the closing quotes, and the next line more.
        let source = "class Blank\n{\n    const string Lines = \"\"\"   \n        a\n      \n            \n        b\n        \"\"\";\n}\n";

        assert_eq!(
            values(&c_sharp(source)?, source)[1..],
            [value("/Blank/Lines", "a\n\n    \nb")]
        );
        Ok(())
    }

    #[test]
    fn crlf_line_endings_parse_symbols_keeps_them() -> anyhow::Result<()> {
        let source = "class Lines\r\n{\r\n    const string Verbatim = @\"a\r\nb\";\r\n    const string Raw = \"\"\"\r\n        a\r\n\r\n        b\r\n        \"\"\";\r\n}\r\n";

        assert_eq!(
            values(&c_sharp(source)?, source)[1..],
            [
                value("/Lines/Verbatim", "a\r\nb"),
                value("/Lines/Raw", "a\r\n\r\nb"),
            ]
        );
        Ok(())
    }

    #[test]
    fn string_that_c_sharp_rejects_parse_symbols_makes_the_name_unreadable() -> anyhow::Result<()> {
        let source = r#"class Broken
{
    const string Question = "\?";
    const string Half = "\uD83D";
    const string TooHigh = "\U00110000";
    const string OpeningLine = """ a
        b
        """;
    const string Outdented = """
        a
      b
        """;
    const string ClosingLine = """
        a
        b""";
    const string NoLines = """
        """;
    const string Fine = "ok";
}

class Unknown
{
    const string Escape = "\q";
}
"#;

        let symbols = c_sharp(source)?;

        assert_eq!(values(&symbols, source)[1..], [value("/Broken/Fine", "ok")]);
        assert_eq!(
            unresolved(&symbols, source, "/Broken/Question")?,
            decode_error(3, 29, r"`\?` is not an escape in a C# string")
        );
        assert_eq!(
            unresolved(&symbols, source, "/Broken/Half")?,
            decode_error(
                4,
                25,
                "the string holds U+D83D, which is only half of a character"
            )
        );
        assert_eq!(
            unresolved(&symbols, source, "/Broken/TooHigh")?,
            decode_error(5, 28, r"`\U00110000` stands for no character")
        );
        assert_eq!(
            unresolved(&symbols, source, "/Broken/OpeningLine")?,
            decode_error(
                6,
                32,
                "a raw string on several lines has text after its opening quotes"
            )
        );
        assert_eq!(
            unresolved(&symbols, source, "/Broken/Outdented")?,
            decode_error(
                9,
                30,
                "a line of a raw string doesn't start with the white space before its closing quotes"
            )
        );
        assert_eq!(
            unresolved(&symbols, source, "/Broken/ClosingLine")?,
            decode_error(
                13,
                32,
                "a raw string on several lines has text before its closing quotes"
            )
        );
        assert_eq!(
            unresolved(&symbols, source, "/Broken/NoLines")?,
            decode_error(
                16,
                28,
                "a raw string on several lines has no line between its quotes"
            )
        );
        // The grammar rejects an escape that C# doesn't have.
        assert_eq!(
            unresolved(&symbols, source, "/Unknown/Escape")?,
            syntax_error(23, 28, "Escape")
        );
        Ok(())
    }

    #[test]
    fn deeply_nested_types_parse_symbols_does_not_overflow_stack() -> anyhow::Result<()> {
        // Each class has a path one segment longer than the class around it, so the paths grow
        // with the square of the depth. This depth is enough to overflow a recursive walk.
        let depth = 3_000;
        let source = format!("{}{}", "class C { ".repeat(depth), "}".repeat(depth));

        let symbols = c_sharp(&source)?;

        assert_eq!(symbols.symbols.len(), depth);
        Ok(())
    }

    #[test]
    fn syntax_errors_parse_symbols_makes_the_definitions_that_hold_them_unreadable()
    -> anyhow::Result<()> {
        // The parser skips the `(` after `Broken`, and the second `=` of `Z`.
        let source = r#"namespace Acme
{
    class A
    {
        const int X = 1;
        void Broken( { }
        const int Y = 2;
    }

    class B
    {
        int Z = = 3;
        int W = 4;
    }
}
"#;

        assert_eq!(
            symbols(&c_sharp(source)?, source),
            vec![
                symbol("/Acme/A/X", "const int X = 1;", Some("1")),
                symbol("/Acme/A/Y", "const int Y = 2;", Some("2")),
                symbol("/Acme/B/W", "int W = 4;", Some("4")),
            ]
        );
        assert_eq!(
            unresolved(&c_sharp(source)?, source, "/Acme/A")?,
            syntax_error(6, 20, "A")
        );
        assert_eq!(
            unresolved(&c_sharp(source)?, source, "/Acme/A/Broken")?,
            syntax_error(6, 20, "Broken")
        );
        assert_eq!(
            unresolved(&c_sharp(source)?, source, "/Acme/B")?,
            syntax_error(12, 17, "B")
        );
        assert_eq!(
            unresolved(&c_sharp(source)?, source, "/Acme/B/Z")?,
            syntax_error(12, 17, "Z")
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Position, language_parsers::Comment};

    #[test]
    fn parses_c_sharp_comments_correctly() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser()?;

        let code = r#"
// Single line comment
using System;

namespace HelloWorld
{
    /* Multi-line
     * comment example.
     */
    class Program
    {
        /// <summary>
        /// XML Doc comment.
        /// </summary>
        static void Main(string[] args)
        {
            Console.WriteLine("Hello World!"); // Another single line
            /* Simple block */
        }
    }
}
"#;
        let blocks: Vec<Comment> = comments_parser.parse(code).collect();

        assert_eq!(
            blocks,
            vec![
                Comment {
                    position_range: Position::new(2, 1)..Position::new(2, 23),
                    source_range: 1..23,
                    comment_text: "   Single line comment".to_string()
                },
                Comment {
                    position_range: Position::new(7, 5)..Position::new(9, 8),
                    source_range: 66..111,
                    comment_text: "   Multi-line\n       comment example.\n       ".to_string()
                },
                Comment {
                    position_range: Position::new(12, 9)..Position::new(12, 22),
                    source_range: 144..157,
                    comment_text: "    <summary>".to_string()
                },
                Comment {
                    position_range: Position::new(13, 9)..Position::new(13, 29),
                    source_range: 166..186,
                    comment_text: "    XML Doc comment.".to_string()
                },
                Comment {
                    position_range: Position::new(14, 9)..Position::new(14, 23),
                    source_range: 195..209,
                    comment_text: "    </summary>".to_string()
                },
                Comment {
                    position_range: Position::new(17, 48)..Position::new(17, 70),
                    source_range: 307..329,
                    comment_text: "   Another single line".to_string()
                },
                Comment {
                    position_range: Position::new(18, 13)..Position::new(18, 31),
                    source_range: 342..360,
                    comment_text: "   Simple block   ".to_string()
                }
            ]
        );

        Ok(())
    }
}
