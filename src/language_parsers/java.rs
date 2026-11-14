use crate::language_parsers::{
    CommentsParser, LanguageParser, LanguageParserImpl, SLASH_MARKERS, c_style_comments_parser,
    node_text,
};
use crate::symbols::code::{self, CodeLanguage, CodeSymbolsParser, Definition, NestedWalk};
use anyhow::{anyhow, bail};
use std::iter::Peekable;
use std::str::Chars;
use tree_sitter::Node;

/// Returns a [`LanguageParser`] for Java, with its symbols.
pub(super) fn parser() -> impl LanguageParser {
    LanguageParserImpl::new(comments_parser()).with_symbols(CodeSymbolsParser::new(
        &tree_sitter_java::LANGUAGE.into(),
        Java,
    ))
}

/// Java, whose types hold its fields, methods and nested types.
struct Java;

impl CodeLanguage for Java {
    /// The types under `root`, the root of a Java file's syntax tree, and their members, in
    /// document order. A type comes before its members.
    fn definitions<'tree>(&self, root: Node<'tree>, _source: &str) -> Vec<Definition<'tree>> {
        // A compact source file holds methods and fields at its top level, as a class body does.
        code::nested_definitions(root, push_members)
    }

    /// Decodes a Java string or text block. Returns `None` for any other node.
    ///
    /// # Errors
    /// Returns an error if an escape is not one of Java's, if the string holds `\u005c`, if a
    /// string that is not a text block holds a line break, or if the string holds half of a
    /// character.
    fn decode_string(&self, node: &Node, source: &str) -> anyhow::Result<Option<String>> {
        if node.kind() != "string_literal" {
            return Ok(None);
        }
        let literal = node_text(node, source);
        let content = if literal.starts_with(TEXT_BLOCK_QUOTES) {
            text_block_content(code::between_quotes(literal, TEXT_BLOCK_QUOTES)?)
        } else {
            let content = code::between_quotes(literal, "\"")?;
            // The grammar lets a string run on to the next line, so a string missing its closing
            // quote takes in the next line too.
            if content.contains(['\r', '\n']) {
                bail!("Java allows a line break only in a text block");
            }
            content.to_string()
        };
        code::decode_utf16(&content, push_escape).map(Some)
    }
}

/// The quotes that open and close a text block.
const TEXT_BLOCK_QUOTES: &str = "\"\"\"";

/// The text of a text block whose content between its quotes is `block`, with its escapes still
/// as written. Each line end becomes a line feed. The first line, which holds only white space
/// after the opening quotes, is left out. The other lines lose the indentation they share, and the
/// white space at their ends.
fn text_block_content(block: &str) -> String {
    let block = code::with_line_feeds(block);
    let content = block
        .split_once('\n')
        .map_or(block.as_str(), |(_, content)| content);
    let lines: Vec<&str> = content.split('\n').collect();
    // The line of the closing quotes counts even when it holds only white space, so the closing
    // quotes can set the indentation.
    let indent = lines
        .iter()
        .filter(|line| !line.chars().all(is_white_space))
        .chain(lines.last())
        .map(|line| line.chars().take_while(|&c| is_white_space(c)).count())
        .min()
        .unwrap_or_default();
    let lines: Vec<String> = lines
        .iter()
        .map(|line| {
            let line: String = line.chars().skip(indent).collect();
            line.trim_end_matches(is_white_space).to_string()
        })
        .collect();
    lines.join("\n")
}

/// Whether a line of a text block loses `character` from its indentation and from its end. Java
/// also counts rarer spaces, such as U+001F and U+3000, but code doesn't indent with them or end
/// a line with them.
fn is_white_space(character: char) -> bool {
    matches!(character, ' ' | '\t' | '\u{c}')
}

