use crate::language_parsers::{
    Comment, CommentsParser, LanguageParser, LanguageParserImpl, TreeSitterCommentsParser, Visit,
    c_style_and_doc_line_and_block_comment_text, node_text,
};
use crate::symbols::code::{self, CodeLanguage, CodeSymbolsParser, Definition, Value};
use anyhow::{anyhow, bail};
use std::ops::Range;
use tree_sitter::Node;

/// Returns a [`LanguageParser`] for Java, with its symbols.
pub(super) fn parser() -> anyhow::Result<impl LanguageParser> {
    Ok(
        LanguageParserImpl::new(comments_parser()?).with_symbols(CodeSymbolsParser::new(
            &tree_sitter_java::LANGUAGE.into(),
            Java,
        )?),
    )
}

/// Java, whose types hold its fields, methods and nested types.
struct Java;

impl CodeLanguage for Java {
    /// The types under `root`, the root of a Java file's syntax tree, and their members, in
    /// document order. A type comes before its members.
    fn definitions<'tree>(&self, root: Node<'tree>, _source: &str) -> Vec<Definition<'tree>> {
        let mut definitions = Vec::new();
        // The walk keeps its own stack, as deeply nested types could overflow the call stack.
        // A compact source file holds methods and fields at its top level, as a class body does.
        let mut bodies = vec![(root, Vec::new())];
        while let Some((body, parents)) = bodies.pop() {
            push_members(body, &parents, &mut definitions, &mut bodies);
        }
        // The walk reads all of a body before the bodies of the types in it.
        definitions.sort_by_key(|definition| definition.range.start);
        definitions
    }

    /// Decodes a Java string or text block. Returns `None` for any other node.
    ///
    /// # Errors
    /// Returns an error if an escape is not one of Java's, if a backslash ends the string, if the
    /// string holds half of a character, or if a text block holds text on the line of its opening
    /// quotes.
    fn decode_string(&self, node: &Node, source: &str) -> anyhow::Result<Option<String>> {
        if node.kind() != "string_literal" {
            return Ok(None);
        }
        let literal = node_text(node, source);
        // Java reads `\u` escapes before anything else in a file. So the character from one can be
        // a line end or white space in a text block, or start another escape.
        let content = match literal.strip_prefix(TEXT_BLOCK_QUOTES) {
            Some(block) => text_block_content(&unicode_unescaped(
                block.strip_suffix(TEXT_BLOCK_QUOTES).unwrap_or(block),
            )?)?,
            None => unicode_unescaped(
                literal
                    .strip_prefix('"')
                    .and_then(|content| content.strip_suffix('"'))
                    .unwrap_or(literal),
            )?,
        };
        unescaped(&content).map(Some)
    }
}

/// The quotes that open and close a text block.
const TEXT_BLOCK_QUOTES: &str = "\"\"\"";

/// The text of a text block whose content between its quotes is `block`, with its escapes other
/// than `\u` still as written. Each line end becomes a line feed. The first line, which holds only
/// white space after the opening quotes, is left out. The other lines lose the indentation they
/// share, and the white space at their ends.
///
/// # Errors
/// Returns an error if `block` has no line end, or if its first line holds anything other than
/// spaces, tabs and form feeds.
fn text_block_content(block: &str) -> anyhow::Result<String> {
    let block = block.replace("\r\n", "\n").replace('\r', "\n");
    let content = block
        .split_once('\n')
        .filter(|(first, _)| first.chars().all(|c| matches!(c, ' ' | '\t' | '\u{c}')))
        .map(|(_, content)| content)
        .ok_or_else(|| anyhow!("a text block must start on the line after its opening quotes"))?;
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
    Ok(lines.join("\n"))
}

/// Whether Java counts `character` as white space, as it does when it removes the indentation of
/// a text block. Unlike Rust, Java doesn't count the spaces that must not break, such as U+00A0,
/// nor U+0085, and it counts U+001C to U+001F.
fn is_white_space(character: char) -> bool {
    match character {
        '\u{1c}'..='\u{1f}' => true,
        '\u{85}' | '\u{a0}' | '\u{2007}' | '\u{202f}' => false,
        _ => character.is_whitespace(),
    }
}

