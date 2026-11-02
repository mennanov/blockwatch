use crate::language_parsers::{
    CommentsParser, LanguageParser, LanguageParserImpl, c_style_line_and_block_comments_parser,
    node_text,
};
use crate::symbols::code::{self, CodeLanguage, CodeSymbolsParser, Definition};
use anyhow::{Context, anyhow, bail};
use std::iter::Peekable;
use std::str::Chars;
use tree_sitter::Node;

/// Returns a [`LanguageParser`] for Kotlin, with its symbols.
pub(super) fn parser() -> anyhow::Result<impl LanguageParser> {
    Ok(
        LanguageParserImpl::new(comments_parser()?).with_symbols(CodeSymbolsParser::new(
            &tree_sitter_kotlin_ng::LANGUAGE.into(),
            Kotlin,
        )?),
    )
}

/// Kotlin, whose files hold properties, functions and types, and whose types hold their members.
struct Kotlin;

impl CodeLanguage for Kotlin {
    /// The definitions under `root`, the root of a Kotlin file's syntax tree, and the members of
    /// its types, in document order. A type comes before its members. The members of a companion
    /// object are members of the type around it.
    fn definitions<'tree>(&self, root: Node<'tree>, _source: &str) -> Vec<Definition<'tree>> {
        code::nested_definitions(root, push_members)
    }

    /// Decodes a Kotlin string or raw string. Returns `None` for any other node, and for a string
    /// with a template, such as `"hi $name"`.
    ///
    /// # Errors
    /// Returns an error if an escape is not one of Kotlin's, if a string that is not raw holds a
    /// line break, or if the string holds half of a character.
    fn decode_string(&self, node: &Node, source: &str) -> anyhow::Result<Option<String>> {
        let literal = node_text(node, source);
        let (quotes, escapes) = match node.kind() {
            "string_literal" => ("\"", true),
            "multiline_string_literal" => (RAW_QUOTES, false),
            _ => return Ok(None),
        };
        let content = literal
            .strip_prefix(quotes)
            .and_then(|content| content.strip_suffix(quotes))
            .context("a string is between its quotes")?;
        if holds_template(content, escapes) {
            return Ok(None);
        }
        if escapes {
            // The grammar lets a string run on to the next line, so a string missing its closing
            // quote takes in the next line too.
            if content.contains(['\r', '\n']) {
                bail!("Kotlin allows a line break only in a raw string");
            }
            code::decode_utf16(content, push_escape).map(Some)
        } else {
            // The compiler reads every line end of a file as a line feed.
            Ok(Some(content.replace("\r\n", "\n").replace('\r', "\n")))
        }
    }

    const NAME_QUOTE: Option<char> = Some('`');
}

/// The quotes that open and close a raw string.
const RAW_QUOTES: &str = "\"\"\"";

/// Whether `content`, the text of a string between its quotes, holds a template, such as `$name`
/// or `${a + b}`. `escapes` tells whether a backslash starts an escape in it, as it does in a
/// string but not in a raw string.
fn holds_template(content: &str, escapes: bool) -> bool {
    // The grammar misses some templates, such as the `$_id` in `"""$_id"""`, so the text is read
    // instead of the syntax tree.
    let mut characters = content.chars();
    while let Some(character) = characters.next() {
        match character {
            // An escaped `$` starts no template.
            '\\' if escapes => {
                characters.next();
            }
            // A `$` before anything else is text, as in `"cost $5"`.
            '$' if characters
                .clone()
                .next()
                .is_some_and(|next| matches!(next, '{' | '`' | '_') || next.is_alphabetic()) =>
            {
                return true;
            }
            _ => {}
        }
    }
    false
}

/// Reads one escape from `characters`, which start right after its backslash, and adds the UTF-16
/// code units it stands for to `units`.
///
/// # Errors
/// Returns an error if the escape is not one of Kotlin's.
fn push_escape(characters: &mut Peekable<Chars>, units: &mut Vec<u16>) -> anyhow::Result<()> {
    // The grammar puts a character after every backslash in a string.
    let escaped = characters.next().unwrap_or_default();
    let character = match escaped {
        't' => '\t',
        'b' => '\u{8}',
        'n' => '\n',
        'r' => '\r',
        '\'' | '"' | '\\' | '$' => escaped,
        'u' => {
            let digits: String = characters.by_ref().take(4).collect();
            let unit = code::utf16_unit(&digits)
                .ok_or_else(|| anyhow!("`\\u{digits}` is not an escape in a Kotlin string"))?;
            units.push(unit);
            return Ok(());
        }
        _ => bail!("`\\{escaped}` is not an escape in a Kotlin string"),
    };
    units.extend(character.encode_utf16(&mut [0; 2]).iter());
    Ok(())
}