/// Reads one escape from `characters`, which start right after its backslash, and adds the UTF-16
/// code units it stands for to `units`.
///
/// # Errors
/// Returns an error if the escape is not one of Java's, or if it is `\u005c`.
fn push_escape(characters: &mut Peekable<Chars>, units: &mut Vec<u16>) -> anyhow::Result<()> {
    // The grammar puts a character after every backslash in a string.
    let escaped = characters.next().unwrap_or_default();
    let character = match escaped {
        'b' => '\u{8}',
        's' => ' ',
        't' => '\t',
        'n' => '\n',
        'f' => '\u{c}',
        'r' => '\r',
        '"' | '\'' | '\\' => escaped,
        // A backslash at the end of a line in a text block joins the next line to it.
        '\n' => return Ok(()),
        '0'..='7' => char::from(u8::from_str_radix(&octal_digits(escaped, characters), 8)?),
        'u' => {
            units.push(unicode_unit(characters)?);
            return Ok(());
        }
        _ => bail!("`\\{escaped}` is not an escape in a Java string"),
    };
    code::push_utf16(character, units);
    Ok(())
}

/// The digits of the octal escape whose first digit is `first`, such as `377`. Takes the digits
/// after the first from `characters`.
fn octal_digits(first: char, characters: &mut Peekable<Chars>) -> String {
    // An octal escape ends at `\377`, so only a first digit up to 3 takes two more.
    let longest = if first <= '3' { 3 } else { 2 };
    let mut digits = String::from(first);
    while digits.len() < longest
        && let Some(digit) = characters.next_if(|c| c.is_digit(8))
    {
        digits.push(digit);
    }
    digits
}

/// The UTF-16 code unit of a `\u` escape, read from `characters`, which start right after its
/// `u`. For `\u00e9`, it is the unit of `é`.
///
/// # Errors
/// Returns an error if the next 4 characters are not hex digits, or if the escape is `\u005c`.
fn unicode_unit(characters: &mut Peekable<Chars>) -> anyhow::Result<u16> {
    let digits: String = characters.by_ref().take(4).collect();
    let unit = code::utf16_unit(&digits)
        .ok_or_else(|| anyhow!("`\\u{digits}` is not an escape in a Java string"))?;
    // Java turns `\u005c` into a backslash before it reads the other escapes, so `\u005cn` is
    // a line feed, which one pass over the escapes can't give.
    if unit == u16::from(b'\\') {
        bail!("Java turns `\\u{digits}` into a backslash before it reads the other escapes");
    }
    Ok(unit)
}

/// Adds the definitions of the members of `body`, the root of a file or the body of a type, to
/// `walk`. `parents` are the names of the types that `body` belongs to. Adds the body of each type
/// in `body` to `walk`, with its own parents.
fn push_members<'tree>(body: Node<'tree>, parents: Vec<Node<'tree>>, walk: &mut NestedWalk<'tree>) {
    let mut cursor = body.walk();
    for member in body.named_children(&mut cursor) {
        let name = member.child_by_field_name("name");
        match member.kind() {
            // A compact source file declares its fields as local variables, and an interface its
            // constants as constants.
            "field_declaration" | "local_variable_declaration" | "constant_declaration" => {
                push_fields(member, &parents, walk);
            }
            // The members of an enum that come after its constants.
            "enum_body_declarations" => walk.add_body(member, parents.clone()),
            "class_declaration"
            | "interface_declaration"
            | "enum_declaration"
            | "record_declaration"
            | "annotation_type_declaration" => {
                if let Some(name) = name {
                    let body = member.child_by_field_name("body");
                    walk.add_definition_with_body(&parents, name, member.byte_range(), None, body);
                }
            }
            // The body of an enum constant is a class without a name, so its members are not
            // members of the enum.
            "method_declaration" | "enum_constant" => {
                if let Some(name) = name {
                    walk.add_definition(&parents, name, member.byte_range(), None);
                }
            }
            "annotation_type_element_declaration" => {
                if let Some(name) = name {
                    let default = member.child_by_field_name("value");
                    walk.add_definition(&parents, name, member.byte_range(), default);
                }
            }
            // Code never refers to a constructor or an initializer block by name.
            _ => {}
        }
    }
}

