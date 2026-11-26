use crate::languages;
use crate::languages::code_symbols::{
    self, CodeLanguage, CodeSymbolsParser, Definition, NestedWalk,
};
use crate::languages::{CommentsParser, LanguageParser, LanguageParserImpl, node_text};
use anyhow::{anyhow, bail};
use std::borrow::Cow;
use tree_sitter::Node;

/// Returns a [`LanguageParser`] for TypeScript, with its symbols.
pub(super) fn parser() -> impl LanguageParser {
    LanguageParserImpl::new(comments_parser()).with_symbols(CodeSymbolsParser::new(
        &tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        EcmaScript,
    ))
}

fn comments_parser() -> impl CommentsParser {
    languages::ecmascript_comments_parser(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())
}

/// TypeScript or JavaScript, whose files hold variables, functions, types, namespaces and object
/// literals, and whose types, namespaces and object literals hold their members.
pub(super) struct EcmaScript;

impl CodeLanguage for EcmaScript {
    /// The top-level definitions under `root`, the root of a TypeScript or JavaScript file's syntax
    /// tree, with object properties and class, enum, interface and namespace members under them,
    /// in document order. `source` is the text of the file.
    fn definitions<'tree>(&self, root: Node<'tree>, source: &str) -> Vec<Definition<'tree>> {
        code_symbols::nested_definitions(root, |body, parents, walk| {
            self.push_members(body, parents, source, walk);
        })
    }

    /// Decodes a string, or a template without a `${...}` field. Returns `None` for any other node.
    ///
    /// # Errors
    /// Returns an error if an escape is a legacy octal escape, or the string is not text, such as
    /// half of a character.
    fn decode_string(&self, node: &Node, source: &str) -> anyhow::Result<Option<String>> {
        if !matches!(node.kind(), "string" | "template_string") {
            return Ok(None);
        }
        // A JavaScript string is UTF-16, so it is built from code units: an escape can give half of
        // a character, which the next escape completes.
        let mut units = Vec::new();
        let mut cursor = node.walk();
        for part in node.named_children(&mut cursor) {
            let written = node_text(&part, source);
            match part.kind() {
                "escape_sequence" => push_escape(written, &mut units)?,
                // A template with a field has no fixed text.
                "template_substitution" => return Ok(None),
                // A template reads a CRLF or CR line end as a line feed. A string can't hold a line
                // end.
                _ => units.extend(
                    written
                        .replace("\r\n", "\n")
                        .replace('\r', "\n")
                        .encode_utf16(),
                ),
            }
        }
        code_symbols::string_from_utf16(units).map(Some)
    }

    /// Decodes a string property key into its path segment, and keeps identifiers and numbers as
    /// written.
    ///
    /// # Panics
    /// Panics if `name` is a string that [`Self::decode_string`] cannot decode.
    fn name_segment<'s>(&self, name: &Node, source: &'s str) -> Cow<'s, str> {
        match self.decode_string(name, source) {
            Ok(Some(text)) => Cow::Owned(text),
            Ok(None) => Cow::Borrowed(node_text(name, source)),
            // `property_key` keeps only string keys that `decode_string` decoded.
            Err(error) => unreachable!("undecodable string key reached name_segment: {error:#}"),
        }
    }

    // Property keys can be single- or double-quoted, so skipped syntax errors check both.
    const NAME_QUOTES: &'static [char] = &['"', '\''];

    // Identifiers can hold `$`, and private class members start with `#`.
    const NAME_PUNCTUATION: &'static [char] = &['$', '#'];
}

impl EcmaScript {
    /// Adds the definitions in `body` to `walk`: a program, a namespace body, an object literal, or
    /// a class, enum or interface body. `parents` are the names of the definitions that `body` sits
    /// in. Adds each nested body in `body` to `walk`, with its own parents.
    fn push_members<'tree>(
        &self,
        body: Node<'tree>,
        parents: &[Node<'tree>],
        source: &str,
        walk: &mut NestedWalk<'tree>,
    ) {
        match body.kind() {
            "program" | "statement_block" => {
                let mut cursor = body.walk();
                for statement in body.named_children(&mut cursor) {
                    push_statement(statement, parents, source, walk);
                }
            }
            "object" => self.push_object_members(body, parents, source, walk),
            "class_body" => self.push_class_members(body, parents, source, walk),
            "enum_body" => self.push_enum_members(body, parents, source, walk),
            "interface_body" => self.push_interface_members(body, parents, source, walk),
            _ => {}
        }
    }

    /// Adds the properties and methods in `object`, an object literal under `parents`, to `walk`.
    fn push_object_members<'tree>(
        &self,
        object: Node<'tree>,
        parents: &[Node<'tree>],
        source: &str,
        walk: &mut NestedWalk<'tree>,
    ) {
        let mut cursor = object.walk();
        for member in object
            .named_children(&mut cursor)
            .filter(|child| !child.is_extra())
        {
            match member.kind() {
                "pair" => self.push_value_member(member, "key", parents, source, walk),
                "shorthand_property_identifier" => {
                    walk.add_definition(parents, member, member.byte_range(), Some(member));
                }
                "method_definition" => {
                    if let Some(name) = self.field_key(member, "name", source) {
                        walk.add_definition(parents, name, member.byte_range(), None);
                    }
                }
                _ => {}
            }
        }
    }

    /// Adds the fields, methods and constructor parameter properties in `body`, a `class_body`
    /// under `parents`, to `walk`.
    fn push_class_members<'tree>(
        &self,
        body: Node<'tree>,
        parents: &[Node<'tree>],
        source: &str,
        walk: &mut NestedWalk<'tree>,
    ) {
        // TypeScript places method decorators as siblings before `method_definition` in
        // `class_body`, while JavaScript nests them inside `method_definition`.
        let mut decorator_start: Option<usize> = None;
        let mut cursor = body.walk();
        for member in body
            .named_children(&mut cursor)
            .filter(|child| !child.is_extra())
        {
            if member.kind() == "decorator" {
                decorator_start.get_or_insert_with(|| member.start_byte());
                continue;
            }
            let prefix_start = decorator_start.take();
            match member.kind() {
                "method_definition" => {
                    self.push_class_method(member, prefix_start, parents, source, walk);
                }
                "public_field_definition" => {
                    self.push_value_member(member, "name", parents, source, walk);
                }
                "field_definition" => {
                    self.push_value_member(member, "property", parents, source, walk);
                }
                "abstract_method_signature" => {
                    if let Some(name) = self.field_key(member, "name", source) {
                        walk.add_definition(parents, name, member.byte_range(), None);
                    }
                }
                _ => {}
            }
        }
    }

    /// Adds `method`, a `method_definition` under `parents`, to `walk`, or its parameter
    /// properties when it is a constructor. `decorator_start` is the start byte of any sibling
    /// TypeScript decorators before it.
    fn push_class_method<'tree>(
        &self,
        method: Node<'tree>,
        decorator_start: Option<usize>,
        parents: &[Node<'tree>],
        source: &str,
        walk: &mut NestedWalk<'tree>,
    ) {
        let Some(name) = self.field_key(method, "name", source) else {
            return;
        };
        if self.is_constructor(method, name, source) {
            push_parameter_properties(method, parents, source, walk);
            return;
        }
        let start = decorator_start.unwrap_or_else(|| method.start_byte());
        walk.add_definition(parents, name, start..method.end_byte(), None);
    }