/// Adds the definitions of the members of `body`, the root of a file or the body of a type, to
/// `definitions`. `parents` are the names of the types that `body` belongs to. Adds the body of
/// each type in `body` to `bodies`, with its own parents.
fn push_members<'tree>(
    body: Node<'tree>,
    parents: Vec<Node<'tree>>,
    definitions: &mut Vec<Definition<'tree>>,
    bodies: &mut Vec<(Node<'tree>, Vec<Node<'tree>>)>,
) {
    let mut cursor = body.walk();
    for member in body.named_children(&mut cursor) {
        let name = match member.kind() {
            "class_declaration" | "object_declaration" => {
                push_type(member, &parents, definitions, bodies);
                continue;
            }
            // Code reads a companion's members through the type, as `Config.MAX`, even when the
            // companion has a name.
            "companion_object" => {
                bodies.extend(child(member, "class_body").map(|body| (body, parents.to_vec())));
                continue;
            }
            "property_declaration" => {
                definitions.extend(property(member, &parents));
                continue;
            }
            "function_declaration" => member.child_by_field_name("name"),
            // The grammar calls the name of a type alias its type.
            "type_alias" => member.child_by_field_name("type"),
            // The body of an enum entry is a class without a name, so its members are not
            // members of the enum.
            "enum_entry" => child(member, "identifier"),
            // Code never refers to a constructor or an `init` block by name, and a script's
            // top-level calls only hold local definitions.
            _ => continue,
        };
        definitions.extend(
            name.map(|name| Definition::new(parents.to_vec(), name, member.byte_range(), None)),
        );
    }
}

/// Adds the definition of `declaration`, a class, interface or object in the type at `parents`,
/// and of its constructor properties, to `definitions`. Adds its body to `bodies`.
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
    let parents = [parents, &[name]].concat();
    let parameters = child(declaration, "primary_constructor")
        .and_then(|constructor| child(constructor, "class_parameters"));
    if let Some(parameters) = parameters {
        let mut cursor = parameters.walk();
        // Only `val` and `var` make a parameter a property. A plain one is only seen by the
        // constructor.
        let properties = parameters.named_children(&mut cursor).filter(|parameter| {
            let mut cursor = parameter.walk();
            parameter
                .children(&mut cursor)
                .any(|token| matches!(token.kind(), "val" | "var"))
        });
        for property in properties {
            definitions.extend(child(property, "identifier").map(|name| {
                Definition::new(
                    parents.clone(),
                    name,
                    property.byte_range(),
                    code::value_after(property, "="),
                )
            }));
        }
    }
    // An enum keeps its members in an `enum_class_body`.
    let mut cursor = declaration.walk();
    let body = declaration
        .named_children(&mut cursor)
        .find(|child| matches!(child.kind(), "class_body" | "enum_class_body"));
    bodies.extend(body.map(|body| (body, parents)));
}