/// Adds a definition to `walk` for each name that `declaration`, a field declaration in the type
/// at `parents`, declares.
fn push_fields<'tree>(
    declaration: Node<'tree>,
    parents: &[Node<'tree>],
    walk: &mut NestedWalk<'tree>,
) {
    let mut cursor = declaration.walk();
    let declarators: Vec<Node> = declaration
        .children_by_field_name("declarator", &mut cursor)
        .collect();
    for (declarator, range) in code::part_ranges(declaration.byte_range(), declarators) {
        let Some(name) = declarator.child_by_field_name("name") else {
            continue;
        };
        walk.add_definition(
            parents,
            name,
            range,
            declarator.child_by_field_name("value"),
        );
    }
}

fn comments_parser() -> impl CommentsParser {
    let java_language = tree_sitter_java::LANGUAGE.into();
    // A text block (`"""..."""`) is a `string_literal`, and tree-sitter can nest a comment-shaped
    // node inside it. Its text is string content, not a source comment.
    c_style_comments_parser(
        &java_language,
        &["line_comment", "block_comment"],
        SLASH_MARKERS,
    )
    .with_break_at_node_kinds(&["string_literal"])
}

#[cfg(test)]
mod symbols_tests {
    use super::*;
    use crate::symbols::code::test_utils::{
        decode_error, symbol, symbols, syntax_error, text_of, unresolved, value, values,
    };
    use crate::symbols::{Symbols, SymbolsParser};

    /// The symbols of `source`, a Java file.
    fn java(source: &str) -> anyhow::Result<Symbols> {
        CodeSymbolsParser::new(&tree_sitter_java::LANGUAGE.into(), Java).parse(source)
    }