/// `content`, the text of a Java string between its quotes, with each `\u` escape read as the
/// character it stands for. The other escapes stay as written.
///
/// # Errors
/// Returns an error if a `\u` escape doesn't end in 4 hex digits, or if the string holds half of
/// a character.
fn unicode_unescaped(content: &str) -> anyhow::Result<String> {
    // A Java string is UTF-16, so it is built from code units: a `\u` escape can give half of a
    // character, which the next one completes.
    let mut units = Vec::new();
    let mut characters = content.chars().peekable();
    // A backslash after an odd number of backslashes is escaped, so it can't start a `\u` escape.
    let mut odd_backslashes = false;
    while let Some(character) = characters.next() {
        if character == '\\' && !odd_backslashes && characters.next_if_eq(&'u').is_some() {
            while characters.next_if_eq(&'u').is_some() {}
            let digits: String = characters.by_ref().take(4).collect();
            // `from_str_radix` alone would also read `+041`, and fewer digits at the end.
            if digits.len() != 4 || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                bail!("a `\\u` escape needs 4 hex digits, not `{digits}`");
            }
            units.push(u16::from_str_radix(&digits, 16)?);
            continue;
        }
        odd_backslashes = character == '\\' && !odd_backslashes;
        units.extend(character.encode_utf16(&mut [0; 2]).iter());
    }
    code::string_from_utf16(units)
}

/// `content`, the text of a Java string between its quotes with its `\u` escapes already read,
/// with each other escape read as what it stands for.
///
/// # Errors
/// Returns an error if an escape is not one of Java's, or if a backslash ends `content`.
fn unescaped(content: &str) -> anyhow::Result<String> {
    let mut text = String::new();
    let mut characters = content.chars().peekable();
    while let Some(character) = characters.next() {
        if character != '\\' {
            text.push(character);
            continue;
        }
        // The grammar puts a character after every backslash written as is. One from a `\u`
        // escape can come last.
        let Some(escaped) = characters.next() else {
            bail!("the string ends in a backslash, which escapes its closing quote");
        };
        let character = match escaped {
            'b' => '\u{8}',
            's' => ' ',
            't' => '\t',
            'n' => '\n',
            'f' => '\u{c}',
            'r' => '\r',
            '"' | '\'' | '\\' => escaped,
            // A backslash at the end of a line in a text block joins the next line to it.
            '\n' => continue,
            '0'..='7' => {
                // An octal escape ends at `\377`, so only a first digit up to 3 takes two more.
                let longest = if escaped <= '3' { 3 } else { 2 };
                let mut digits = String::from(escaped);
                while digits.len() < longest
                    && let Some(digit) = characters.next_if(|c| c.is_digit(8))
                {
                    digits.push(digit);
                }
                char::from(u8::from_str_radix(&digits, 8)?)
            }
            _ => bail!("`\\{escaped}` is not an escape in a Java string"),
        };
        text.push(character);
    }
    Ok(text)
}

/// Adds the definitions of the members of `body`, the root of a file or the body of a type, to
/// `definitions`. `parents` are the names of the types that `body` belongs to. Adds the body of
/// each type in `body` to `bodies`, with its own parents.
fn push_members<'tree>(
    body: Node<'tree>,
    parents: &[Node<'tree>],
    definitions: &mut Vec<Definition<'tree>>,
    bodies: &mut Vec<(Node<'tree>, Vec<Node<'tree>>)>,
) {
    let mut cursor = body.walk();
    for member in body.named_children(&mut cursor) {
        let name = member.child_by_field_name("name");
        match member.kind() {
            // A compact source file declares its fields as local variables, and an interface its
            // constants as constants.
            "field_declaration" | "local_variable_declaration" | "constant_declaration" => {
                push_fields(member, parents, definitions);
            }
            // The members of an enum that come after its constants.
            "enum_body_declarations" => bodies.push((member, parents.to_vec())),
            "class_declaration"
            | "interface_declaration"
            | "enum_declaration"
            | "record_declaration"
            | "annotation_type_declaration" => {
                let Some(name) = name else {
                    continue;
                };
                definitions.push(definition(parents, name, member.byte_range(), None));
                if let Some(body) = member.child_by_field_name("body") {
                    bodies.push((body, [parents, &[name]].concat()));
                }
            }
            // The body of an enum constant is a class without a name, so its members are not
            // members of the enum.
            "method_declaration" | "enum_constant" => definitions
                .extend(name.map(|name| definition(parents, name, member.byte_range(), None))),
            "annotation_type_element_declaration" => {
                definitions.extend(name.map(|name| {
                    let default = member.child_by_field_name("value");
                    definition(parents, name, member.byte_range(), default)
                }));
            }
            // Code never refers to a constructor or an initializer block by name.
            _ => {}
        }
    }
}

