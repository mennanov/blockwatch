use crate::language_parsers;
use crate::language_parsers::{CommentsParser, LanguageParser, LanguageParserImpl, node_text};
use crate::symbols::code::{self, CodeLanguage, CodeSymbolsParser, Definition, NestedWalk, child};
use anyhow::anyhow;
use std::iter::Peekable;
use std::ops::{Range, RangeInclusive};
use std::str::Chars;
use tree_sitter::Node;

/// Returns a [`LanguageParser`] for Dart, with its symbols.
pub(super) fn parser() -> anyhow::Result<impl LanguageParser> {
    Ok(
        LanguageParserImpl::new(comments_parser()?).with_symbols(CodeSymbolsParser::new(
            &tree_sitter_dart::LANGUAGE.into(),
            Dart,
        )?),
    )
}

/// Dart, whose files hold variables, functions and types, and whose types and named extensions
/// hold their members.
struct Dart;

impl CodeLanguage for Dart {
    /// The definitions under `root`, the root of a Dart file's syntax tree, and the members of
    /// its types and named extensions, in document order. A type comes before its members.
    fn definitions<'tree>(&self, root: Node<'tree>, _source: &str) -> Vec<Definition<'tree>> {
        code::nested_definitions(root, push_members)
    }

    /// Decodes a Dart string: its parts, such as `'a' "b"`, joined. Returns `None` for any other
    /// node, and for a string with a part that holds `$name` or `${...}`.
    ///
    /// # Errors
    /// Returns an error if an escape is not one of Dart's, or stands for no character, or if the
    /// string holds half of a character.
    fn decode_string(&self, node: &Node, source: &str) -> anyhow::Result<Option<String>> {
        if node.kind() != "string_literal" {
            return Ok(None);
        }
        code::decode_concatenated(node, source, &decode_part)
    }

    // Generated code often has names such as `_$UserFromJson`.
    const NAME_PUNCTUATION: &'static [char] = &['$'];
}

/// Decodes `part`, one string of a Dart string literal, such as the `"b"` of `'a' "b"`. Returns
/// `None` if `part` is not a string, or holds `$name` or `${...}`.
///
/// # Errors
/// Returns the errors of [`Dart::decode_string`].
fn decode_part(part: &Node, source: &str) -> anyhow::Result<Option<String>> {
    let Some(quoting) = Quoting::of(part.kind()) else {
        return Ok(None);
    };
    if child(*part, "template_substitution").is_some() {
        return Ok(None);
    }
    let content = quoting.content(node_text(part, source))?;
    if quoting.raw {
        // Dart reads every line end in a string as a line feed.
        Ok(Some(code::with_line_feeds(content)))
    } else {
        unescape(content).map(Some)
    }
}

/// How a Dart string is quoted.
struct Quoting {
    /// Whether the string is raw, such as `r'\n'`, which has no escapes.
    raw: bool,
    /// The quotes around the string, such as `'''`.
    quotes: &'static str,
}

impl Quoting {
    /// The quoting of a string part whose node kind is `kind`. Returns `None` for any other kind.
    fn of(kind: &str) -> Option<Self> {
        let (raw, quotes) = match kind {
            "string_literal_single_quotes" => (false, "'"),
            "string_literal_double_quotes" => (false, "\""),
            "string_literal_single_quotes_multiple" => (false, "'''"),
            "string_literal_double_quotes_multiple" => (false, "\"\"\""),
            "raw_string_literal_single_quotes" => (true, "'"),
            "raw_string_literal_double_quotes" => (true, "\""),
            "raw_string_literal_single_quotes_multiple" => (true, "'''"),
            "raw_string_literal_double_quotes_multiple" => (true, "\"\"\""),
            // The grammar gives a string literal no other parts.
            _ => return None,
        };
        Some(Self { raw, quotes })
    }

    /// The text of `literal`, a string with this quoting, without its `r` and its quotes, with
    /// its escapes still as written. A multi-line string loses its first line if Dart leaves it
    /// out.
    ///
    /// # Errors
    /// Returns an error if `literal` doesn't start and end with the quotes.
    fn content<'a>(&self, literal: &'a str) -> anyhow::Result<&'a str> {
        let literal = if self.raw {
            literal.strip_prefix('r').unwrap_or(literal)
        } else {
            literal
        };
        let content = code::between_quotes(literal, self.quotes)?;
        Ok(if self.quotes.len() == 3 {
            without_blank_first_line(content)
        } else {
            content
        })
    }
}

/// `content`, the text of a multi-line string between its quotes, without its first line when
/// Dart leaves that line out. Dart leaves it out, with its line end, when it holds only spaces and
/// tabs. A backslash can come before each of them, and before the line end, even in a raw string.
fn without_blank_first_line(content: &str) -> &str {
    let mut characters = content.chars();
    while let Some(mut character) = characters.next() {
        if character == '\\' {
            match characters.next() {
                Some(escaped) => character = escaped,
                None => break,
            }
        }
        match character {
            ' ' | '\t' => {}
            '\n' => return characters.as_str(),
            '\r' => {
                let rest = characters.as_str();
                return rest.strip_prefix('\n').unwrap_or(rest);
            }
            _ => break,
        }
    }
    content
}