    #[test]
    fn file_with_every_shape_parse_symbols_returns_every_symbol_in_order() -> anyhow::Result<()> {
        let source = r#"package com.example;

import java.util.List;

/** Doc. */
@Deprecated
public final class Config {
    /** Doc. */
    @Native
    public static final int MAX_RETRIES = 20 * 1024;
    static final int LOW = 1, HIGH = (2);
    int[] sizes = {1, 2};
    boolean ready;
    Runnable task = new Runnable() {
        public void run() {}
    };

    Config() {}

    static {}

    {}

    @Override
    public int hashCode() {
        return 1;
    }

    void run(int times) {}

    void run() {}

    boolean ready() {
        return ready;
    }

    <T> T first(List<T> items) {
        class Local {}
        return items.get(0);
    }

    enum Mode {
        RED,
        @Deprecated
        GREEN(2) {
            void paint() {}
        };

        static final int COUNT = 2;
    }

    interface Named {
        int LIMIT = 8;

        String name();

        default String label() {
            return name();
        }
    }

    @interface Tag {
        int value() default 1;

        int ORDER = 3;
    }

    record Point(int x, int y) {
        Point {}

        static final int ORIGIN = 0;

        public int x() {
            return x;
        }
    }

    static class Inner {
        static final long ID = 0x10L;

        class Deeper {}
    }
}

interface Service {}

enum Level { LOW }

record Pair<A, B>(A first, B second) {}

@interface Marker {}
"#;
        let config = text_of(source, "@Deprecated", "\n}")?;

        // <block affects="docs/symbols.md:java-paths">
        assert_eq!(
            symbols(&java(source)?, source),
            vec![
                symbol("/Config", config, None),
                symbol(
                    "/Config/MAX_RETRIES",
                    "@Native\n    public static final int MAX_RETRIES = 20 * 1024;",
                    Some("20 * 1024")
                ),
                symbol("/Config/LOW", "LOW = 1", Some("1")),
                symbol("/Config/HIGH", "HIGH = (2)", Some("2")),
                symbol("/Config/sizes", "int[] sizes = {1, 2};", Some("{1, 2}")),
                symbol("/Config/ready", "boolean ready;", None),
                symbol(
                    "/Config/task",
                    "Runnable task = new Runnable() {\n        public void run() {}\n    };",
                    Some("new Runnable() {\n        public void run() {}\n    }")
                ),
                symbol(
                    "/Config/hashCode",
                    "@Override\n    public int hashCode() {\n        return 1;\n    }",
                    None
                ),
                symbol("/Config/run", "void run(int times) {}", None),
                symbol("/Config/run", "void run() {}", None),
                symbol(
                    "/Config/ready",
                    "boolean ready() {\n        return ready;\n    }",
                    None
                ),
                symbol(
                    "/Config/first",
                    "<T> T first(List<T> items) {\n        class Local {}\n        return items.get(0);\n    }",
                    None
                ),
                symbol(
                    "/Config/Mode",
                    "enum Mode {\n        RED,\n        @Deprecated\n        GREEN(2) {\n            void paint() {}\n        };\n\n        static final int COUNT = 2;\n    }",
                    None
                ),
                symbol("/Config/Mode/RED", "RED", None),
                symbol(
                    "/Config/Mode/GREEN",
                    "@Deprecated\n        GREEN(2) {\n            void paint() {}\n        }",
                    None
                ),
                symbol(
                    "/Config/Mode/COUNT",
                    "static final int COUNT = 2;",
                    Some("2")
                ),
                symbol(
                    "/Config/Named",
                    "interface Named {\n        int LIMIT = 8;\n\n        String name();\n\n        default String label() {\n            return name();\n        }\n    }",
                    None
                ),
                symbol("/Config/Named/LIMIT", "int LIMIT = 8;", Some("8")),
                symbol("/Config/Named/name", "String name();", None),
                symbol(
                    "/Config/Named/label",
                    "default String label() {\n            return name();\n        }",
                    None
                ),
                symbol(
                    "/Config/Tag",
                    "@interface Tag {\n        int value() default 1;\n\n        int ORDER = 3;\n    }",
                    None
                ),
                symbol("/Config/Tag/value", "int value() default 1;", Some("1")),
                symbol("/Config/Tag/ORDER", "int ORDER = 3;", Some("3")),
                symbol(
                    "/Config/Point",
                    "record Point(int x, int y) {\n        Point {}\n\n        static final int ORIGIN = 0;\n\n        public int x() {\n            return x;\n        }\n    }",
                    None
                ),
                symbol(
                    "/Config/Point/ORIGIN",
                    "static final int ORIGIN = 0;",
                    Some("0")
                ),
                symbol(
                    "/Config/Point/x",
                    "public int x() {\n            return x;\n        }",
                    None
                ),
                symbol(
                    "/Config/Inner",
                    "static class Inner {\n        static final long ID = 0x10L;\n\n        class Deeper {}\n    }",
                    None
                ),
                symbol(
                    "/Config/Inner/ID",
                    "static final long ID = 0x10L;",
                    Some("0x10L")
                ),
                symbol("/Config/Inner/Deeper", "class Deeper {}", None),
                symbol("/Service", "interface Service {}", None),
                symbol("/Level", "enum Level { LOW }", None),
                symbol("/Level/LOW", "LOW", None),
                symbol("/Pair", "record Pair<A, B>(A first, B second) {}", None),
                symbol("/Marker", "@interface Marker {}", None),
            ]
        );
        // </block>
        Ok(())
    }

    #[test]
    fn compact_source_file_parse_symbols_returns_its_top_level_members() -> anyhow::Result<()> {
        // A file with methods and fields at its top level declares a class without a name.
        let source = r#"static final int LIMIT = 3;
int count;

void main() {
    int local = LIMIT;
}

class Helper {}
"#;

        assert_eq!(
            symbols(&java(source)?, source),
            vec![
                symbol("/LIMIT", "static final int LIMIT = 3;", Some("3")),
                symbol("/count", "int count;", None),
                symbol("/main", "void main() {\n    int local = LIMIT;\n}", None),
                symbol("/Helper", "class Helper {}", None),
            ]
        );
        Ok(())
    }

    #[test]
    fn string_kinds_parse_symbols_returns_decoded_values() -> anyhow::Result<()> {
        let source = r#"class Strings {
    static final String ESCAPES = "a\b\s\t\n\f\r\"\'\\";
    static final String OCTAL = "\0\7\77\377\400";
    static final String UNICODE = "caf\u00e9 \uD83D\uDE00";
    static final String NOT_UNICODE = "\\u0041";
    static final String TEMPLATE = STR."Hi \{name}";
    static final char CHAR = 'a';
    static final String JOINED = "a" + "b";
}
"#;