    /// Adds `member`, an object pair or class field whose name sits in `name_field` and whose
    /// initializer sits in `value`, to `walk` under `parents`, along with any nested object or
    /// class initializer.
    fn push_value_member<'tree>(
        &self,
        member: Node<'tree>,
        name_field: &str,
        parents: &[Node<'tree>],
        source: &str,
        walk: &mut NestedWalk<'tree>,
    ) {
        let Some(name) = self.field_key(member, name_field, source) else {
            return;
        };
        let value = member.child_by_field_name("value").map(without_type_syntax);
        let body = initializer_body(value);
        walk.add_definition_with_body(parents, name, member.byte_range(), value, body);
    }

    /// Adds the members in `body`, an `enum_body` under `parents`, to `walk`.
    fn push_enum_members<'tree>(
        &self,
        body: Node<'tree>,
        parents: &[Node<'tree>],
        source: &str,
        walk: &mut NestedWalk<'tree>,
    ) {
        let mut cursor = body.walk();
        for member in body
            .named_children(&mut cursor)
            .filter(|child| !child.is_extra())
        {
            if member.kind() == "enum_assignment" {
                let Some(name) = self.field_key(member, "name", source) else {
                    continue;
                };
                let value = member.child_by_field_name("value").map(without_type_syntax);
                walk.add_definition(parents, name, member.byte_range(), value);
            } else if let Some(name) = self.property_key(member, source) {
                walk.add_definition(parents, name, member.byte_range(), None);
            }
        }
    }

    /// Adds the property and method signatures in `body`, an `interface_body` under `parents`, to
    /// `walk`.
    fn push_interface_members<'tree>(
        &self,
        body: Node<'tree>,
        parents: &[Node<'tree>],
        source: &str,
        walk: &mut NestedWalk<'tree>,
    ) {
        let mut cursor = body.walk();
        for member in body
            .named_children(&mut cursor)
            .filter(|child| !child.is_extra())
        {
            if matches!(member.kind(), "property_signature" | "method_signature")
                && let Some(name) = self.field_key(member, "name", source)
            {
                walk.add_definition(parents, name, member.byte_range(), None);
            }
        }
    }

    /// The [`Self::property_key`] in `node`'s `field`, if present and indexable.
    fn field_key<'tree>(
        &self,
        node: Node<'tree>,
        field: &str,
        source: &str,
    ) -> Option<Node<'tree>> {
        node.child_by_field_name(field)
            .and_then(|key| self.property_key(key, source))
    }

    /// `key` when it is a property, class member or enum member name that blockwatch indexes, or
    /// `None` for computed names, non-decimal numeric literals and undecodable string keys.
    fn property_key<'tree>(&self, key: Node<'tree>, source: &str) -> Option<Node<'tree>> {
        match key.kind() {
            "property_identifier" | "private_property_identifier" => Some(key),
            "number" => is_decimal_integer_key(node_text(&key, source)).then_some(key),
            "string" => matches!(self.decode_string(&key, source), Ok(Some(_))).then_some(key),
            _ => None,
        }
    }

    /// Whether `method`, whose validated key is `name`, is a class constructor rather than a
    /// `static`, `get` or `set` member named `constructor`.
    fn is_constructor(&self, method: Node<'_>, name: Node<'_>, source: &str) -> bool {
        let mut cursor = method.walk();
        // tree-sitter-javascript reads `static get` followed by a line break as one `"static get"`
        // token.
        if method
            .children(&mut cursor)
            .any(|child| matches!(child.kind(), "static" | "get" | "set" | "static get"))
        {
            return false;
        }
        self.name_segment(&name, source) == "constructor"
    }
}

/// Adds the definitions in `statement` under `parents` to `walk`.
fn push_statement<'tree>(
    statement: Node<'tree>,
    parents: &[Node<'tree>],
    source: &str,
    walk: &mut NestedWalk<'tree>,
) {
    let Some(declaration) = declaration(statement) else {
        return;
    };
    match declaration.kind() {
        "lexical_declaration" | "variable_declaration" => {
            push_variables(declaration, statement, parents, source, walk);
        }
        "class_declaration"
        | "abstract_class_declaration"
        | "enum_declaration"
        | "interface_declaration" => {
            if let Some(name) = declaration
                .child_by_field_name("name")
                .filter(|name| is_declaration_name(name, source))
            {
                let body = declaration.child_by_field_name("body");
                walk.add_definition_with_body(parents, name, statement.byte_range(), None, body);
            }
        }
        "function_declaration" | "generator_function_declaration" | "type_alias_declaration" => {
            if let Some(name) = declaration
                .child_by_field_name("name")
                .filter(|name| is_declaration_name(name, source))
            {
                walk.add_definition(parents, name, statement.byte_range(), None);
            }
        }
        "internal_module" | "module" => {
            let Some(name) = declaration.child_by_field_name("name") else {
                return;
            };
            let segments = namespace_segments(name, source);
            if let Some((last, prefix)) = segments.split_last() {
                let parents = [parents, prefix].concat();
                let body = declaration.child_by_field_name("body");
                walk.add_definition_with_body(&parents, *last, statement.byte_range(), None, body);
            }
        }
        _ => {}
    }
}

/// Adds the definitions of the variables that `declaration`, a `const`, `let` or `var` declaration
/// that `statement` holds, declares under `parents`, to `walk`, in document order.
fn push_variables<'tree>(
    declaration: Node<'tree>,
    statement: Node<'tree>,
    parents: &[Node<'tree>],
    source: &str,
    walk: &mut NestedWalk<'tree>,
) {
    let mut cursor = declaration.walk();
    let declarators: Vec<Node> = declaration
        .named_children(&mut cursor)
        .filter(|child| child.kind() == "variable_declarator")
        .collect();
    for (declarator, range) in code_symbols::part_ranges(statement.byte_range(), declarators) {
        let Some(name) = declarator
            .child_by_field_name("name")
            .filter(|name| is_declaration_name(name, source))
        else {
            continue;
        };
        let value = declarator
            .child_by_field_name("value")
            .map(without_type_syntax);
        let body = initializer_body(value);
        walk.add_definition_with_body(parents, name, range, value, body);
    }
}

/// Adds TypeScript constructor parameter properties (those with `public`, `private`, `protected`,
/// `readonly` or `override`) in `constructor` to `walk` under `parents`.
fn push_parameter_properties<'tree>(
    constructor: Node<'tree>,
    parents: &[Node<'tree>],
    source: &str,
    walk: &mut NestedWalk<'tree>,
) {
    let Some(parameters) = constructor.child_by_field_name("parameters") else {
        return;
    };
    let mut cursor = parameters.walk();
    for parameter in parameters.named_children(&mut cursor) {
        if !matches!(
            parameter.kind(),
            "required_parameter" | "optional_parameter"
        ) {
            continue;
        }
        let mut param_cursor = parameter.walk();
        let is_property = parameter.children(&mut param_cursor).any(|child| {
            matches!(
                child.kind(),
                "accessibility_modifier" | "readonly" | "override_modifier"
            )
        });
        if !is_property {
            continue;
        }
        let Some(name) = parameter
            .child_by_field_name("pattern")
            .filter(|pattern| is_declaration_name(pattern, source))
        else {
            continue;
        };
        let value = parameter
            .child_by_field_name("value")
            .map(without_type_syntax);
        let body = initializer_body(value);
        walk.add_definition_with_body(parents, name, parameter.byte_range(), value, body);
    }
}