/// Adds a definition to `definitions` for each name that `declaration`, a field declaration in
/// the type at `parents`, declares.
fn push_fields<'tree>(
    declaration: Node<'tree>,
    parents: &[Node<'tree>],
    definitions: &mut Vec<Definition<'tree>>,
) {
    let mut cursor = declaration.walk();
    let declarators: Vec<Node> = declaration
        .children_by_field_name("declarator", &mut cursor)
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
        let value = declarator.child_by_field_name("value");
        definitions.push(definition(parents, name, range, value));
    }
}

/// The definition of `name` in the type at `parents`, which covers `range` and has the
/// initializer `value`, if any.
fn definition<'tree>(
    parents: &[Node<'tree>],
    name: Node<'tree>,
    range: Range<usize>,
    value: Option<Node<'tree>>,
) -> Definition<'tree> {
    Definition {
        parents: parents.to_vec(),
        names: vec![name],
        range,
        value: value.map(Value::Initializer),
    }
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let java_language = tree_sitter_java::LANGUAGE.into();
    let parser = TreeSitterCommentsParser::new(
        &java_language,
        Box::new(|node, source_code| {
            // A text block (`"""..."""`) is a `string_literal`, and tree-sitter can nest a
            // comment-shaped node inside it; its text is string content, not a source comment.
            if node.kind() == "string_literal" {
                return Visit::Break(None);
            }
            match c_style_and_doc_line_and_block_comment_text(
                node,
                source_code,
                "line_comment",
                "block_comment",
            ) {
                Some(text) => Visit::Break(Some(Comment::from_node(node, source_code, text))),
                None => Visit::Continue,
            }
        }),
    )?;
    Ok(parser)
}

#[cfg(test)]
mod symbols_tests {
    use super::*;
    use crate::symbols::code::test_utils::{
        decode_error, symbol, symbols, syntax_error, unresolved, value, values,
    };
    use crate::symbols::{Symbols, SymbolsParser};
    use anyhow::Context;