        assert_eq!(
            values(&java(source)?, source)[1..],
            [
                value("/Strings/ESCAPES", "a\u{8} \t\n\u{c}\r\"'\\"),
                // An octal escape ends at `\377`, so `\400` is `\40` and then `0`.
                value("/Strings/OCTAL", "\0\u{7}?\u{ff} 0"),
                value("/Strings/UNICODE", "café 😀"),
                // The first backslash escapes the second, so no `\u` escape starts.
                value("/Strings/NOT_UNICODE", r"\u0041"),
                value("/Strings/TEMPLATE", r#"STR."Hi \{name}""#),
                value("/Strings/CHAR", "'a'"),
                value("/Strings/JOINED", r#""a" + "b""#),
            ]
        );
        Ok(())
    }

    #[test]
    fn text_blocks_parse_symbols_removes_the_indentation_their_lines_share() -> anyhow::Result<()> {
        let source = r#"class Blocks {
    static final String QUERY = """
        SELECT *
          FROM t
        WHERE x = 1\s
        """;
    static final String CLOSED_ON_LAST_LINE = """
        a
          b   """;
    static final String CLOSING_LEFT = """
            a
        """;
    static final String BLANK_LINE = """
        a

        b
        """;
    static final String JOINED = """
        one \
        two
        """;
    static final String ESCAPED_INDENT = """
        \ta
        caf\u00e9
        """;
}
"#;

        assert_eq!(
            values(&java(source)?, source)[1..],
            [
                // The closing quotes set the indentation. The `\s` escape keeps the space before
                // it.
                value("/Blocks/QUERY", "SELECT *\n  FROM t\nWHERE x = 1 \n"),
                value("/Blocks/CLOSED_ON_LAST_LINE", "a\n  b"),
                value("/Blocks/CLOSING_LEFT", "    a\n"),
                value("/Blocks/BLANK_LINE", "a\n\nb\n"),
                value("/Blocks/JOINED", "one two\n"),
                // Escapes are read after the indentation is removed.
                value("/Blocks/ESCAPED_INDENT", "\ta\ncafé\n"),
            ]
        );
        Ok(())
    }

    #[test]
    fn cr_and_crlf_line_endings_parse_symbols_reads_line_feeds() -> anyhow::Result<()> {
        // Java reads a CR without an LF after it as a line end too.
        let source = "class Lines {\r\n    static final String BLOCK = \"\"\"\r\n        a\r        b \\\r\n        c\r\n        \"\"\";\r\n}\r\n";

        assert_eq!(
            values(&java(source)?, source)[1..],
            [value("/Lines/BLOCK", "a\nb c\n")]
        );
        Ok(())
    }

    #[test]
    fn tabs_and_form_feeds_in_text_block_parse_symbols_counts_them_as_white_space()
    -> anyhow::Result<()> {
        let source = "class Tabs {\n\tstatic final String BLOCK = \"\"\"\n\t\ta\t\u{c}\n\t\t\tb\n\t\t\"\"\";\n}\n";

        assert_eq!(
            values(&java(source)?, source)[1..],
            [value("/Tabs/BLOCK", "a\n\tb\n")]
        );
        Ok(())
    }

    #[test]
    fn escape_that_is_not_text_parse_symbols_makes_the_name_unreadable() -> anyhow::Result<()> {
        let source = r#"class Broken {
    static final String HEX = "\x41";
    static final String HALF = "\uD800";
    static final String BACKSLASH = "\u005cn";
    static final String SIGN = """
        \u+041""";
    static final String FINE = "ok";
}
"#;