/// Decodes the escapes in `content`, the text of a string between its quotes.
///
/// # Errors
/// Returns the errors of [`Dart::decode_string`].
fn unescape(content: &str) -> anyhow::Result<String> {
    // Two `\u` escapes can make one character together, such as the two halves of `😀`. So the
    // text is built as UTF-16 code units, as Dart builds it.
    let mut units = Vec::with_capacity(content.len());
    let mut characters = content.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\\' => push_escape(&mut characters, &mut units)?,
            // A line end becomes a line feed, unless a backslash keeps it.
            '\r' => {
                characters.next_if_eq(&'\n');
                units.push(u16::from(b'\n'));
            }
            _ => code::push_utf16(character, &mut units),
        }
    }
    code::string_from_utf16(units)
}

/// Reads one escape from `characters`, which start right after its backslash, and adds the UTF-16
/// code units it stands for to `units`.
///
/// # Errors
/// Returns an error if the escape is `\x` or `\u` without the hex digits Dart needs after it, or
/// if it stands for no character.
fn push_escape(characters: &mut Peekable<Chars>, units: &mut Vec<u16>) -> anyhow::Result<()> {
    // The grammar puts a character after every backslash in a string.
    let escaped = characters.next().unwrap_or_default();
    let character = match escaped {
        'n' => '\n',
        'r' => '\r',
        'f' => '\u{c}',
        'b' => '\u{8}',
        't' => '\t',
        'v' => '\u{b}',
        'x' => byte_escape(characters)?,
        'u' if characters.next_if_eq(&'{').is_some() => {
            return push_code_point_escape(characters, units);
        }
        'u' => {
            units.push(code_unit_escape(characters)?);
            return Ok(());
        }
        // Any other character after a backslash is that character, a line end too.
        _ => escaped,
    };
    code::push_utf16(character, units);
    Ok(())
}

/// The character of a `\x` escape, read from `characters`, which start right after its `x`. For
/// `\x41`, it is `A`.
///
/// # Errors
/// Returns an error if the next 2 characters are not hex digits.
fn byte_escape(characters: &mut Peekable<Chars>) -> anyhow::Result<char> {
    let digits: String = characters.by_ref().take(2).collect();
    hex_value(&digits, 2..=2)
        .and_then(char::from_u32)
        .ok_or_else(|| anyhow!("`\\x{digits}` is not an escape in a Dart string"))
}

/// The UTF-16 code unit of a `\u` escape without braces, read from `characters`, which start right
/// after its `u`. For `\u00e9`, it is the unit of `é`.
///
/// # Errors
/// Returns an error if the next 4 characters are not hex digits.
fn code_unit_escape(characters: &mut Peekable<Chars>) -> anyhow::Result<u16> {
    let digits: String = characters.by_ref().take(4).collect();
    code::utf16_unit(&digits)
        .ok_or_else(|| anyhow!("`\\u{digits}` is not an escape in a Dart string"))
}

/// Reads a `\u{...}` escape from `characters`, which start right after its `{`, and adds the
/// UTF-16 code units it stands for to `units`.
///
/// # Errors
/// Returns an error if the braces don't hold 1 to 6 hex digits, or if the number is above
/// `10FFFF`.
fn push_code_point_escape(
    characters: &mut Peekable<Chars>,
    units: &mut Vec<u16>,
) -> anyhow::Result<()> {
    let digits: String = characters.by_ref().take_while(|&c| c != '}').collect();
    let escape = format!("\\u{{{digits}}}");
    let code = hex_value(&digits, 1..=6)
        .ok_or_else(|| anyhow!("`{escape}` is not an escape in a Dart string"))?;
    // A Dart string is UTF-16, so `\u{D83D}` is a code unit of its own, which the next escape
    // can complete.
    match u16::try_from(code) {
        Ok(unit) => units.push(unit),
        Err(_) => {
            let character = char::from_u32(code)
                .ok_or_else(|| anyhow!("`{escape}` stands for no character"))?;
            code::push_utf16(character, units);
        }
    }
    Ok(())
}