/// The declaration that `statement` holds: past its `export` and its `declare`. Returns `None`
/// when it holds none, as `export { a }` and `declare const X: T;` do.
fn declaration(statement: Node<'_>) -> Option<Node<'_>> {
    let mut node = statement;
    let mut cursor = statement.walk();
    loop {
        node = match node.kind() {
            "export_statement" => node.child_by_field_name("declaration")?,
            // `declare const` only states a type, and the value is elsewhere. A comment after
            // `declare` is a child too.
            "ambient_declaration" => node
                .named_children(&mut cursor)
                .find(|child| !child.is_extra())
                .filter(|child| {
                    !matches!(child.kind(), "lexical_declaration" | "variable_declaration")
                })?,
            // The grammar reads `namespace N {}` as an expression.
            "expression_statement" => node
                .named_children(&mut cursor)
                .find(|child| !child.is_extra())?,
            _ => return Some(node),
        };
    }
}

/// The largest integer JavaScript keeps without rounding (`Number.MAX_SAFE_INTEGER`).
///
/// JavaScript stores numeric keys as 64-bit floats, so integers past `2^53 - 1` can round (for
/// example, `9007199254740993` becomes `"9007199254740992"`).
const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;

/// Whether `text` is a canonical decimal integer that JavaScript turns into the exact same
/// property key string (`0`, or digits with no leading zero up to `2^53 - 1`).
fn is_decimal_integer_key(text: &str) -> bool {
    (text == "0" || text.starts_with(|c: char| matches!(c, '1'..='9')))
        && text.parse::<u64>().is_ok_and(|n| n <= MAX_SAFE_INTEGER)
}

/// Whether `node` is a declaration name that code can refer to, skipping destructuring patterns,
/// quoted names and `_`.
fn is_declaration_name(node: &Node, source: &str) -> bool {
    matches!(node.kind(), "identifier" | "type_identifier") && node_text(node, source) != "_"
}

/// The parts of `name`, the name of a namespace such as `A.B`, outermost first. Returns an empty
/// vector for quoted module names or any name with `_`.
fn namespace_segments<'tree>(name: Node<'tree>, source: &str) -> Vec<Node<'tree>> {
    let mut segments = Vec::new();
    let mut rest = Some(name);
    while let Some(node) = rest {
        match node.kind() {
            "nested_identifier" | "member_expression" => {
                let Some(property) = node.child_by_field_name("property") else {
                    return Vec::new();
                };
                if node_text(&property, source) == "_" {
                    return Vec::new();
                }
                segments.push(property);
                rest = node.child_by_field_name("object");
            }
            "identifier" if node_text(&node, source) != "_" => {
                segments.push(node);
                rest = None;
            }
            _ => return Vec::new(),
        }
    }
    segments.reverse();
    segments
}