        assert_eq!(
            values(&java(source)?, source)[1..],
            [value("/Broken/FINE", "ok")]
        );
        assert_eq!(
            unresolved(&java(source)?, source, "/Broken/HEX")?,
            decode_error(2, 31, r"`\x` is not an escape in a Java string")
        );
        assert_eq!(
            unresolved(&java(source)?, source, "/Broken/HALF")?,
            decode_error(
                3,
                32,
                "the string holds U+D800, which is only half of a character"
            )
        );
        assert_eq!(
            unresolved(&java(source)?, source, "/Broken/BACKSLASH")?,
            decode_error(
                4,
                37,
                r"Java turns `\u005c` into a backslash before it reads the other escapes"
            )
        );
        // The grammar rejects this escape in a string, but not in a text block.
        assert_eq!(
            unresolved(&java(source)?, source, "/Broken/SIGN")?,
            decode_error(5, 32, r"`\u+041` is not an escape in a Java string")
        );
        Ok(())
    }

    #[test]
    fn line_break_in_a_string_parse_symbols_makes_the_name_unreadable() -> anyhow::Result<()> {
        // Java reads a carriage return alone as a line end too.
        let source = "class A {\n    static final String LF = \"a\nb\";\n    static final String CR = \"a\rb\";\n}\n";

        assert_eq!(values(&java(source)?, source).len(), 1, "only the class");
        assert_eq!(
            unresolved(&java(source)?, source, "/A/LF")?,
            decode_error(2, 30, "Java allows a line break only in a text block")
        );
        assert_eq!(
            unresolved(&java(source)?, source, "/A/CR")?,
            decode_error(4, 30, "Java allows a line break only in a text block")
        );
        Ok(())
    }

    #[test]
    fn deeply_nested_classes_parse_symbols_does_not_overflow_stack() -> anyhow::Result<()> {
        // Each class has a path one segment longer than the class around it, so the paths grow
        // with the square of the depth. This depth is enough to overflow a recursive walk.
        let depth = 3_000;
        let source = format!("{}{}", "class C { ".repeat(depth), "}".repeat(depth));

        let symbols = java(&source)?;

        assert_eq!(symbols.symbols.len(), depth);
        Ok(())
    }

    #[test]
    fn syntax_errors_parse_symbols_makes_the_definitions_that_hold_them_unreadable()
    -> anyhow::Result<()> {
        // The parser assumes a `)` after `broken(`, and skips the second `=` of `Z`.
        let source = r#"class A {
    static final int X = 1;
    void broken( {
        return;
    }
    static final int Y = 2;
}

class B {
    int Z = = 3;
    int W = 4;
}
"#;

        assert_eq!(
            symbols(&java(source)?, source),
            vec![
                symbol("/A/X", "static final int X = 1;", Some("1")),
                symbol("/A/Y", "static final int Y = 2;", Some("2")),
                symbol("/B/W", "int W = 4;", Some("4")),
            ]
        );
        assert_eq!(
            unresolved(&java(source)?, source, "/A")?,
            syntax_error(3, 17, "A")
        );
        assert_eq!(
            unresolved(&java(source)?, source, "/A/broken")?,
            syntax_error(3, 17, "broken")
        );
        assert_eq!(
            unresolved(&java(source)?, source, "/B")?,
            syntax_error(10, 13, "B")
        );
        assert_eq!(
            unresolved(&java(source)?, source, "/B/Z")?,
            syntax_error(10, 13, "Z")
        );
        Ok(())
    }

    #[test]
    fn span_that_swallows_definitions_parse_symbols_makes_their_names_unreadable()
    -> anyhow::Result<()> {
        // The unclosed `Config(` makes the parser skip up to the `;` after it, `Y` included. The
        // skipped text could also hold a second class `Config`.
        let source = r#"class A {
    static final int X = 1;
    Config(
    static final int Y = 2;
    class Inner { int Q = 5; }
}

class Config { int W = 4; }
"#;

        assert_eq!(
            symbols(&java(source)?, source),
            vec![
                symbol("/A/X", "static final int X = 1;", Some("1")),
                symbol("/A/Inner", "class Inner { int Q = 5; }", None),
                symbol("/A/Inner/Q", "int Q = 5;", Some("5")),
                symbol("/Config", "class Config { int W = 4; }", None),
                symbol("/Config/W", "int W = 4;", Some("4")),
            ]
        );
        assert_eq!(
            unresolved(&java(source)?, source, "/A")?,
            syntax_error(3, 5, "A")
        );
        assert_eq!(
            unresolved(&java(source)?, source, "/A/Y")?,
            syntax_error(3, 5, "Y")
        );
        assert_eq!(
            unresolved(&java(source)?, source, "/Config/W")?,
            syntax_error(3, 5, "Config")
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Position, language_parsers::Comment};

    #[test]
    fn real_block_is_parsed_while_text_block_marker_is_ignored() -> anyhow::Result<()> {
        let contents = r#"class Example {
    // <block name="real">
    int answer = 42;
    // </block>
    String template = """
        // <block name="fake">
        this text lives inside a Java text block
        // </block>
        """;
}
"#;
        let blocks = parser()
            .parse_blocks(contents)
            .collect::<anyhow::Result<Vec<_>>>()?;
        let names: Vec<&str> = blocks
            .iter()
            .map(|block| block.attributes["name"].text())
            .collect();
        assert_eq!(names, ["real"]);
        Ok(())
    }

    #[test]
    fn parses_comments_correctly() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser();

        let blocks: Vec<Comment> = comments_parser
            .parse(
                r#"
        /**
         * This is a simple Java program demonstrating different types of comments.
         * 
         * @version 1.0
         */
        public class CommentExample {
        
            public static void main(String[] args) {
                // This is a single-line comment.
                System.out.println("Hello, World!"); // Prints a message to the console.
        
                /*
                 * This is a multi-line comment.
                 * It can span multiple lines.
                 */
                int number = 42; /* Assigning a value to the variable */
        
                /** This is a single-line doc-comment. */
                printMessage();
            }
        
            /**
             * Prints a sample message to the console.
             */
            public static void printMessage() {
                System.out.println("/**This is a method with a Javadoc comment.*/");
            }
        }
        /// Markdown doc comment.
//! Rust and Doxygen read this marker as a doc comment.
//// A banner line repeats the marker.
        "#,
            )
            .collect();

        assert_eq!(
            blocks,
            vec![
                Comment {
                    position_range: Position::new(2, 9)..Position::new(6, 12),
                    source_range: 9..144,
                    comment_text: "   \n           This is a simple Java program demonstrating different types of comments.\n           \n           @version 1.0\n           ".to_string()
                },
                Comment {
                    position_range: Position::new(10, 17)..Position::new(10, 50),
                    source_range: 261..294,
                    comment_text: "   This is a single-line comment.".to_string()
                },
                Comment {
                    position_range: Position::new(11, 54)..Position::new(11, 89),
                    source_range: 348..383,
                    comment_text: "   Prints a message to the console.".to_string()
                },
                Comment {
                    position_range: Position::new(13, 17)..Position::new(16, 20),
                    source_range: 409..527,
                    comment_text: "  \n                   This is a multi-line comment.\n                   It can span multiple lines.\n                   ".to_string()
                },
                Comment {
                    position_range: Position::new(17, 34)..Position::new(17, 73),
                    source_range: 561..600,
                    comment_text: "   Assigning a value to the variable   ".to_string()
                },
                Comment {
                    position_range: Position::new(19, 17)..Position::new(19, 58),
                    source_range: 626..667,
                    comment_text: "    This is a single-line doc-comment.   ".to_string()
                },
                Comment {
                    position_range: Position::new(23, 13)..Position::new(25, 16),
                    source_range: 735..809,
                    comment_text: "   \n               Prints a sample message to the console.\n               ".to_string()
                },
                Comment {
                    position_range: Position::new(30, 9)..Position::new(30, 34),
                    source_range: 975..1000,
                    comment_text: "    Markdown doc comment.".to_string()
                },
                Comment {
                    position_range: Position::new(31, 1)..Position::new(31, 56),
                    source_range: 1001..1056,
                    comment_text: "    Rust and Doxygen read this marker as a doc comment.".to_string()
                },
                Comment {
                    position_range: Position::new(32, 1)..Position::new(32, 39),
                    source_range: 1057..1095,
                    comment_text: "     A banner line repeats the marker.".to_string()
                },
            ]
        );

        Ok(())
    }
}
