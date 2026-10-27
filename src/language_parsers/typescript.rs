use crate::language_parsers;
use crate::language_parsers::{CommentsParser, LanguageParser, LanguageParserImpl, node_text};
use crate::symbols::code::{CodeLanguage, CodeSymbolsParser, Definition, Value};
use anyhow::{anyhow, bail};
use std::ops::Range;
use tree_sitter::Node;

/// Returns a [`LanguageParser`] for TypeScript, with its symbols.
pub(super) fn parser() -> anyhow::Result<impl LanguageParser> {
    Ok(
        LanguageParserImpl::new(comments_parser()?).with_symbols(CodeSymbolsParser::new(
            &tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            EcmaScript,
        )?),
    )
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    language_parsers::ecmascript_comments_parser(
        &tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
    )
}

/// TypeScript or JavaScript, whose top-level names are variables, functions, classes, enums,
/// interfaces, type aliases and namespaces.
pub(super) struct EcmaScript;

impl CodeLanguage for EcmaScript {
    /// The top-level definitions under `root`, the root of a TypeScript or JavaScript file's syntax
    /// tree, in document order. `source` is the text of the file.
    fn definitions<'tree>(&self, root: Node<'tree>, source: &str) -> Vec<Definition<'tree>> {
        let mut definitions = Vec::new();
        let mut cursor = root.walk();
        for statement in root.named_children(&mut cursor) {
            let Some(declaration) = declaration(statement) else {
                continue;
            };
            match declaration.kind() {
                "lexical_declaration" | "variable_declaration" => {
                    let mut inner = declaration.walk();
                    let declarators: Vec<Node> = declaration
                        .named_children(&mut inner)
                        .filter(|child| child.kind() == "variable_declarator")
                        .collect();
                    // In `const A = 1, B = 2;`, each name covers only its own part.
                    let several = declarators.len() > 1;
                    for declarator in declarators {
                        let range = if several {
                            declarator.byte_range()
                        } else {
                            statement.byte_range()
                        };
                        let value = declarator
                            .child_by_field_name("value")
                            .map(|value| Value::Node(without_type_syntax(value)));
                        definitions.extend(definition(declarator, range, value, source));
                    }
                }
                "function_declaration"
                | "generator_function_declaration"
                | "class_declaration"
                | "abstract_class_declaration"
                | "interface_declaration"
                | "type_alias_declaration"
                | "enum_declaration"
                | "internal_module"
                | "module" => {
                    definitions.extend(definition(
                        declaration,
                        statement.byte_range(),
                        None,
                        source,
                    ));
                }
                _ => {}
            }
        }
        definitions
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
                // A template reads a CRLF line end as a line feed. A string can't hold a line end.
                _ => units.extend(written.replace("\r\n", "\n").encode_utf16()),
            }
        }
        char::decode_utf16(units)
            .collect::<Result<String, _>>()
            .map(Some)
            .map_err(|error| {
                anyhow!(
                    "the string holds U+{:04X}, which is only half of a character",
                    error.unpaired_surrogate()
                )
            })
    }
}

/// The declaration that `statement`, a top-level statement, holds: past its `export` and its
/// `declare`. Returns `None` when it holds none, as `export { a }` and `declare const X: T;` do.
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
            "expression_statement" => node.named_child(0)?,
            _ => return Some(node),
        };
    }
}

/// The definition of the name that `node` holds in its `name` field, covering `range`. Returns
/// `None` when the name is not one code can refer to, such as a pattern in `const { a } = b`,
/// the quoted name in `declare module "x"`, or `_`.
fn definition<'tree>(
    node: Node<'tree>,
    range: Range<usize>,
    value: Option<Value<'tree>>,
    source: &str,
) -> Option<Definition<'tree>> {
    let name = node.child_by_field_name("name").filter(|name| {
        matches!(name.kind(), "identifier" | "type_identifier") && node_text(name, source) != "_"
    })?;
    Some(Definition {
        names: vec![name],
        range,
        value,
    })
}

/// `value` without the type syntax that TypeScript removes when it compiles, such as `as const`,
/// `satisfies T` and a trailing `!`, and without the parentheses around it.
fn without_type_syntax(mut value: Node<'_>) -> Node<'_> {
    let mut cursor = value.walk();
    while matches!(
        value.kind(),
        "as_expression"
            | "satisfies_expression"
            | "non_null_expression"
            | "parenthesized_expression"
    ) {
        // A comment inside the parentheses is a child too, but not part of the value.
        match value
            .named_children(&mut cursor)
            .find(|child| !child.is_extra())
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
    // The grammar starts each escape with a backslash and at least one more character.
    let mut characters = escape.chars();
    characters.next();
    let first = characters.next().unwrap_or_default();
    let digits = characters.as_str();
    let character = match first {
        // A backslash at the end of a line joins the next line to it.
        '\r' | '\n' => return Ok(()),
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
    units.extend(character.encode_utf16(&mut [0; 2]).iter());
    Ok(())
}

#[cfg(test)]
mod symbols_tests {
    use super::*;
    use crate::symbols::code::test_utils::{
        decode_error, symbol, symbols, syntax_error, unresolved, value, values,
    };
    use crate::symbols::{Symbols, SymbolsParser};

    /// The symbols of `source`, a TypeScript file.
    fn typescript(source: &str) -> anyhow::Result<Symbols> {
        CodeSymbolsParser::new(
            &tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            EcmaScript,
        )?
        .parse(source)
    }

    /// The symbols of `source`, a TSX file.
    fn tsx(source: &str) -> anyhow::Result<Symbols> {
        CodeSymbolsParser::new(&tree_sitter_typescript::LANGUAGE_TSX.into(), EcmaScript)?
            .parse(source)
    }

    /// The symbols of `source`, a JavaScript file.
    fn javascript(source: &str) -> anyhow::Result<Symbols> {
        CodeSymbolsParser::new(&tree_sitter_javascript::LANGUAGE.into(), EcmaScript)?.parse(source)
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

        // <block affects="docs/symbols.md:typescript-paths, src/skill.md#/blockwatch/Symbols/TypeScript and JavaScript">
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
                symbol("/Flag", "const enum Flag {\n  On,\n}", None),
                symbol("/Shape", "interface Shape {}", None),
                symbol("/Mode", r#"type Mode = "dev" | "prod";"#, None),
                symbol(
                    "/Tools",
                    "namespace Tools {\n  export const INNER = 1;\n}",
                    None
                ),
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
    fn crlf_line_endings_parse_symbols_reads_line_feeds() -> anyhow::Result<()> {
        // A template reads every line end as a line feed.
        let source = "const SPLIT = \"a\\\r\nb\";\r\nconst TEMPLATE = `a\r\nb`;\r\n";

        assert_eq!(
            values(&typescript(source)?, source),
            vec![value("/SPLIT", "ab"), value("/TEMPLATE", "a\nb")]
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
        let source = r#"const PORTS = 8;
function helper() {
  return = 1;
}
const BROKEN = = 1;
const AFTER = 3;
"#;

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
        let source = r#"const PORTS = 8;
const CONFIG = {
  a: 1,

const AFTER = 3;
"#;

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
    use crate::{Position, language_parsers::Comment};

    #[test]
    fn parses_typescript_comments_correctly() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser()?;

        // The HTML-like comments are single-line: the statement between the `<!--` and `-->`
        // lines is code, not comment content.
        let blocks: Vec<Comment> = comments_parser
            .parse(
                r#"
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
            "#,
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
                    comment_text: "  / Triple slash comment.".to_string()
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
            ]
        );

        Ok(())
    }
}