    /// The symbols of `source`, a Java file.
    fn java(source: &str) -> anyhow::Result<Symbols> {
        CodeSymbolsParser::new(&tree_sitter_java::LANGUAGE.into(), Java)?.parse(source)
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
        // The class `Config` runs from its annotation to the line before `interface Service`.
        let config = &source[source.find("@Deprecated").context("no class")?
            ..source.find("\n\ninterface").context("no interface")?];

        // <block affects="docs/symbols.md:java-paths, src/skill.md#/blockwatch/Symbols/Java">
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
    static final String UNICODE_BACKSLASH = "\u005cn";
    static final String UNICODE_AFTER_BACKSLASHES = "\\\u0041";
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
                // Java reads `\u` escapes before the others, so the backslash from the first one
                // starts `\n`.
                value("/Strings/UNICODE_BACKSLASH", "\n"),
                // The third backslash comes after two, so it starts a `\u` escape.
                value("/Strings/UNICODE_AFTER_BACKSLASHES", r"\A"),
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
    static final String SEVERAL_US = """
        \uu0041
        """;
    static final String UNICODE_SPACE_AT_END = """
        a\u0020
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
                // Escapes other than `\u` are read after the indentation is removed.
                value("/Blocks/ESCAPED_INDENT", "\ta\ncafé\n"),
                // Java allows any number of `u`s in a `\u` escape.
                value("/Blocks/SEVERAL_US", "A\n"),
                // Java reads `\u` escapes before it removes the white space at line ends.
                value("/Blocks/UNICODE_SPACE_AT_END", "a\n"),
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
    fn lines_that_end_in_unusual_spaces_parse_symbols_removes_only_java_white_space()
    -> anyhow::Result<()> {
        // Java doesn't count the spaces that must not break, nor U+0085, as white space. It counts
        // U+001F, which Rust doesn't.
        let source = "class Spaces {\n    static final String ENDS = \"\"\"\n        a\u{85}\n        a\u{a0}\n        a\u{2007}\n        a\u{202f}\n        a\u{1f}\n        \"\"\";\n}\n";

        assert_eq!(
            values(&java(source)?, source)[1..],
            [value(
                "/Spaces/ENDS",
                "a\u{85}\na\u{a0}\na\u{2007}\na\u{202f}\na\n"
            )]
        );
        Ok(())
    }

    #[test]
    fn escape_that_is_not_text_parse_symbols_makes_the_name_unreadable() -> anyhow::Result<()> {
        let source = r#"class Broken {
    static final String HEX = "\x41";
    static final String HALF = "\uD800";
    static final String BACKSLASH = "\u005c";
    static final String PLUS = """
        \u+041
        """;
    static final String SHORT = """
        \u41""";
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
        // Java reads a `\u` escape before the others, so the backslash from `\u005c` escapes the
        // closing quote.
        assert_eq!(
            unresolved(&java(source)?, source, "/Broken/BACKSLASH")?,
            decode_error(
                4,
                37,
                "the string ends in a backslash, which escapes its closing quote"
            )
        );
        assert_eq!(
            unresolved(&java(source)?, source, "/Broken/PLUS")?,
            decode_error(5, 32, r"a `\u` escape needs 4 hex digits, not `+041`")
        );
        assert_eq!(
            unresolved(&java(source)?, source, "/Broken/SHORT")?,
            decode_error(8, 33, r"a `\u` escape needs 4 hex digits, not `41`")
        );
        Ok(())
    }

    #[test]
    fn text_on_line_of_opening_quotes_parse_symbols_makes_the_name_unreadable() -> anyhow::Result<()>
    {
        // Only spaces, tabs and form feeds can follow the opening quotes on their line. Java counts
        // U+001C as white space only when it removes white space from the other lines.
        let source = r#"class Opening {
    static final String SPACES = """WHITE_SPACE
        a
        """;
    static final String OTHER_SPACE = """SEPARATOR
        a
        """;
    static final String TEXT = """ a
        b
        """;
    static final String SAME_LINE = """a""";
}
"#
        .replace("WHITE_SPACE", " \t\u{c}")
        .replace("SEPARATOR", "\u{1c}");

        assert_eq!(
            values(&java(&source)?, &source)[1..],
            [value("/Opening/SPACES", "a\n")]
        );
        let problem = "a text block must start on the line after its opening quotes";
        assert_eq!(
            unresolved(&java(&source)?, &source, "/Opening/OTHER_SPACE")?,
            decode_error(5, 39, problem)
        );
        assert_eq!(
            unresolved(&java(&source)?, &source, "/Opening/TEXT")?,
            decode_error(8, 32, problem)
        );
        assert_eq!(
            unresolved(&java(&source)?, &source, "/Opening/SAME_LINE")?,
            decode_error(11, 37, problem)
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
        let blocks = parser()?
            .parse_blocks(contents)
            .collect::<anyhow::Result<Vec<_>>>()?;
        let names: Vec<&str> = blocks
            .iter()
            .map(|block| block.attributes["name"].as_str())
            .collect();
        assert_eq!(names, ["real"]);
        Ok(())
    }

    #[test]
    fn parses_comments_correctly() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser()?;

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
                }
            ]
        );

        Ok(())
    }
}