/// The number that `digits` write in base 16. Returns `None` unless `digits` are only hex digits,
/// and as many as `lengths` allows.
fn hex_value(digits: &str, lengths: RangeInclusive<usize>) -> Option<u32> {
    // `u32::from_str_radix` also takes a sign, as in `+41`.
    if !lengths.contains(&digits.len()) || !digits.bytes().all(|digit| digit.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(digits, 16).ok()
}

/// Adds the definitions in `body`, the root of a file or the body of a type, to `walk`. `parents`
/// are the names of the type that `body` belongs to. Adds the body of each type in `body` to
/// `walk`, with its own parents.
fn push_members<'tree>(body: Node<'tree>, parents: Vec<Node<'tree>>, walk: &mut NestedWalk<'tree>) {
    let mut cursor = body.walk();
    for member in body.named_children(&mut cursor) {
        let range = member.byte_range();
        match member.kind() {
            "top_level_variable_declaration" => push_declaration(member, range, &parents, walk),
            "member_declaration" => push_member(member, &parents, walk),
            "function_declaration" | "getter_declaration" | "setter_declaration" => {
                let signature = member.child_by_field_name("signature");
                let body = member.child_by_field_name("body");
                if let Some(signature) = signature {
                    push_signature(signature, body, range, &parents, walk);
                }
            }
            "enum_entry" => {
                if let Some(name) = member.child_by_field_name("name") {
                    walk.add_definition(&parents, name, range, None);
                }
            }
            // The grammar gives the name of a type alias no field name.
            "type_alias" => {
                if let Some(name) = child(member, "type_identifier") {
                    walk.add_definition(&parents, name, range, None);
                }
            }
            "class_declaration"
            | "mixin_declaration"
            | "enum_declaration"
            | "extension_declaration" => push_type(member, &parents, walk),
            "extension_type_declaration" => push_extension_type(member, &parents, walk),
            // An external declaration has no body for a block to follow, and code never refers
            // to a library or a directive by name.
            _ => {}
        }
    }
}

/// Adds the definition of `declaration`, a class, mixin, enum or extension in the type at
/// `parents`, and its body to `walk`. An extension without a name adds nothing, as its members
/// have no path to sit under.
fn push_type<'tree>(
    declaration: Node<'tree>,
    parents: &[Node<'tree>],
    walk: &mut NestedWalk<'tree>,
) {
    // A class written as `class A = B with C;` has its name in a `mixin_application_class`.
    let name = declaration.child_by_field_name("name").or_else(|| {
        child(declaration, "mixin_application_class").and_then(|class| child(class, "identifier"))
    });
    if let Some(name) = name {
        let body = declaration.child_by_field_name("body");
        walk.add_definition_with_body(parents, name, declaration.byte_range(), None, body);
    }
}

/// Adds the definitions of `member`, a member of the type at `parents`, to `walk`.
fn push_member<'tree>(member: Node<'tree>, parents: &[Node<'tree>], walk: &mut NestedWalk<'tree>) {
    // A member keeps its annotations in the `member_declaration` around it, so its range covers
    // them.
    let range = member.byte_range();
    let mut cursor = member.walk();
    for declaration in member.named_children(&mut cursor) {
        match declaration.kind() {
            "declaration" => push_declaration(declaration, range.clone(), parents, walk),
            "method_declaration" => push_method(declaration, range.clone(), parents, walk),
            // Such as the annotations.
            _ => {}
        }
    }
}

/// Adds the definition of `method`, a method with a body in the type at `parents`, to `walk`.
/// `range` is the range of the whole method, with its annotations.
fn push_method<'tree>(
    method: Node<'tree>,
    range: Range<usize>,
    parents: &[Node<'tree>],
    walk: &mut NestedWalk<'tree>,
) {
    let Some(signatures) = method.child_by_field_name("signature") else {
        return;
    };
    let body = method.child_by_field_name("body");
    // A method's signature holds its `static` keyword and the signature of its kind.
    let mut cursor = signatures.walk();
    for signature in signatures.named_children(&mut cursor) {
        push_signature(signature, body, range.clone(), parents, walk);
    }
}

/// Adds to `walk` the definitions of `declaration`, a declaration in the type at `parents` that
/// holds variables or signatures without a body. `range` is the range of the whole declaration,
/// with its annotations.
fn push_declaration<'tree>(
    declaration: Node<'tree>,
    range: Range<usize>,
    parents: &[Node<'tree>],
    walk: &mut NestedWalk<'tree>,
) {
    // The grammar gives `external` a node of its own, but makes it a named one only in some
    // declarations.
    let mut cursor = declaration.walk();
    if declaration
        .children(&mut cursor)
        .any(|part| part.kind() == "external")
    {
        return;
    }
    for part in declaration.named_children(&mut cursor) {
        match part.kind() {
            "static_final_declaration_list" | "initialized_identifier_list" => {
                push_variables(part, range.clone(), parents, walk);
            }
            _ => push_signature(part, None, range.clone(), parents, walk),
        }
    }
}

/// Adds the definition that `signature` declares, with its `body`, if any, to `walk`. `range` is
/// the range of the whole declaration. A getter whose body is `=> v` has the value `v`.
fn push_signature<'tree>(
    signature: Node<'tree>,
    body: Option<Node<'tree>>,
    range: Range<usize>,
    parents: &[Node<'tree>],
    walk: &mut NestedWalk<'tree>,
) {
    let (name, value) = match signature.kind() {
        "function_signature" | "setter_signature" => (signature.child_by_field_name("name"), None),
        "getter_signature" => (
            signature.child_by_field_name("name"),
            body.and_then(|body| code::value_after(body, "=>")),
        ),
        "constructor_signature"
        | "constant_constructor_signature"
        | "factory_constructor_signature"
        | "redirecting_factory_constructor_signature" => (constructor_name(signature), None),
        // A path segment can't hold an operator well, and code doesn't refer to one by name.
        // The other nodes, such as a comment, declare nothing.
        _ => (None, None),
    };
    if let Some(name) = name {
        walk.add_definition(parents, name, range, value);
    }
}