/// The definition of `declaration`, a property in the type at `parents`. Its value is the `v` of
/// `val x = v` or of `val x get() = v`. Returns `None` for a declaration with several names, such
/// as `val (a, b) = pair`.
fn property<'tree>(declaration: Node<'tree>, parents: &[Node<'tree>]) -> Option<Definition<'tree>> {
    // Annotations can come before the name, as in `val @Suppress("x") a = 1`.
    let name = child(child(declaration, "variable_declaration")?, "identifier")?;
    // Code reads a property with a getter as it reads one with a value. A getter with a block
    // body computes its value, so it has none.
    let value = code::value_after(declaration, "=").or_else(|| {
        let body = child(child(declaration, "getter")?, "function_body")?;
        code::value_after(body, "=")
    });
    Some(Definition::new(
        parents.to_vec(),
        name,
        declaration.byte_range(),
        value,
    ))
}

/// The first named child of `node` of the `kind`, for the nodes the grammar gives no field name.
fn child<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|child| child.kind() == kind)
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let kotlin_language = tree_sitter_kotlin_ng::LANGUAGE.into();
    let parser =
        c_style_line_and_block_comments_parser(&kotlin_language, "line_comment", "block_comment")?;
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

    /// The symbols of `source`, a Kotlin file.
    fn kotlin(source: &str) -> anyhow::Result<Symbols> {
        CodeSymbolsParser::new(&tree_sitter_kotlin_ng::LANGUAGE.into(), Kotlin)?.parse(source)
    }

    #[test]
    fn file_with_every_shape_parse_symbols_returns_every_symbol_in_order() -> anyhow::Result<()> {
        let source = r#"@file:JvmName("Limits")

package com.example

import kotlin.math.max

/** Doc. */
@JvmField
val SIZES = listOf(1, 2)
const val MAX_RETRIES = 20 * 1024
var counter: Int = (0)
val ready: Boolean
    get() = counter > 0
val total: Int
    get() {
        return counter
    }
val lazyValue by lazy { 3 }

fun top(a: Int): Int = a + 1

fun Int.twice() = this * 2

typealias Handler = (Int) -> Unit

@Deprecated("old")
data class Config(
    @JvmField val retries: Int = 3,
    var limit: Int,
    port: Int,
) {
    val sum = retries + 1
    lateinit var name: String

    constructor() : this(3, 4, 5)

    init {
        counter = port
    }

    fun run() {}

    fun run(times: Int) {}

    fun limit() = limit

    companion object {
        const val ORIGIN = 0

        fun create() = Config()
    }

    object Defaults {
        const val PORT = 80
    }

    inner class Inner {
        val id = 0x10L
    }

    interface Named {
        val label: Int

        fun name(): Int
    }
}

enum class Mode(val level: Int) {
    RED(1),
    @Deprecated("old")
    GREEN(2) {
        override fun paint() = 1
    };

    open fun paint() = 0
}

class Factory {
    companion object Builder {
        val DEFAULT = 1
    }
}

annotation class Tag(val value: Int)

sealed interface Shape

object Registry {
    const val SIZE = 8
}
"#;
        // The text from the first `start` up to the first `end` after it.
        let text_of = |start: &str, end: &str| -> anyhow::Result<&str> {
            let start = source.find(start).context("no start")?;
            let end = start + source[start..].find(end).context("no end")?;
            Ok(&source[start..end])
        };

        // <block affects="docs/symbols.md:kotlin-paths, src/skill.md#/blockwatch/Symbols/Kotlin">
        assert_eq!(
            symbols(&kotlin(source)?, source),
            vec![
                symbol(
                    "/SIZES",
                    "@JvmField\nval SIZES = listOf(1, 2)",
                    Some("listOf(1, 2)")
                ),
                symbol(
                    "/MAX_RETRIES",
                    "const val MAX_RETRIES = 20 * 1024",
                    Some("20 * 1024")
                ),
                symbol("/counter", "var counter: Int = (0)", Some("0")),
                symbol(
                    "/ready",
                    "val ready: Boolean\n    get() = counter > 0",
                    Some("counter > 0")
                ),
                symbol(
                    "/total",
                    "val total: Int\n    get() {\n        return counter\n    }",
                    None
                ),
                symbol("/lazyValue", "val lazyValue by lazy { 3 }", None),
                symbol("/top", "fun top(a: Int): Int = a + 1", None),
                symbol("/twice", "fun Int.twice() = this * 2", None),
                symbol("/Handler", "typealias Handler = (Int) -> Unit", None),
                symbol("/Config", text_of("@Deprecated", "\n\nenum")?, None),
                symbol(
                    "/Config/retries",
                    "@JvmField val retries: Int = 3",
                    Some("3")
                ),
                symbol("/Config/limit", "var limit: Int", None),
                symbol("/Config/sum", "val sum = retries + 1", Some("retries + 1")),
                symbol("/Config/name", "lateinit var name: String", None),
                symbol("/Config/run", "fun run() {}", None),
                symbol("/Config/run", "fun run(times: Int) {}", None),
                symbol("/Config/limit", "fun limit() = limit", None),
                symbol("/Config/ORIGIN", "const val ORIGIN = 0", Some("0")),
                symbol("/Config/create", "fun create() = Config()", None),
                symbol(
                    "/Config/Defaults",
                    "object Defaults {\n        const val PORT = 80\n    }",
                    None
                ),
                symbol("/Config/Defaults/PORT", "const val PORT = 80", Some("80")),
                symbol(
                    "/Config/Inner",
                    "inner class Inner {\n        val id = 0x10L\n    }",
                    None
                ),
                symbol("/Config/Inner/id", "val id = 0x10L", Some("0x10L")),
                symbol(
                    "/Config/Named",
                    "interface Named {\n        val label: Int\n\n        fun name(): Int\n    }",
                    None
                ),
                symbol("/Config/Named/label", "val label: Int", None),
                symbol("/Config/Named/name", "fun name(): Int", None),
                symbol("/Mode", text_of("enum class", "\n\nclass Factory")?, None),
                symbol("/Mode/level", "val level: Int", None),
                symbol("/Mode/RED", "RED(1)", None),
                symbol(
                    "/Mode/GREEN",
                    "@Deprecated(\"old\")\n    GREEN(2) {\n        override fun paint() = 1\n    }",
                    None
                ),
                symbol("/Mode/paint", "open fun paint() = 0", None),
                symbol(
                    "/Factory",
                    "class Factory {\n    companion object Builder {\n        val DEFAULT = 1\n    }\n}",
                    None
                ),
                symbol("/Factory/DEFAULT", "val DEFAULT = 1", Some("1")),
                symbol("/Tag", "annotation class Tag(val value: Int)", None),
                symbol("/Tag/value", "val value: Int", None),
                symbol("/Shape", "sealed interface Shape", None),
                symbol(
                    "/Registry",
                    "object Registry {\n    const val SIZE = 8\n}",
                    None
                ),
                symbol("/Registry/SIZE", "const val SIZE = 8", Some("8")),
            ]
        );
        // </block>
        Ok(())
    }

    #[test]
    fn script_parse_symbols_returns_its_top_level_definitions() -> anyhow::Result<()> {
        // A `.kts` script runs its top-level calls, so their blocks hold no symbols.
        let source = r#"import java.util.Locale

plugins {
    id("base")
}

val javaVersion = 17

fun shout(text: String) = text.uppercase(Locale.ROOT)

tasks.register("hello") {
    val local = 1
}

class Helper
"#;

        assert_eq!(
            symbols(&kotlin(source)?, source),
            vec![
                symbol("/javaVersion", "val javaVersion = 17", Some("17")),
                symbol(
                    "/shout",
                    "fun shout(text: String) = text.uppercase(Locale.ROOT)",
                    None
                ),
                symbol("/Helper", "class Helper", None),
            ]
        );
        Ok(())
    }

    #[test]
    fn names_in_backticks_parse_symbols_returns_them_without_the_backticks() -> anyhow::Result<()> {
        let source = r#"class `Config` {
    fun `parses empty input`() {}

    val `is` = 1
}
"#;

        assert_eq!(
            symbols(&kotlin(source)?, source),
            vec![
                symbol("/Config", source.trim_end(), None),
                symbol(
                    "/Config/parses empty input",
                    "fun `parses empty input`() {}",
                    None
                ),
                symbol("/Config/is", "val `is` = 1", Some("1")),
            ]
        );
        Ok(())
    }

    #[test]
    fn string_kinds_parse_symbols_returns_decoded_values() -> anyhow::Result<()> {
        let source = r#"val ESCAPES = "a\t\b\n\r\'\"\\\$"
val UNICODE = "caf\u00e9 \uD83D\uDE00"
val NOT_UNICODE = "\\u0041"
val ESCAPED_TEMPLATE = "\$name"
val DOLLARS = "cost $5, $ and $"
val TEMPLATE = "hi $name"
val UNDERSCORE_TEMPLATE = "id $_id"
val BRACED_TEMPLATE = "sum ${a + b}"
val QUOTED_TEMPLATE = "$`is`"
val RAW = """C:\new $5"""
val RAW_LINES = """
    a
      b
"""
val RAW_TEMPLATE = """${'$'}x"""
val CHAR = 'a'
val JOINED = "a" + "b"
val TRIMMED = """a""".trimIndent()
"#;

        assert_eq!(
            values(&kotlin(source)?, source),
            [
                value("/ESCAPES", "a\t\u{8}\n\r'\"\\$"),
                value("/UNICODE", "café 😀"),
                // The first backslash escapes the second, so no `\u` escape starts.
                value("/NOT_UNICODE", r"\u0041"),
                value("/ESCAPED_TEMPLATE", "$name"),
                // A `$` that no name or `{` follows is text.
                value("/DOLLARS", "cost $5, $ and $"),
                value("/TEMPLATE", r#""hi $name""#),
                value("/UNDERSCORE_TEMPLATE", r#""id $_id""#),
                value("/BRACED_TEMPLATE", r#""sum ${a + b}""#),
                value("/QUOTED_TEMPLATE", r#""$`is`""#),
                // A raw string has no escapes.
                value("/RAW", r"C:\new $5"),
                value("/RAW_LINES", "\n    a\n      b\n"),
                value("/RAW_TEMPLATE", r#""""${'$'}x""""#),
                value("/CHAR", "'a'"),
                value("/JOINED", r#""a" + "b""#),
                value("/TRIMMED", r#""""a""".trimIndent()"#),
            ]
        );
        Ok(())
    }

    #[test]
    fn cr_and_crlf_line_endings_parse_symbols_reads_line_feeds() -> anyhow::Result<()> {
        // The compiler reads every line end of a file as a line feed.
        let source = "val RAW = \"\"\"a\r\nb\rc\"\"\"\r\n";

        assert_eq!(values(&kotlin(source)?, source), [value("/RAW", "a\nb\nc")]);
        Ok(())
    }

    #[test]
    fn string_that_kotlin_rejects_parse_symbols_makes_the_name_unreadable() -> anyhow::Result<()> {
        let source = r#"val UNKNOWN = "\q"
val SHORT = "\u12"
val HALF = "\uD83D"
val TEMPLATE = "\q $x"
val SIGN = "\u+041"
val FINE = "ok"
"#;

        assert_eq!(
            values(&kotlin(source)?, source),
            [
                // A string with a template is kept as written, so its escapes are not read.
                value("/TEMPLATE", r#""\q $x""#),
                value("/FINE", "ok"),
            ]
        );
        assert_eq!(
            unresolved(&kotlin(source)?, source, "/UNKNOWN")?,
            decode_error(1, 15, r"`\q` is not an escape in a Kotlin string")
        );
        assert_eq!(
            unresolved(&kotlin(source)?, source, "/SHORT")?,
            decode_error(2, 13, r"`\u12` is not an escape in a Kotlin string")
        );
        assert_eq!(
            unresolved(&kotlin(source)?, source, "/HALF")?,
            decode_error(
                3,
                12,
                "the string holds U+D83D, which is only half of a character"
            )
        );
        assert_eq!(
            unresolved(&kotlin(source)?, source, "/SIGN")?,
            decode_error(5, 12, r"`\u+041` is not an escape in a Kotlin string")
        );
        Ok(())
    }

    #[test]
    fn line_break_in_a_string_parse_symbols_makes_the_name_unreadable() -> anyhow::Result<()> {
        // Kotlin reads a carriage return alone as a line end too.
        let source = "val LF = \"a\nb\"\nval CR = \"a\rb\"\nval TEMPLATE = \"${\n1}\"\n";

        assert_eq!(
            values(&kotlin(source)?, source),
            [value("/TEMPLATE", "\"${\n1}\"")]
        );
        assert_eq!(
            unresolved(&kotlin(source)?, source, "/LF")?,
            decode_error(1, 10, "Kotlin allows a line break only in a raw string")
        );
        assert_eq!(
            unresolved(&kotlin(source)?, source, "/CR")?,
            decode_error(3, 10, "Kotlin allows a line break only in a raw string")
        );
        Ok(())
    }

    #[test]
    fn deeply_nested_classes_parse_symbols_does_not_overflow_stack() -> anyhow::Result<()> {
        // Each class has a path one segment longer than the class around it, so the paths grow
        // with the square of the depth. This depth is enough to overflow a recursive walk.
        let depth = 3_000;
        let source = format!("{}{}", "class C {\n".repeat(depth), "}\n".repeat(depth));

        let symbols = kotlin(&source)?;

        assert_eq!(symbols.symbols.len(), depth);
        Ok(())
    }

    #[test]
    fn syntax_error_parse_symbols_makes_the_definitions_that_hold_it_unreadable()
    -> anyhow::Result<()> {
        // The parser skips `= )`. A second error in the file would make it skip the whole file.
        let source = r#"class A {
    val x = 1
    fun broken() { val q = ) }
    val y = 2
}

val after = 5
"#;

        assert_eq!(
            symbols(&kotlin(source)?, source),
            vec![
                symbol("/A/x", "val x = 1", Some("1")),
                symbol("/A/y", "val y = 2", Some("2")),
                symbol("/after", "val after = 5", Some("5")),
            ]
        );
        assert_eq!(
            unresolved(&kotlin(source)?, source, "/A")?,
            syntax_error(3, 26, "A")
        );
        assert_eq!(
            unresolved(&kotlin(source)?, source, "/A/broken")?,
            syntax_error(3, 26, "broken")
        );
        Ok(())
    }

    #[test]
    fn span_that_swallows_a_definition_parse_symbols_makes_its_name_unreadable()
    -> anyhow::Result<()> {
        // The unclosed `(` makes the parser skip `= (3` and `val w`, up to the `=` of `w`.
        let source = r#"class B {
    val z = (3
    val w = 4
}

val after = 5
"#;

        assert_eq!(
            symbols(&kotlin(source)?, source),
            vec![symbol("/after", "val after = 5", Some("5"))]
        );
        assert_eq!(
            unresolved(&kotlin(source)?, source, "/B/z")?,
            syntax_error(2, 11, "z")
        );
        assert_eq!(
            unresolved(&kotlin(source)?, source, "/B/w")?,
            syntax_error(2, 11, "w")
        );
        Ok(())
    }

    #[test]
    fn name_in_backticks_in_a_skipped_span_parse_symbols_makes_it_unreadable() -> anyhow::Result<()>
    {
        // The unclosed `(` makes the parser skip the function.
        let source = r#"class B {
    val z = (3
    fun `has spaces`() = 1
}
"#;

        assert_eq!(
            unresolved(&kotlin(source)?, source, "/B/has spaces")?,
            syntax_error(2, 11, "has spaces")
        );
        Ok(())
    }

    #[test]
    fn annotation_before_property_name_parse_symbols_returns_the_name() -> anyhow::Result<()> {
        let source = r#"val @Suppress("x") annotated = 1
"#;

        assert_eq!(
            symbols(&kotlin(source)?, source),
            vec![symbol(
                "/annotated",
                r#"val @Suppress("x") annotated = 1"#,
                Some("1")
            )]
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Position, language_parsers::Comment};

    #[test]
    fn parses_kotlin_comments_correctly() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser()?;

        let blocks: Vec<Comment> = comments_parser
            .parse(
                r#"
            // This is a single-line comment in Kotlin
            fun main() {
                
            /*
             * This is a multi-line comment.
             * It spans multiple lines in Kotlin.
             */
            
                println("Hello, Kotlin!") // Prints a message
                
                /* Another comment
                 * split into
                 * multiple lines
                 */
                 
                return
            }
/// Triple slash comment.
            "#,
            )
            .collect();

        assert_eq!(
            blocks,
            vec![
                Comment {
                    position_range: Position::new(2, 13)..Position::new(2, 55),
                    source_range: 13..55,
                    comment_text: "   This is a single-line comment in Kotlin".to_string()
                },
                Comment {
                    position_range: Position::new(5, 13)..Position::new(8, 16),
                    source_range: 110..223,
                    comment_text:
                        "  \n               This is a multi-line comment.\n               It spans multiple lines in Kotlin.\n               "
                            .to_string()
                },
                Comment {
                    position_range: Position::new(10, 43)..Position::new(10, 62),
                    source_range: 279..298,
                    comment_text: "   Prints a message".to_string()
                },
                Comment {
                    position_range: Position::new(12, 17)..Position::new(15, 20),
                    source_range: 332..434,
                    comment_text: "   Another comment\n                   split into\n                   multiple lines\n                   ".to_string()
                },
                Comment {
                    position_range: Position::new(19, 1)..Position::new(19, 26),
                    source_range: 490..515,
                    comment_text: "  / Triple slash comment.".to_string()
                },
            ]
        );

        Ok(())
    }
}