/// The body of `value`, an unwrapped initializer, when it is an object literal or class expression.
fn initializer_body(value: Option<Node<'_>>) -> Option<Node<'_>> {
    let value = value?;
    match value.kind() {
        "object" => Some(value),
        "class" => value.child_by_field_name("body"),
        _ => None,
    }
}

/// `value` without the type syntax that TypeScript removes when it compiles, such as `as const`,
/// `satisfies T`, `<T>x` and a trailing `!`, and without the parentheses around it.
fn without_type_syntax(mut value: Node<'_>) -> Node<'_> {
    let mut cursor = value.walk();
    while matches!(
        value.kind(),
        "as_expression"
            | "satisfies_expression"
            | "non_null_expression"
            | "parenthesized_expression"
            | "type_assertion"
    ) {
        // A comment inside the parentheses is an extra child, and `<T>x` puts `type_arguments`
        // before the expression.
        match value
            .named_children(&mut cursor)
            .find(|child| !child.is_extra() && child.kind() != "type_arguments")
        {
            Some(inner) => value = inner,
            None => break,
        }
    }
    value
}

/// Adds the UTF-16 code units that `escape`, such as `\n` or `\u{e9}`, stands for to `units`.
///
/// # Errors
/// Returns an error if `escape` is a legacy octal escape, such as `\07`, or stands for no
/// character.
fn push_escape(escape: &str, units: &mut Vec<u16>) -> anyhow::Result<()> {
    // The grammar starts each escape with a backslash.
    let mut characters = escape.chars();
    characters.next();
    let first = code_symbols::escaped_character(&mut characters)?;
    let digits = characters.as_str();
    let character = match first {
        // A backslash at the end of a line joins the next line to it. JavaScript also ends a line
        // at U+2028 and U+2029.
        '\r' | '\n' | '\u{2028}' | '\u{2029}' => return Ok(()),
        'b' => '\u{8}',
        'f' => '\u{c}',
        'n' => '\n',
        'r' => '\r',
        't' => '\t',
        'v' => '\u{b}',
        '0' if digits.is_empty() => '\0',
        '0'..='9' => bail!("`{escape}` is a legacy escape, which blockwatch doesn't decode"),
        'x' | 'u' => {
            let code = u32::from_str_radix(digits.trim_matches(['{', '}']), 16).ok();
            // Such as half of a character.
            if let Some(unit) = code.and_then(|code| u16::try_from(code).ok()) {
                units.push(unit);
                return Ok(());
            }
            code.and_then(char::from_u32)
                .ok_or_else(|| anyhow!("`{escape}` stands for no character"))?
        }
        // JavaScript keeps the character after a backslash it doesn't know, such as `\d`.
        _ => first,
    };
    code_symbols::push_utf16(character, units);
    Ok(())
}

#[cfg(test)]
mod symbols_tests {
    use super::*;
    use crate::languages::code_symbols::test_utils::{
        decode_error, symbol, symbols, syntax_error, text_of, unresolved, value, values,
    };
    use crate::symbols::{Symbols, SymbolsParser};

    /// The symbols of `source`, a TypeScript file.
    fn typescript(source: &str) -> anyhow::Result<Symbols> {
        CodeSymbolsParser::new(
            &tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            EcmaScript,
        )
        .parse(source)
    }

    /// The symbols of `source`, a TSX file.
    fn tsx(source: &str) -> anyhow::Result<Symbols> {
        CodeSymbolsParser::new(&tree_sitter_typescript::LANGUAGE_TSX.into(), EcmaScript)
            .parse(source)
    }

    /// The symbols of `source`, a JavaScript file.
    fn javascript(source: &str) -> anyhow::Result<Symbols> {
        CodeSymbolsParser::new(&tree_sitter_javascript::LANGUAGE.into(), EcmaScript).parse(source)
    }

    #[test]
    fn file_with_every_typescript_shape_parse_symbols_returns_every_symbol_in_order()
    -> anyhow::Result<()> {
        let source = r#"import { readFile } from "fs";
import fs = require("fs");

/** Doc. */
export const MAX_RETRIES = 5;
const MODE = "dev" as const;
const A = 1, B = 2;
export let FIRST = 1, SECOND;
let PENDING;
var OLD = 1;
const /* c */ COMMENTED = (/* c */ 32);
const { x } = obj;
const [y] = list;
const _ = 3;
const LIMIT = (10 satisfies number);
const ROOT = document.body!;
const NESTED = 3 as unknown as Port;
const ENTITY_ROOT: Record<string, number> = {
  a: 1,
};
const translate = (reason: string) => reason;
function normalize(text: string): string {
  return text;
}
function overloaded(a: string): void;
function overloaded(a: number): void;
function overloaded(a: unknown) {}
function* generate() {}
export async function run() {}
@sealed
export class Point {}
abstract class Base {}
enum Color {
  Red = "red",
}
const enum Flag {
  On,
}
interface Shape {}
type Mode = "dev" | "prod";
namespace Tools {
  export const INNER = 1;
}
export namespace Shared {}
module Legacy {}
declare const DECLARED: number;
declare var GLOBAL: string;
declare function declared(): void;
declare class Ambient {}
declare /* c */ class CommentedAmbient {}
declare /* c */ const COMMENTED_DECLARED: number;
declare module "external" {}
declare global {}
export default function main() {}
export { MODE as ALIAS };
export * from "./other";
"#;

        // <block affects="docs/symbols.md:typescript-paths">
        assert_eq!(
            symbols(&typescript(source)?, source),
            vec![
                symbol("/MAX_RETRIES", "export const MAX_RETRIES = 5;", Some("5")),
                symbol("/MODE", r#"const MODE = "dev" as const;"#, Some("dev")),
                symbol("/A", "A = 1", Some("1")),
                symbol("/B", "B = 2", Some("2")),
                symbol("/FIRST", "FIRST = 1", Some("1")),
                symbol("/SECOND", "SECOND", None),
                symbol("/PENDING", "let PENDING;", None),
                symbol("/OLD", "var OLD = 1;", Some("1")),
                symbol(
                    "/COMMENTED",
                    "const /* c */ COMMENTED = (/* c */ 32);",
                    Some("32")
                ),
                symbol("/LIMIT", "const LIMIT = (10 satisfies number);", Some("10")),
                symbol(
                    "/ROOT",
                    "const ROOT = document.body!;",
                    Some("document.body")
                ),
                symbol("/NESTED", "const NESTED = 3 as unknown as Port;", Some("3")),
                symbol(
                    "/ENTITY_ROOT",
                    "const ENTITY_ROOT: Record<string, number> = {\n  a: 1,\n};",
                    Some("{\n  a: 1,\n}")
                ),
                symbol("/ENTITY_ROOT/a", "a: 1", Some("1")),
                symbol(
                    "/translate",
                    "const translate = (reason: string) => reason;",
                    Some("(reason: string) => reason")
                ),
                symbol(
                    "/normalize",
                    "function normalize(text: string): string {\n  return text;\n}",
                    None
                ),
                symbol("/overloaded", "function overloaded(a: unknown) {}", None),
                symbol("/generate", "function* generate() {}", None),
                symbol("/run", "export async function run() {}", None),
                symbol("/Point", "@sealed\nexport class Point {}", None),
                symbol("/Base", "abstract class Base {}", None),
                symbol("/Color", "enum Color {\n  Red = \"red\",\n}", None),
                symbol("/Color/Red", r#"Red = "red""#, Some("red")),
                symbol("/Flag", "const enum Flag {\n  On,\n}", None),
                symbol("/Flag/On", "On", None),
                symbol("/Shape", "interface Shape {}", None),
                symbol("/Mode", r#"type Mode = "dev" | "prod";"#, None),
                symbol(
                    "/Tools",
                    "namespace Tools {\n  export const INNER = 1;\n}",
                    None
                ),
                symbol("/Tools/INNER", "export const INNER = 1;", Some("1")),
                symbol("/Shared", "export namespace Shared {}", None),
                symbol("/Legacy", "module Legacy {}", None),
                symbol("/Ambient", "declare class Ambient {}", None),
                symbol(
                    "/CommentedAmbient",
                    "declare /* c */ class CommentedAmbient {}",
                    None
                ),
                symbol("/main", "export default function main() {}", None),
            ]
        );
        // </block>
        Ok(())
    }

    #[test]
    fn file_with_every_javascript_shape_parse_symbols_returns_every_symbol_in_order()
    -> anyhow::Result<()> {
        let source = r#"const PORT = 8080;
let counter = 0;
var legacy = "a";
const { a } = obj;
function helper() {}
function* items() {}
async function load() {}
@decorator
export class Widget {}
class Plain {}
const element = <div />;
module.exports = { PORT };
exports.extra = 1;
export default async function () {}
"#;

        assert_eq!(
            symbols(&javascript(source)?, source),
            vec![
                symbol("/PORT", "const PORT = 8080;", Some("8080")),
                symbol("/counter", "let counter = 0;", Some("0")),
                symbol("/legacy", r#"var legacy = "a";"#, Some("a")),
                symbol("/helper", "function helper() {}", None),
                symbol("/items", "function* items() {}", None),
                symbol("/load", "async function load() {}", None),
                symbol("/Widget", "@decorator\nexport class Widget {}", None),
                symbol("/Plain", "class Plain {}", None),
                symbol("/element", "const element = <div />;", Some("<div />")),
            ]
        );
        Ok(())
    }

    #[test]
    fn object_literals_parse_symbols_returns_nested_properties_through_type_syntax()
    -> anyhow::Result<()> {
        let source = r##"export const STRING_LIMITS = {
  SLUG: 40,
  BRANCH_NAME: 100,
} as const;

export const brand = {
  primary: "#80eaff",
  iconPlate: "#171c1e",
} satisfies Record<string, string>;

const ASSERTED = <const>{
  ok: true,
};
const TYPED = <Config>{
  nested: ({
    deep: "v" as const,
  })!,
};
let A = { x: 1 }, B = { y: 2 };
var KEYS = {
  "fast-mode": "fast",
  'safe-mode': 'safe',
  "esc\"quote": 1,
  "a/b": 2,
  "": 3,
  "_": 4,
  _: 5,
  200: "OK",
  PORT,
  get label() { return "lbl"; },
  format() {},
  [COMPUTED]: 9,
  ...OTHER,
};
const _ = { ignored: 1 };
"##;

        assert_eq!(
            symbols(&typescript(source)?, source),
            vec![
                symbol(
                    "/STRING_LIMITS",
                    "export const STRING_LIMITS = {\n  SLUG: 40,\n  BRANCH_NAME: 100,\n} as const;",
                    Some("{\n  SLUG: 40,\n  BRANCH_NAME: 100,\n}")
                ),
                symbol("/STRING_LIMITS/SLUG", "SLUG: 40", Some("40")),
                symbol(
                    "/STRING_LIMITS/BRANCH_NAME",
                    "BRANCH_NAME: 100",
                    Some("100")
                ),
                symbol(
                    "/brand",
                    "export const brand = {\n  primary: \"#80eaff\",\n  iconPlate: \"#171c1e\",\n} satisfies Record<string, string>;",
                    Some("{\n  primary: \"#80eaff\",\n  iconPlate: \"#171c1e\",\n}")
                ),
                symbol("/brand/primary", r##"primary: "#80eaff""##, Some("#80eaff")),
                symbol(
                    "/brand/iconPlate",
                    r##"iconPlate: "#171c1e""##,
                    Some("#171c1e")
                ),
                symbol(
                    "/ASSERTED",
                    "const ASSERTED = <const>{\n  ok: true,\n};",
                    Some("{\n  ok: true,\n}")
                ),
                symbol("/ASSERTED/ok", "ok: true", Some("true")),
                symbol(
                    "/TYPED",
                    "const TYPED = <Config>{\n  nested: ({\n    deep: \"v\" as const,\n  })!,\n};",
                    Some("{\n  nested: ({\n    deep: \"v\" as const,\n  })!,\n}")
                ),
                symbol(
                    "/TYPED/nested",
                    "nested: ({\n    deep: \"v\" as const,\n  })!",
                    Some("{\n    deep: \"v\" as const,\n  }")
                ),
                symbol("/TYPED/nested/deep", r#"deep: "v" as const"#, Some("v")),
                symbol("/A", "A = { x: 1 }", Some("{ x: 1 }")),
                symbol("/A/x", "x: 1", Some("1")),
                symbol("/B", "B = { y: 2 }", Some("{ y: 2 }")),
                symbol("/B/y", "y: 2", Some("2")),
                symbol(
                    "/KEYS",
                    text_of(source, "var KEYS", "\n};")?,
                    Some(text_of(source, "{\n  \"fast-mode\"", "\n}")?)
                ),
                symbol("/KEYS/fast-mode", r#""fast-mode": "fast""#, Some("fast")),
                symbol("/KEYS/safe-mode", "'safe-mode': 'safe'", Some("safe")),
                symbol("/KEYS/esc%22quote", r#""esc\"quote": 1"#, Some("1")),
                symbol("/KEYS/a~1b", r#""a/b": 2"#, Some("2")),
                symbol("/KEYS/", r#""": 3"#, Some("3")),
                symbol("/KEYS/_", r#""_": 4"#, Some("4")),
                symbol("/KEYS/_", "_: 5", Some("5")),
                symbol("/KEYS/200", r#"200: "OK""#, Some("OK")),
                symbol("/KEYS/PORT", "PORT", Some("PORT")),
                symbol("/KEYS/label", r#"get label() { return "lbl"; }"#, None),
                symbol("/KEYS/format", "format() {}", None),
            ]
        );

        let js_source = r##"export const BRAND = {
  primary: "#80eaff",
  nested: { gap: 8 },
};
"##;
        assert_eq!(
            values(&javascript(js_source)?, js_source),
            vec![
                value(
                    "/BRAND",
                    "{\n  primary: \"#80eaff\",\n  nested: { gap: 8 },\n}"
                ),
                value("/BRAND/primary", "#80eaff"),
                value("/BRAND/nested", "{ gap: 8 }"),
                value("/BRAND/nested/gap", "8"),
            ]
        );

        let tsx_source = r#"export const UI = {
  badge: <Badge label="new" />,
} as const;
"#;
        assert_eq!(
            values(&tsx(tsx_source)?, tsx_source),
            vec![
                value("/UI", "{\n  badge: <Badge label=\"new\" />,\n}"),
                value("/UI/badge", r#"<Badge label="new" />"#),
            ]
        );
        Ok(())
    }

    #[test]
    fn numeric_property_keys_parse_symbols_keeps_only_canonical_decimal_integers()
    -> anyhow::Result<()> {
        let source = r#"const CODES = {
  0: "zero",
  200: "OK",
  9007199254740991: "max",
  0x10: { nested: 1 },
  0o10: 1,
  0b10: 1,
  010: 1,
  00: 1,
  1_000: 1,
  1.0: 1,
  1.5: 1,
  1e2: 1,
  1n: 1,
  9007199254740992: 1,
};
"#;
        assert_eq!(
            values(&typescript(source)?, source),
            vec![
                value("/CODES", text_of(source, "{\n", "\n}")?),
                value("/CODES/0", "zero"),
                value("/CODES/200", "OK"),
                value("/CODES/9007199254740991", "max"),
            ]
        );
        Ok(())
    }

    #[test]
    fn class_members_parse_symbols_returns_fields_methods_and_nested_objects() -> anyhow::Result<()>
    {
        let ts_source = r#"@sealed
export class Config {
  static readonly MAX_RETRIES = 5 as const;
  @dec
  label: string = "cfg";
  uninitialized?: number;
  #secret = "s";
  "quoted-field" = 1;
  42 = "answer";
  static readonly LIMITS = {
    SLUG: 40,
  } as const;
  [COMPUTED] = 0;
  [k: string]: unknown;
  static {
    const local = 1;
  }

  constructor(
    @tag public readonly id: number,
    private name = "anon",
    protected role?: string,
    readonly cfg = { a: 1 } as const,
    plain: boolean,
  ) {}

  get value() { return 1; }
  set value(v: number) {}

  @a
  // comment between decorators
  @b
  async run() {
    const localInMethod = 1;
  }

  *gen() {}
  #hidden() {}
  static constructor() {}

  format(a: string): string;
  format(a: number): string;
  format(a: unknown) { return String(a); }
}

abstract class Base {
  abstract size: number;
  abstract stop(): void;
}

declare class Ambient {
  static VERSION: string;
  ping(): void;
}

const Expr = class {
  static X = 1;
};

class Derived extends Base {
  constructor(override value: number) {
    super();
  }
}
"#;
        let config_text = text_of(ts_source, "@sealed", "\n}")?;
        let run_text = text_of(ts_source, "@a", "  }")?;
        let format_impl = "format(a: unknown) { return String(a); }";

        assert_eq!(
            symbols(&typescript(ts_source)?, ts_source),
            vec![
                symbol("/Config", config_text, None),
                symbol(
                    "/Config/MAX_RETRIES",
                    "static readonly MAX_RETRIES = 5 as const",
                    Some("5")
                ),
                symbol(
                    "/Config/label",
                    "@dec\n  label: string = \"cfg\"",
                    Some("cfg")
                ),
                symbol("/Config/uninitialized", "uninitialized?: number", None),
                symbol("/Config/%23secret", r#"#secret = "s""#, Some("s")),
                symbol("/Config/quoted-field", r#""quoted-field" = 1"#, Some("1")),
                symbol("/Config/42", r#"42 = "answer""#, Some("answer")),
                symbol(
                    "/Config/LIMITS",
                    "static readonly LIMITS = {\n    SLUG: 40,\n  } as const",
                    Some("{\n    SLUG: 40,\n  }")
                ),
                symbol("/Config/LIMITS/SLUG", "SLUG: 40", Some("40")),
                symbol("/Config/id", "@tag public readonly id: number", None),
                symbol("/Config/name", r#"private name = "anon""#, Some("anon")),
                symbol("/Config/role", "protected role?: string", None),
                symbol(
                    "/Config/cfg",
                    "readonly cfg = { a: 1 } as const",
                    Some("{ a: 1 }")
                ),
                symbol("/Config/cfg/a", "a: 1", Some("1")),
                symbol("/Config/value", "get value() { return 1; }", None),
                symbol("/Config/value", "set value(v: number) {}", None),
                symbol("/Config/run", run_text, None),
                symbol("/Config/gen", "*gen() {}", None),
                symbol("/Config/%23hidden", "#hidden() {}", None),
                symbol("/Config/constructor", "static constructor() {}", None),
                symbol("/Config/format", format_impl, None),
                symbol(
                    "/Base",
                    text_of(ts_source, "abstract class Base", "\n}")?,
                    None
                ),
                symbol("/Base/size", "abstract size: number", None),
                symbol("/Base/stop", "abstract stop(): void", None),
                symbol(
                    "/Ambient",
                    text_of(ts_source, "declare class Ambient", "\n}")?,
                    None
                ),
                symbol("/Ambient/VERSION", "static VERSION: string", None),
                symbol(
                    "/Expr",
                    "const Expr = class {\n  static X = 1;\n};",
                    Some("class {\n  static X = 1;\n}")
                ),
                symbol("/Expr/X", "static X = 1", Some("1")),
                symbol(
                    "/Derived",
                    text_of(ts_source, "class Derived", "\n}")?,
                    None
                ),
                symbol("/Derived/value", "override value: number", None),
            ]
        );

        let js_source = r#"class Widget {
  @dec
  static MAX = 10;
  count;
  #secret = "js";
  defaults = { retries: 3 };
  static { const x = 1; }
  constructor(x) {}
  @dec
  async render() {}
  get kind() { return "w"; }
  static get
  constructor() { return 1; }
  #helper() {}
}
"#;
        assert_eq!(
            symbols(&javascript(js_source)?, js_source),
            vec![
                symbol("/Widget", text_of(js_source, "class Widget", "\n}")?, None),
                symbol("/Widget/MAX", "@dec\n  static MAX = 10", Some("10")),
                symbol("/Widget/count", "count", None),
                symbol("/Widget/%23secret", r#"#secret = "js""#, Some("js")),
                symbol(
                    "/Widget/defaults",
                    "defaults = { retries: 3 }",
                    Some("{ retries: 3 }")
                ),
                symbol("/Widget/defaults/retries", "retries: 3", Some("3")),
                symbol("/Widget/render", "@dec\n  async render() {}", None),
                symbol("/Widget/kind", r#"get kind() { return "w"; }"#, None),
                symbol(
                    "/Widget/constructor",
                    "static get\n  constructor() { return 1; }",
                    None
                ),
                symbol("/Widget/%23helper", "#helper() {}", None),
            ]
        );
        Ok(())
    }

    #[test]
    fn enum_members_parse_symbols_returns_values_or_definitions() -> anyhow::Result<()> {
        let source = r#"export enum Status {
  OPEN = "open",
  CLOSED = (2),
  PENDING,
  "Quoted-Key" = "q",
  "Bare",
  [COMPUTED] = 1,
}

const enum Flags {
  NONE = 0,
}

declare enum AmbientEnum {
  A = 1,
  B,
}
"#;

        assert_eq!(
            symbols(&typescript(source)?, source),
            vec![
                symbol(
                    "/Status",
                    text_of(source, "export enum Status", "\n}")?,
                    None
                ),
                symbol("/Status/OPEN", r#"OPEN = "open""#, Some("open")),
                symbol("/Status/CLOSED", "CLOSED = (2)", Some("2")),
                symbol("/Status/PENDING", "PENDING", None),
                symbol("/Status/Quoted-Key", r#""Quoted-Key" = "q""#, Some("q")),
                symbol("/Status/Bare", r#""Bare""#, None),
                symbol("/Flags", "const enum Flags {\n  NONE = 0,\n}", None),
                symbol("/Flags/NONE", "NONE = 0", Some("0")),
                symbol(
                    "/AmbientEnum",
                    "declare enum AmbientEnum {\n  A = 1,\n  B,\n}",
                    None
                ),
                symbol("/AmbientEnum/A", "A = 1", Some("1")),
                symbol("/AmbientEnum/B", "B", None),
            ]
        );
        Ok(())
    }

    #[test]
    fn interface_members_parse_symbols_returns_property_and_method_signatures() -> anyhow::Result<()>
    {
        let source = r#"export interface Options {
  readonly retries?: number;
  "content-type": string;
  200: string;
  [key: string]: unknown;
  [Symbol.iterator](): Iterator<string>;
  (x: number): void;
  new (): Options;
  reset(): void;
}
"#;

        assert_eq!(
            symbols(&typescript(source)?, source),
            vec![
                symbol(
                    "/Options",
                    text_of(source, "export interface Options", "\n}")?,
                    None
                ),
                symbol("/Options/retries", "readonly retries?: number", None),
                symbol("/Options/content-type", r#""content-type": string"#, None),
                symbol("/Options/200", "200: string", None),
                symbol("/Options/reset", "reset(): void", None),
            ]
        );
        Ok(())
    }

    #[test]
    fn namespace_members_parse_symbols_returns_nested_and_dotted_members() -> anyhow::Result<()> {
        let source = r#"export namespace Api {
  export const VERSION = "1.0";
  const INTERNAL = 2;
  export const LIMITS = { MAX: 10 } as const;
  export enum Status { OPEN = "open" }
  export class Client { static RETRIES = 3; }
  export interface Opts { retries: number; }
  export namespace V2 {
    export const PATH = "/v2";
  }
}

namespace A.B.C {
  export const X = 1;
}

module Legacy {
  export const Y = 2;
}

declare namespace Ambient {
  export const VERSION: string;
  export enum Mode { Fast = 1 }
}

namespace Merged { export const A = 1; }
namespace Merged { export const B = 2; }

declare module "external" { export const X = 1; }
declare global { const G = 1; }
namespace _ { export const X = 1; }
namespace A._ { export const X = 1; }
"#;

        let parsed = typescript(source)?;
        assert_eq!(
            values(&parsed, source),
            vec![
                ("/Api".to_string(), None),
                value("/Api/VERSION", "1.0"),
                value("/Api/INTERNAL", "2"),
                value("/Api/LIMITS", "{ MAX: 10 }"),
                value("/Api/LIMITS/MAX", "10"),
                ("/Api/Status".to_string(), None),
                value("/Api/Status/OPEN", "open"),
                ("/Api/Client".to_string(), None),
                value("/Api/Client/RETRIES", "3"),
                ("/Api/Opts".to_string(), None),
                ("/Api/Opts/retries".to_string(), None),
                ("/Api/V2".to_string(), None),
                value("/Api/V2/PATH", "/v2"),
                ("/A/B/C".to_string(), None),
                value("/A/B/C/X", "1"),
                ("/Legacy".to_string(), None),
                value("/Legacy/Y", "2"),
                ("/Ambient".to_string(), None),
                ("/Ambient/VERSION".to_string(), None),
                ("/Ambient/Mode".to_string(), None),
                value("/Ambient/Mode/Fast", "1"),
                ("/Merged".to_string(), None),
                value("/Merged/A", "1"),
                ("/Merged".to_string(), None),
                value("/Merged/B", "2"),
            ]
        );
        assert_eq!(
            unresolved(&parsed, source, "/Merged")?,
            "ambiguous symbol, defined at 26:1, 27:1"
        );
        Ok(())
    }

    #[test]
    fn duplicate_members_or_keys_parse_symbols_reports_ambiguous() -> anyhow::Result<()> {
        let source = r#"const D = {
  a: 1,
  "a": 2,
  'a': 3,
  "\x61": 4,
};

class Box {
  get size() { return 1; }
  set size(v: number) {}
}

const Obj = {
  get x() { return 1; },
  set x(v: number) {},
};

const Codes = {
  200: "a",
  "200": "b",
};

interface Overloaded {
  f(a: string): void;
  f(a: number): void;
}

abstract class Abs {
  abstract g(a: string): void;
  abstract g(a: number): void;
}
"#;
        let parsed = typescript(source)?;
        assert_eq!(
            unresolved(&parsed, source, "/D/a")?,
            "ambiguous symbol, defined at 2:3, 3:3, 4:3, 5:3"
        );
        assert_eq!(
            unresolved(&parsed, source, "/Box/size")?,
            "ambiguous symbol, defined at 9:3, 10:3"
        );
        assert_eq!(
            unresolved(&parsed, source, "/Obj/x")?,
            "ambiguous symbol, defined at 14:3, 15:3"
        );
        assert_eq!(
            unresolved(&parsed, source, "/Codes/200")?,
            "ambiguous symbol, defined at 19:3, 20:3"
        );
        assert_eq!(
            unresolved(&parsed, source, "/Overloaded/f")?,
            "ambiguous symbol, defined at 24:3, 25:3"
        );
        assert_eq!(
            unresolved(&parsed, source, "/Abs/g")?,
            "ambiguous symbol, defined at 29:3, 30:3"
        );
        Ok(())
    }

    #[test]
    fn undecodable_string_key_or_value_parse_symbols_keeps_other_entries_readable()
    -> anyhow::Result<()> {
        let source = r#"const D = {
  "\07": { nested: 1 },
  "\uD800": 2,
  "bad_val": "\uD800",
  "ok": "fine",
};

class BadClass {
  "\07"() {}
  ok() {}
}

enum BadEnum {
  "\07" = 1,
  OK = 2,
}

interface BadInterface {
  "\07": number;
  ok: number;
}
"#;
        let parsed = typescript(source)?;
        assert_eq!(
            values(&parsed, source),
            vec![
                value(
                    "/D",
                    "{\n  \"\\07\": { nested: 1 },\n  \"\\uD800\": 2,\n  \"bad_val\": \"\\uD800\",\n  \"ok\": \"fine\",\n}"
                ),
                value("/D/ok", "fine"),
                ("/BadClass".to_string(), None),
                ("/BadClass/ok".to_string(), None),
                ("/BadEnum".to_string(), None),
                value("/BadEnum/OK", "2"),
                ("/BadInterface".to_string(), None),
                ("/BadInterface/ok".to_string(), None),
            ]
        );
        assert_eq!(
            unresolved(&parsed, source, "/D/bad_val")?,
            decode_error(
                4,
                14,
                "the string holds U+D800, which is only half of a character"
            )
        );
        Ok(())
    }

    #[test]
    fn syntax_errors_in_nested_bodies_parse_symbols_makes_affected_paths_unreadable()
    -> anyhow::Result<()> {
        let source = r#"class Broken {
  BAD = = 1;
  OK = 2;
  #privBroken = = 1;
  $dollarBroken = = 1;
}

enum BadEnum {
  BAD = = 1,
  OK = 2,
}

namespace BadNs {
  export const BAD = = 1;
  export const OK = 2;
}

const CONFIG = {
  "fast-mode": "fast",
  "bad": = 1,
  "double-broken" "missing-colon",
  'single-broken' 'missing-colon',
};
"#;
        let parsed = typescript(source)?;
        assert_eq!(
            values(&parsed, source),
            vec![
                value("/Broken/OK", "2"),
                value("/BadEnum/OK", "2"),
                value("/BadNs/OK", "2"),
                value("/CONFIG/fast-mode", "fast"),
            ]
        );
        assert_eq!(
            unresolved(&parsed, source, "/Broken")?,
            syntax_error(2, 7, "Broken")
        );
        assert_eq!(
            unresolved(&parsed, source, "/Broken/BAD")?,
            syntax_error(2, 7, "BAD")
        );
        assert_eq!(
            unresolved(&parsed, source, "/Broken/%23privBroken")?,
            syntax_error(4, 15, "#privBroken")
        );
        assert_eq!(
            unresolved(&parsed, source, "/Broken/$dollarBroken")?,
            syntax_error(5, 17, "$dollarBroken")
        );
        assert_eq!(
            unresolved(&parsed, source, "/BadEnum/BAD")?,
            syntax_error(9, 7, "BAD")
        );
        assert_eq!(
            unresolved(&parsed, source, "/BadNs/BAD")?,
            syntax_error(14, 20, "BAD")
        );
        assert_eq!(
            unresolved(&parsed, source, "/CONFIG/bad")?,
            syntax_error(20, 10, "bad")
        );
        assert_eq!(
            unresolved(&parsed, source, "/CONFIG/double-broken")?,
            syntax_error(21, 3, "double-broken")
        );
        assert_eq!(
            unresolved(&parsed, source, "/CONFIG/single-broken")?,
            syntax_error(22, 3, "single-broken")
        );
        Ok(())
    }

    #[test]
    fn tsx_file_parse_symbols_returns_its_symbols() -> anyhow::Result<()> {
        let source = r#"interface Props {}
export const App = (props: Props) => <div className="app" />;
"#;

        assert_eq!(
            symbols(&tsx(source)?, source),
            vec![
                symbol("/Props", "interface Props {}", None),
                symbol(
                    "/App",
                    r#"export const App = (props: Props) => <div className="app" />;"#,
                    Some(r#"(props: Props) => <div className="app" />"#)
                ),
            ]
        );
        Ok(())
    }

    #[test]
    fn string_kinds_parse_symbols_returns_decoded_values() -> anyhow::Result<()> {
        let source = r#"const ESCAPES = "a\n\r\t\b\f\v\0\'\"\\\x41é\u{1F600}😀\d";
const SPLIT = "a\
b";
const SINGLE = 'a"b';
const TEMPLATE = `a
b\n\`c`;
const FIELD = `a${x}`;
const TAGGED = String.raw`a\n`;
const EMPTY = "";
"#;

        // The three grammars give strings the same nodes.
        for symbols in [javascript(source)?, typescript(source)?, tsx(source)?] {
            assert_eq!(
                values(&symbols, source),
                vec![
                    // An escape of half a character pairs with the next one: `😀` is 😀.
                    // JavaScript keeps the character after an unknown escape, such as `\d`.
                    value("/ESCAPES", "a\n\r\t\u{8}\u{c}\u{b}\0'\"\\Aé😀😀d"),
                    value("/SPLIT", "ab"),
                    value("/SINGLE", "a\"b"),
                    value("/TEMPLATE", "a\nb\n`c"),
                    value("/FIELD", "`a${x}`"),
                    value("/TAGGED", r"String.raw`a\n`"),
                    value("/EMPTY", ""),
                ]
            );
        }
        Ok(())
    }

    #[test]
    fn line_ends_other_than_a_line_feed_parse_symbols_returns_decoded_values() -> anyhow::Result<()>
    {
        // A template reads a CRLF or CR line end as a line feed. A backslash joins the next line to
        // its own at any line end, and JavaScript also ends a line at U+2028 and U+2029.
        let source = "const SPLIT = \"a\\\r\nb\";\r\nconst TEMPLATE = `a\r\nb`;\r\n\
            const CR_TEMPLATE = `a\rb`;\r\n\
            const LINE_SEPARATOR = \"a\\\u{2028}b\";\r\nconst PARAGRAPH_SEPARATOR = \"a\\\u{2029}b\";\r\n";

        assert_eq!(
            values(&typescript(source)?, source),
            vec![
                value("/SPLIT", "ab"),
                value("/TEMPLATE", "a\nb"),
                value("/CR_TEMPLATE", "a\nb"),
                value("/LINE_SEPARATOR", "ab"),
                value("/PARAGRAPH_SEPARATOR", "ab"),
            ]
        );
        Ok(())
    }

    #[test]
    fn escape_that_is_not_text_parse_symbols_makes_the_name_unreadable() -> anyhow::Result<()> {
        let source = r#"const OCTAL = "\07";
const DIGIT = "\8";
const HALF = "\uD800";
const HALF_BRACED = "a\u{DC00}";
const TOO_BIG = "\u{110000}";
const FINE = "ok";
"#;

        assert_eq!(
            symbols(&typescript(source)?, source),
            vec![symbol("/FINE", r#"const FINE = "ok";"#, Some("ok"))]
        );
        assert_eq!(
            unresolved(&typescript(source)?, source, "/OCTAL")?,
            decode_error(
                1,
                15,
                r"`\07` is a legacy escape, which blockwatch doesn't decode"
            )
        );
        assert_eq!(
            unresolved(&typescript(source)?, source, "/DIGIT")?,
            decode_error(
                2,
                15,
                r"`\8` is a legacy escape, which blockwatch doesn't decode"
            )
        );
        assert_eq!(
            unresolved(&typescript(source)?, source, "/HALF")?,
            decode_error(
                3,
                14,
                "the string holds U+D800, which is only half of a character"
            )
        );
        assert_eq!(
            unresolved(&typescript(source)?, source, "/HALF_BRACED")?,
            decode_error(
                4,
                21,
                "the string holds U+DC00, which is only half of a character"
            )
        );
        assert_eq!(
            unresolved(&typescript(source)?, source, "/TOO_BIG")?,
            decode_error(5, 17, r"`\u{110000}` stands for no character")
        );
        Ok(())
    }

    #[test]
    fn syntax_errors_parse_symbols_makes_the_names_near_them_unreadable() -> anyhow::Result<()> {
        let source = r"const PORTS = 8;
function helper() {
  return = 1;
}
const BROKEN = = 1;
const AFTER = 3;
";

        assert_eq!(
            symbols(&typescript(source)?, source),
            vec![
                symbol("/PORTS", "const PORTS = 8;", Some("8")),
                symbol("/AFTER", "const AFTER = 3;", Some("3")),
            ]
        );
        assert_eq!(
            unresolved(&typescript(source)?, source, "/helper")?,
            syntax_error(3, 10, "helper")
        );
        assert_eq!(
            unresolved(&typescript(source)?, source, "/BROKEN")?,
            syntax_error(5, 14, "BROKEN")
        );
        Ok(())
    }

    #[test]
    fn span_that_swallows_definitions_parse_symbols_makes_their_names_unreadable()
    -> anyhow::Result<()> {
        // The unclosed `{` makes the parser skip the lines after it, `AFTER` included.
        let source = r"const PORTS = 8;
const CONFIG = {
  a: 1,

const AFTER = 3;
";

        assert_eq!(
            symbols(&typescript(source)?, source),
            vec![symbol("/PORTS", "const PORTS = 8;", Some("8"))]
        );
        for name in ["CONFIG", "AFTER"] {
            assert_eq!(
                unresolved(&typescript(source)?, source, &format!("/{name}"))?,
                syntax_error(2, 14, name)
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
    fn parses_typescript_comments_correctly() {
        let mut comments_parser = comments_parser();

        // The HTML-like comments are single-line: the statement between the `<!--` and `-->`
        // lines is code, not comment content.
        let blocks: Vec<Comment> = comments_parser
            .parse(
                r"
            /**
             * This is a TypeScript class example with comments.
             *
             * @class Example
             */
            class Example {
                // This is a single-line comment in TypeScript.
                private value: number;

                /*
                 * This is a multi-line comment
                 * that spans across several lines.
                 */
                constructor(value: number) {
                    this.value = value; /* Inline multi-line comment */
                }

                // Method to get the value
                public getValue(): number {
                    return this.value; // Inline comment next to a return statement
                }
            }
/// Triple slash comment.
let done = 1;
<!-- html open comment
let between = 2;
--> html close comment
//! Rust and Doxygen read this marker as a doc comment.
//// A banner line repeats the marker.
            ",
            )
            .collect();

        assert_eq!(
            blocks,
            vec![
                Comment {
                    position_range: Position::new(2, 13)..Position::new(6, 16),
                    source_range: 13..142,
                    comment_text:
                        "   \n               This is a TypeScript class example with comments.\n              \n               @class Example\n               "
                            .to_string()
                },
                Comment {
                    position_range: Position::new(8, 17)..Position::new(8, 64),
                    source_range: 187..234,
                    comment_text: "   This is a single-line comment in TypeScript.".to_string()
                },
                Comment {
                    position_range: Position::new(11, 17)..Position::new(14, 20),
                    source_range: 291..413,
                    comment_text:
                        "  \n                   This is a multi-line comment\n                   that spans across several lines.\n                   "
                            .to_string()
                },
                Comment {
                    position_range: Position::new(16, 41)..Position::new(16, 72),
                    source_range: 499..530,
                    comment_text: "   Inline multi-line comment   ".to_string()
                },
                Comment {
                    position_range: Position::new(19, 17)..Position::new(19, 43),
                    source_range: 566..592,
                    comment_text: "   Method to get the value".to_string()
                },
                Comment {
                    position_range: Position::new(21, 40)..Position::new(21, 84),
                    source_range: 676..720,
                    comment_text: "   Inline comment next to a return statement".to_string()
                },
                Comment {
                    position_range: Position::new(24, 1)..Position::new(24, 26),
                    source_range: 753..778,
                    comment_text: "    Triple slash comment.".to_string()
                },
                Comment {
                    position_range: Position::new(26, 1)..Position::new(26, 23),
                    source_range: 793..815,
                    comment_text: "     html open comment".to_string()
                },
                Comment {
                    position_range: Position::new(28, 1)..Position::new(28, 23),
                    source_range: 833..855,
                    comment_text: "    html close comment".to_string()
                },
                Comment {
                    position_range: Position::new(29, 1)..Position::new(29, 56),
                    source_range: 856..911,
                    comment_text: "    Rust and Doxygen read this marker as a doc comment.".to_string()
                },
                Comment {
                    position_range: Position::new(30, 1)..Position::new(30, 39),
                    source_range: 912..950,
                    comment_text: "     A banner line repeats the marker.".to_string()
                },
            ]
        );
    }
}