/// The name of the constructor that `signature` declares, such as `draft` in `Invoice.draft()`.
/// Returns `None` for the unnamed constructor, even when it is written `Invoice.new()`.
fn constructor_name(signature: Node<'_>) -> Option<Node<'_>> {
    // The first name is the type's. The grammar gives the `.` and the `new` keyword the field
    // name too.
    let mut cursor = signature.walk();
    signature
        .children_by_field_name("name", &mut cursor)
        .filter(|name| name.is_named())
        .nth(1)
}

/// Adds a definition to `walk` for each variable in `list`, the variables of one declaration in
/// the type at `parents`. `range` is the range of the whole declaration.
fn push_variables<'tree>(
    list: Node<'tree>,
    range: Range<usize>,
    parents: &[Node<'tree>],
    walk: &mut NestedWalk<'tree>,
) {
    let mut cursor = list.walk();
    let variables: Vec<Node> = list.named_children(&mut cursor).collect();
    for (variable, range) in code::part_ranges(range, variables) {
        let Some(name) = variable.child_by_field_name("name") else {
            continue;
        };
        walk.add_definition(parents, name, range, variable.child_by_field_name("value"));
    }
}

/// Adds the definition of `declaration`, an extension type in the type at `parents`, to `walk`,
/// with the named constructor and the field that its header declares, and its body.
fn push_extension_type<'tree>(
    declaration: Node<'tree>,
    parents: &[Node<'tree>],
    walk: &mut NestedWalk<'tree>,
) {
    // In `extension type Meters.from(double value)`, the name holds the type's name and the
    // constructor's.
    let Some(header) = declaration.child_by_field_name("name") else {
        return;
    };
    let mut cursor = header.walk();
    let mut names = header.named_children(&mut cursor);
    let Some(name) = names.next() else {
        return;
    };
    let constructor = names.next();
    let body = declaration.child_by_field_name("body");
    walk.add_definition_with_body(parents, name, declaration.byte_range(), None, body);
    if let Some(representation) = declaration.child_by_field_name("representation") {
        let parents = [parents, &[name]].concat();
        if let Some(constructor) = constructor {
            let range = header.start_byte()..representation.end_byte();
            walk.add_definition(&parents, constructor, range, None);
        }
        push_representation_field(representation, &parents, walk);
    }
}

/// Adds the definition of the field that `representation`, such as `(double value)`, declares in
/// the extension type at `parents` to `walk`.
fn push_representation_field<'tree>(
    representation: Node<'tree>,
    parents: &[Node<'tree>],
    walk: &mut NestedWalk<'tree>,
) {
    let Some(field) = representation.child_by_field_name("name") else {
        return;
    };
    // The field covers what is between the parentheses, with its annotations, but not a comment
    // before them.
    let mut cursor = representation.walk();
    let start = representation
        .named_children(&mut cursor)
        .find(|part| !part.is_extra())
        .map_or(field.start_byte(), |part| part.start_byte());
    walk.add_definition(parents, field, start..field.end_byte(), None);
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let dart_language = tree_sitter_dart::LANGUAGE.into();
    // The grammar gives `///` and `/** */` comments a node kind of their own.
    let parser = language_parsers::c_style_comments_parser(
        &dart_language,
        &["comment", "documentation_comment"],
        language_parsers::SLASH_MARKERS,
    )?;
    Ok(parser)
}

#[cfg(test)]
mod symbols_tests {
    use super::*;
    use crate::symbols::code::test_utils::{
        decode_error, symbol, symbols, syntax_error, text_of, unresolved, value, values,
    };
    use crate::symbols::{Symbols, SymbolsParser};

    /// The symbols of `source`, a Dart file.
    fn dart(source: &str) -> anyhow::Result<Symbols> {
        CodeSymbolsParser::new(&tree_sitter_dart::LANGUAGE.into(), Dart)?.parse(source)
    }

    #[test]
    fn file_with_every_shape_parse_symbols_returns_every_symbol_in_order() -> anyhow::Result<()> {
        let source = r#"library billing;

import 'dart:math';

/// Doc.
@immutable
const kApiVersion = '2.1.0';
final String userAgent = 'billing/$kApiVersion';
int retries = 3, timeout = (30);
late final int lazy;
String get greeting => 'hi';
int get counted {
  return 1;
}
set level(int value) {}
void main() {}
typedef Handler = void Function(int);
typedef void Callback(int code);

/// Doc.
@immutable
class Invoice extends Base with Printable {
  /// Doc.
  @Deprecated('x')
  static const int maxLines = 20 * 1024;
  final int id;

  Invoice(this.id);
  Invoice.draft() : id = 0;
  factory Invoice.fromJson(Map<String, Object?> json) => Invoice(json['id'] as int);

  int get total => _total;
  void send() {}
  static int count() => 0;
}

abstract class Repository {
  void save();
  int get size;
}

mixin Printable on Base {
  void printIt() {}
}

class Alias = Base with Printable;

enum Mode { light, dark('d'); const Mode([this.code]); final String? code; }

extension StringX on String { bool get isBlank => trim().isEmpty; }

extension type Id(int value) {
  int get doubled => value * 2;
}
"#;
        let invoice = text_of(source, "@immutable\nclass", "\n}")?;

        // <block affects="docs/symbols.md:dart-paths, src/skill.md#/blockwatch/Symbols/Dart">
        assert_eq!(
            symbols(&dart(source)?, source),
            vec![
                symbol(
                    "/kApiVersion",
                    "@immutable\nconst kApiVersion = '2.1.0';",
                    Some("2.1.0")
                ),
                symbol(
                    "/userAgent",
                    "final String userAgent = 'billing/$kApiVersion';",
                    Some("'billing/$kApiVersion'")
                ),
                symbol("/retries", "retries = 3", Some("3")),
                symbol("/timeout", "timeout = (30)", Some("30")),
                symbol("/lazy", "late final int lazy;", None),
                symbol("/greeting", "String get greeting => 'hi';", Some("hi")),
                symbol("/counted", "int get counted {\n  return 1;\n}", None),
                symbol("/level", "set level(int value) {}", None),
                symbol("/main", "void main() {}", None),
                symbol("/Handler", "typedef Handler = void Function(int);", None),
                symbol("/Callback", "typedef void Callback(int code);", None),
                symbol("/Invoice", invoice, None),
                symbol(
                    "/Invoice/maxLines",
                    "@Deprecated('x')\n  static const int maxLines = 20 * 1024;",
                    Some("20 * 1024")
                ),
                symbol("/Invoice/id", "final int id;", None),
                symbol("/Invoice/draft", "Invoice.draft() : id = 0;", None),
                symbol(
                    "/Invoice/fromJson",
                    "factory Invoice.fromJson(Map<String, Object?> json) => Invoice(json['id'] as int);",
                    None
                ),
                symbol("/Invoice/total", "int get total => _total;", Some("_total")),
                symbol("/Invoice/send", "void send() {}", None),
                symbol("/Invoice/count", "static int count() => 0;", None),
                symbol(
                    "/Repository",
                    "abstract class Repository {\n  void save();\n  int get size;\n}",
                    None
                ),
                symbol("/Repository/save", "void save();", None),
                symbol("/Repository/size", "int get size;", None),
                symbol(
                    "/Printable",
                    "mixin Printable on Base {\n  void printIt() {}\n}",
                    None
                ),
                symbol("/Printable/printIt", "void printIt() {}", None),
                symbol("/Alias", "class Alias = Base with Printable;", None),
                symbol(
                    "/Mode",
                    "enum Mode { light, dark('d'); const Mode([this.code]); final String? code; }",
                    None
                ),
                symbol("/Mode/light", "light", None),
                symbol("/Mode/dark", "dark('d')", None),
                symbol("/Mode/code", "final String? code;", None),
                symbol(
                    "/StringX",
                    "extension StringX on String { bool get isBlank => trim().isEmpty; }",
                    None
                ),
                symbol(
                    "/StringX/isBlank",
                    "bool get isBlank => trim().isEmpty;",
                    Some("trim().isEmpty")
                ),
                symbol(
                    "/Id",
                    "extension type Id(int value) {\n  int get doubled => value * 2;\n}",
                    None
                ),
                symbol("/Id/value", "int value", None),
                symbol(
                    "/Id/doubled",
                    "int get doubled => value * 2;",
                    Some("value * 2")
                ),
            ]
        );
        // </block>
        Ok(())
    }

    #[test]
    fn getter_and_setter_with_one_name_parse_symbols_returns_both() -> anyhow::Result<()> {
        // Both have the path `/Box/size`, so a reference to it is ambiguous.
        let source = r#"int get count => _count;
set count(int value) => _count = value;

class Box {
  int get size => _size;
  set size(int value) {
    _size = value;
  }
}
"#;
        let set_size = text_of(source, "set size", "\n  }")?;

        assert_eq!(
            symbols(&dart(source)?, source),
            vec![
                symbol("/count", "int get count => _count;", Some("_count")),
                symbol("/count", "set count(int value) => _count = value;", None),
                symbol("/Box", text_of(source, "class", "\n}")?, None),
                symbol("/Box/size", "int get size => _size;", Some("_size")),
                symbol("/Box/size", set_size, None),
            ]
        );
        Ok(())
    }

    #[test]
    fn constructors_parse_symbols_returns_only_the_named_ones() -> anyhow::Result<()> {
        // `Point.new` is the unnamed constructor, written with its tear-off name.
        let source = r#"class Point {
  final int x;
  Point(this.x);
  Point.origin() : this(0);
  const Point.zero() : x = 0;
  Point._private() : x = 1;
  factory Point.parse(String text) => Point(int.parse(text));
  factory Point.copy(Point other) = Point.clone;
  Point.clone(Point other) : x = other.x;
}

class Pair {
  Pair.new();
  factory Pair.of() => Pair();
}

class Single {
  factory Single() => Single._();
  Single._();
}
"#;

        let paths: Vec<String> = symbols(&dart(source)?, source)
            .into_iter()
            .map(|(path, _, _)| path)
            .collect();
        assert_eq!(
            paths,
            [
                "/Point",
                "/Point/x",
                "/Point/origin",
                "/Point/zero",
                "/Point/_private",
                "/Point/parse",
                "/Point/copy",
                "/Point/clone",
                "/Pair",
                "/Pair/of",
                "/Single",
                "/Single/_",
            ]
        );
        Ok(())
    }

    #[test]
    fn named_constructor_and_field_with_one_name_parse_symbols_returns_both() -> anyhow::Result<()>
    {
        // Both have the path `/Button/disabled`, so a reference to it is ambiguous.
        let source = r#"class Button {
  final bool disabled;
  const Button.disabled() : disabled = true;
}
"#;

        assert_eq!(
            symbols(&dart(source)?, source)[1..],
            [
                symbol("/Button/disabled", "final bool disabled;", None),
                symbol(
                    "/Button/disabled",
                    "const Button.disabled() : disabled = true;",
                    None
                ),
            ]
        );
        Ok(())
    }

    #[test]
    fn named_and_unnamed_extensions_parse_symbols_returns_only_the_named_members()
    -> anyhow::Result<()> {
        // Both extensions extend `String`, so their members don't go under it.
        let source = r#"extension StringX on String {
  static const empty = '';
  bool get isBlank => trim().isEmpty;
  String twice() => this + this;
}

extension on String {
  bool get isShort => length < 3;
}

extension NumX on num { int get rounded => round(); }
"#;

        let paths: Vec<String> = symbols(&dart(source)?, source)
            .into_iter()
            .map(|(path, _, _)| path)
            .collect();
        assert_eq!(
            paths,
            [
                "/StringX",
                "/StringX/empty",
                "/StringX/isBlank",
                "/StringX/twice",
                "/NumX",
                "/NumX/rounded",
            ]
        );
        Ok(())
    }

    #[test]
    fn extension_types_parse_symbols_returns_the_representation_and_the_members()
    -> anyhow::Result<()> {
        // `Meters.from` declares a named constructor, as `Point.from(...)` in a class does.
        let source = r#"extension type const UserId(@protected int raw) implements int {
  static const zero = UserId(0);
  UserId.parse(String text) : raw = int.parse(text);
  bool get isValid => raw > 0;
}

extension type Meters.from(/* in meters */ double value) {}
"#;

        assert_eq!(
            symbols(&dart(source)?, source),
            vec![
                symbol("/UserId", text_of(source, "extension", "\n}")?, None),
                symbol("/UserId/raw", "@protected int raw", None),
                symbol(
                    "/UserId/zero",
                    "static const zero = UserId(0);",
                    Some("UserId(0)")
                ),
                symbol(
                    "/UserId/parse",
                    "UserId.parse(String text) : raw = int.parse(text);",
                    None
                ),
                symbol(
                    "/UserId/isValid",
                    "bool get isValid => raw > 0;",
                    Some("raw > 0")
                ),
                symbol(
                    "/Meters",
                    "extension type Meters.from(/* in meters */ double value) {}",
                    None
                ),
                symbol(
                    "/Meters/from",
                    "Meters.from(/* in meters */ double value)",
                    None
                ),
                symbol("/Meters/value", "double value", None),
            ]
        );
        Ok(())
    }

    #[test]
    fn enum_with_members_parse_symbols_returns_its_values_and_members() -> anyhow::Result<()> {
        let source = r#"enum Planet implements Comparable<Planet> {
  mercury(mass: 3.3),
  @Deprecated('x')
  pluto(mass: 0.01);

  const Planet({required this.mass});
  const Planet.light() : mass = 0;

  static const count = 2;
  final double mass;
  bool get isHeavy => mass > 1;
  int compareTo(Planet other) => 0;
}
"#;

        assert_eq!(
            symbols(&dart(source)?, source)[1..],
            [
                symbol("/Planet/mercury", "mercury(mass: 3.3)", None),
                symbol(
                    "/Planet/pluto",
                    "@Deprecated('x')\n  pluto(mass: 0.01)",
                    None
                ),
                symbol("/Planet/light", "const Planet.light() : mass = 0;", None),
                symbol("/Planet/count", "static const count = 2;", Some("2")),
                symbol("/Planet/mass", "final double mass;", None),
                symbol(
                    "/Planet/isHeavy",
                    "bool get isHeavy => mass > 1;",
                    Some("mass > 1")
                ),
                symbol(
                    "/Planet/compareTo",
                    "int compareTo(Planet other) => 0;",
                    None
                ),
            ]
        );
        Ok(())
    }

    #[test]
    fn external_declarations_parse_symbols_returns_none_of_them() -> anyhow::Result<()> {
        let source = r#"external int nativeAdd(int a, int b);
external int counter;
external String get platform;
external set platform(String value);
const kept = 1;

class Interop {
  external Interop();
  external Interop.named();
  external factory Interop.create();
  external static int count;
  external int get size;
  external void run();
  void kept() {}
}
"#;

        let paths: Vec<String> = symbols(&dart(source)?, source)
            .into_iter()
            .map(|(path, _, _)| path)
            .collect();
        assert_eq!(paths, ["/kept", "/Interop", "/Interop/kept"]);
        Ok(())
    }

    #[test]
    fn operators_parse_symbols_returns_none_of_them() -> anyhow::Result<()> {
        let source = r#"class Vector {
  bool operator ==(Object other) => true;
  Vector operator -() => this;
  int operator [](int index) => 0;
  void operator []=(int index, int value) {}
  int length() => 0;
}
"#;

        let paths: Vec<String> = symbols(&dart(source)?, source)
            .into_iter()
            .map(|(path, _, _)| path)
            .collect();
        assert_eq!(paths, ["/Vector", "/Vector/length"]);
        Ok(())
    }

    #[test]
    fn string_kinds_parse_symbols_returns_decoded_values() -> anyhow::Result<()> {
        let source = r#"class Strings {
  static const single = 'it\'s';
  static const quoted = "say \"hi\"";
  static const escapes = '\n\r\f\b\t\v\x41\$\q\\';
  static const unicode = 'caf\u00e9 \u{1F600} \u{41}\u{9}';
  static const pair = '\uD83D\uDE00';
  static const bracedPair = '\u{D83D}\u{DE00}';
  static const raw = r'C:\path\n$name';
  static const rawDouble = r"it's";
  static const adjacent = 'a' "b" r'\c';
  static const interpolated = 'hi $name';
  static const braced = 'sum ${1 + 2}';
  static const partly = 'a' 'b$name';
}
"#;

        assert_eq!(
            values(&dart(source)?, source)[1..],
            [
                value("/Strings/single", "it's"),
                value("/Strings/quoted", r#"say "hi""#),
                // Any other character after a backslash is that character.
                value("/Strings/escapes", "\n\r\u{c}\u{8}\t\u{b}A$q\\"),
                value("/Strings/unicode", "café 😀 A\t"),
                // A Dart string is UTF-16, so two escapes can make one character.
                value("/Strings/pair", "😀"),
                value("/Strings/bracedPair", "😀"),
                value("/Strings/raw", r"C:\path\n$name"),
                value("/Strings/rawDouble", "it's"),
                value("/Strings/adjacent", r"ab\c"),
                value("/Strings/interpolated", "'hi $name'"),
                value("/Strings/braced", "'sum ${1 + 2}'"),
                value("/Strings/partly", "'a' 'b$name'"),
            ]
        );
        Ok(())
    }

    #[test]
    fn multi_line_strings_parse_symbols_drops_only_a_blank_first_line() -> anyhow::Result<()> {
        let source = r#"class Lines {
  static const plain = '''
a
  b''';
  static const escapedTab = """  \t
c""";
  static const text = '''x
y''';
  static const spacesThenText = '''  x
y''';
  static const escapedBlank = '''\ \
z''';
  static const raw = r'''
\n''';
  static const quotes = '''it's "fine"''';
  static const rawQuotes = r"""it's "raw" \n""";
  static const rawBackslash = r'''\''';
  static const joined = '''one\
two''';
}
"#;

        assert_eq!(
            values(&dart(source)?, source)[1..],
            [
                value("/Lines/plain", "a\n  b"),
                // An escape is not white space, even `\t`.
                value("/Lines/escapedTab", "  \t\nc"),
                value("/Lines/text", "x\ny"),
                value("/Lines/spacesThenText", "  x\ny"),
                // A space in the blank first line can have a backslash before it, and so can
                // its line end.
                value("/Lines/escapedBlank", "z"),
                value("/Lines/raw", r"\n"),
                value("/Lines/quotes", r#"it's "fine""#),
                value("/Lines/rawQuotes", r#"it's "raw" \n"#),
                value("/Lines/rawBackslash", r"\"),
                // A backslash before a line end gives the line end.
                value("/Lines/joined", "one\ntwo"),
            ]
        );
        Ok(())
    }

    #[test]
    fn cr_and_crlf_line_endings_parse_symbols_reads_line_feeds() -> anyhow::Result<()> {
        // A blank first line can end with CR LF or CR too, and hold tabs.
        let source = "const crlf = '''\r\na\r\nb''';
const cr = '''\ra\rb''';
const raw = r''' \t\r\na\r\nb''';
const escaped = '''a\\\r\nb''';
";

        assert_eq!(
            values(&dart(source)?, source),
            [
                value("/crlf", "a\nb"),
                value("/cr", "a\nb"),
                value("/raw", "a\nb"),
                // A backslash keeps the CR, and the LF after it is a line end of its own.
                value("/escaped", "a\r\nb"),
            ]
        );
        Ok(())
    }

    #[test]
    fn escape_that_is_not_text_parse_symbols_makes_the_name_unreadable() -> anyhow::Result<()> {
        let source = r#"class Broken {
  static const tooLong = '\u{1234567}';
  static const tooHigh = '\u{110000}';
  static const half = '\uD800';
  static const bracedHalf = 'a' '\u{DC00}';
  static const fine = 'ok';
}
"#;

        assert_eq!(
            values(&dart(source)?, source)[1..],
            [value("/Broken/fine", "ok")]
        );
        assert_eq!(
            unresolved(&dart(source)?, source, "/Broken/tooLong")?,
            decode_error(2, 26, r"`\u{1234567}` is not an escape in a Dart string")
        );
        assert_eq!(
            unresolved(&dart(source)?, source, "/Broken/tooHigh")?,
            decode_error(3, 26, r"`\u{110000}` stands for no character")
        );
        assert_eq!(
            unresolved(&dart(source)?, source, "/Broken/half")?,
            decode_error(
                4,
                23,
                "the string holds U+D800, which is only half of a character"
            )
        );
        assert_eq!(
            unresolved(&dart(source)?, source, "/Broken/bracedHalf")?,
            decode_error(
                5,
                29,
                "the string holds U+DC00, which is only half of a character"
            )
        );
        Ok(())
    }

    #[test]
    fn syntax_error_parse_symbols_makes_the_definitions_that_hold_it_unreadable()
    -> anyhow::Result<()> {
        // The parser skips `call(1 2)`, and the `+` of `z`.
        let source = r#"class A {
  static const x = 1;
  void broken() { call(1 2); }
  static const y = 2;
}

class B {
  int z = 3 +;
  int w = 4;
}

const after = 5;
"#;

        assert_eq!(
            symbols(&dart(source)?, source),
            vec![
                symbol("/A/x", "static const x = 1;", Some("1")),
                symbol("/A/y", "static const y = 2;", Some("2")),
                symbol("/B/w", "int w = 4;", Some("4")),
                symbol("/after", "const after = 5;", Some("5")),
            ]
        );
        assert_eq!(
            unresolved(&dart(source)?, source, "/A")?,
            syntax_error(3, 19, "A")
        );
        assert_eq!(
            unresolved(&dart(source)?, source, "/A/broken")?,
            syntax_error(3, 19, "broken")
        );
        assert_eq!(
            unresolved(&dart(source)?, source, "/B")?,
            syntax_error(8, 13, "B")
        );
        assert_eq!(
            unresolved(&dart(source)?, source, "/B/z")?,
            syntax_error(8, 13, "z")
        );
        Ok(())
    }

    #[test]
    fn name_with_a_dollar_in_a_skipped_span_parse_symbols_makes_it_unreadable() -> anyhow::Result<()>
    {
        // Dart allows `$` in a name. The parser skips `call(_$count 2)`.
        let source = r#"void broken() { call(_$count 2); }

const _$count = 1;
"#;

        assert_eq!(
            unresolved(&dart(source)?, source, "/_$count")?,
            syntax_error(1, 17, "_$count")
        );
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

        let comments: Vec<Comment> = comments_parser
            .parse(
                r#"
// This is a line comment
/// Doc comment
int add(int a, int b) {
  return a + b; // inline comment
}

/* This is a block comment
spanning multiple lines
*/
/** Documentation block */
//! Rust and Doxygen read this marker as a doc comment.
//// A banner line repeats the marker.
"#,
            )
            .collect();

        assert_eq!(
            comments,
            vec![
                Comment {
                    position_range: Position::new(2, 1)..Position::new(2, 26),
                    source_range: 1..26,
                    comment_text: "   This is a line comment".to_string()
                },
                Comment {
                    position_range: Position::new(3, 1)..Position::new(3, 16),
                    source_range: 27..42,
                    comment_text: "    Doc comment".to_string()
                },
                Comment {
                    position_range: Position::new(5, 17)..Position::new(5, 34),
                    source_range: 83..100,
                    comment_text: "   inline comment".to_string()
                },
                Comment {
                    position_range: Position::new(8, 1)..Position::new(10, 3),
                    source_range: 104..157,
                    comment_text: "   This is a block comment\nspanning multiple lines\n  "
                        .to_string()
                },
                Comment {
                    position_range: Position::new(11, 1)..Position::new(11, 27),
                    source_range: 158..184,
                    comment_text: "    Documentation block   ".to_string()
                },
                Comment {
                    position_range: Position::new(12, 1)..Position::new(12, 56),
                    source_range: 185..240,
                    comment_text: "    Rust and Doxygen read this marker as a doc comment."
                        .to_string()
                },
                Comment {
                    position_range: Position::new(13, 1)..Position::new(13, 39),
                    source_range: 241..279,
                    comment_text: "     A banner line repeats the marker.".to_string()
                },
            ]
        );

        Ok(())
    }
}
