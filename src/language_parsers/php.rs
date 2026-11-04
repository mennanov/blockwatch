use crate::language_parsers;
use crate::language_parsers::{
    Comment, CommentsParser, LanguageParser, LanguageParserImpl, TreeSitterCommentsParser,
    node_text,
};
use crate::symbols::code::{self, CodeLanguage, CodeSymbolsParser, Definition, NestedWalk, child};
use anyhow::Context;
use std::iter::Peekable;
use std::str::Chars;
use tree_sitter::{Node, StreamingIterator};

/// Returns a [`LanguageParser`] for PHP, with its symbols.
pub(super) fn parser() -> anyhow::Result<impl LanguageParser> {
    // `LANGUAGE_PHP` reads the HTML of a template as text, so a template parses without errors.
    Ok(
        LanguageParserImpl::new(comments_parser()?).with_symbols(CodeSymbolsParser::new(
            &tree_sitter_php::LANGUAGE_PHP.into(),
            Php,
        )?),
    )
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    PhpCommentsParser::new()
}

/// Parses PHP comments and the HTML comments found in the template sections of a PHP file.
///
/// tree-sitter-php exposes the HTML sections of a template as opaque `text` nodes, so the HTML
/// comments in them are extracted with a separate HTML grammar pass, mirroring how markdown.rs
/// handles its `html_block` nodes.
struct PhpCommentsParser {
    php_tree_sitter_parser: tree_sitter::Parser,
    regions_query: tree_sitter::Query,
    html_comments_parser: TreeSitterCommentsParser,
}

impl PhpCommentsParser {
    fn new() -> anyhow::Result<Self> {
        let php_language: tree_sitter::Language = tree_sitter_php::LANGUAGE_PHP.into();
        let php_tree_sitter_parser = language_parsers::tree_sitter_parser(&php_language)?;
        let regions_query =
            tree_sitter::Query::new(&php_language, "(comment) @comment (text) @text")
                .context("failed to compile tree-sitter php regions query")?;
        let html_comments_parser = language_parsers::xml_style_comments_parser(
            &tree_sitter_html::LANGUAGE.into(),
            "comment",
        )?;
        Ok(Self {
            php_tree_sitter_parser,
            regions_query,
            html_comments_parser,
        })
    }
}

impl CommentsParser for PhpCommentsParser {
    fn parse<'source>(
        &'source mut self,
        source_code: &'source str,
    ) -> impl Iterator<Item = Comment> + 'source {
        // The HTML template sections are blanked into an "HTML view" of the file (PHP regions
        // become whitespace, line breaks kept, `text` nodes copied back verbatim), then parsed
        // once with the HTML grammar. An HTML comment interrupted by a `<?php ?>` island reforms
        // into one comment, and the view is byte-for-byte aligned with the source so the parsed
        // positions are already the source positions. Skip it all when there is no `<!--`.
        let mut html_view = source_code.contains("<!--").then(|| {
            let mut view = source_code.as_bytes().to_vec();
            language_parsers::blank_preserving_line_breaks(&mut view);
            view
        });

        // A single PHP parse yields both the PHP `comment` nodes and the `text` regions that make
        // up the HTML view.
        let mut comments = Vec::new();
        let tree = self
            .php_tree_sitter_parser
            .parse(source_code, None)
            .unwrap();
        let mut query_cursor = tree_sitter::QueryCursor::new();
        let mut matches = query_cursor.matches(
            &self.regions_query,
            tree.root_node(),
            source_code.as_bytes(),
        );
        while let Some(query_match) = matches.next() {
            let node = query_match
                .captures()
                .first()
                .expect("Empty Tree-sitter region query match")
                .node;
            if node.kind() == "comment" {
                let text =
                    language_parsers::hash_and_c_style_comment_text(node_text(&node, source_code));
                comments.push(Comment::from_node(&node, source_code, text));
            } else if let Some(view) = html_view.as_mut() {
                // A `text` node: copy the HTML template section back into the blanked view.
                let range = node.byte_range();
                view[range.clone()].copy_from_slice(&source_code.as_bytes()[range]);
            }
        }
        drop(matches);

        if let Some(view) = html_view {
            let view =
                String::from_utf8(view).expect("HTML view is built from the valid-UTF-8 source");
            comments.extend(self.html_comments_parser.parse(&view).map(|mut comment| {
                // Blanking rewrites a PHP region byte by byte, so a multi-byte character there
                // becomes several spaces and the view's columns run ahead of the source's.
                comment.set_character_columns(source_code);
                comment
            }));
        }
        comments.sort_by_key(|comment| comment.source_range.start);
        comments.into_iter()
    }
}

/// PHP, whose files hold constants, variables, functions and types, and whose types hold their
/// members.
struct Php;

impl CodeLanguage for Php {
    /// The definitions under `root`, the root of a PHP file's syntax tree, and the members of its
    /// types, in document order. A type comes before its members. A `define` call in a function
    /// or a method is a top-level definition.
    fn definitions<'tree>(&self, root: Node<'tree>, source: &str) -> Vec<Definition<'tree>> {
        code::nested_definitions(root, |body, parents, walk| {
            push_members(body, &parents, walk, source);
        })
    }

    /// Decodes a PHP string, heredoc or nowdoc. Returns `None` for any other node, for a string
    /// with a variable or an expression in it, such as `"hi $name"`, and for a string whose bytes
    /// are not UTF-8.
    ///
    /// # Errors
    /// Never returns an error. A string that PHP rejects keeps its value as written too, as a
    /// file that doesn't run has no value to copy.
    fn decode_string(&self, node: &Node, source: &str) -> anyhow::Result<Option<String>> {
        let literal = node_text(node, source);
        // A `b` before the quotes, as in `b"text"`, keeps the string as written.
        let decoded = match node.kind() {
            "string" => code::between_quotes(literal, "'")
                .ok()
                .map(decode_single_quoted),
            "encapsed_string" if holds_only_text(*node) => code::between_quotes(literal, "\"")
                .ok()
                .and_then(|content| decode_escapes(content, true)),
            "heredoc" if child(*node, "heredoc_body").is_none_or(holds_only_text) => {
                heredoc_text(literal).and_then(|text| decode_escapes(&text, false))
            }
            "nowdoc" => heredoc_text(literal),
            _ => None,
        };
        Ok(decoded)
    }

    // A property and a variable keep their `$`.
    const NAME_PUNCTUATION: &'static [char] = &['$'];
}

/// Adds the definitions in `body` to `walk`: the root of a file, a block, or the body of a type.
/// `parents` are the names of the types that `body` belongs to. Adds each body in `body` to `walk`,
/// with its own parents.
fn push_members<'tree>(
    body: Node<'tree>,
    parents: &[Node<'tree>],
    walk: &mut NestedWalk<'tree>,
    source: &str,
) {
    let mut cursor = body.walk();
    for member in body.named_children(&mut cursor) {
        match member.kind() {
            kind if is_block(kind) => walk.add_body(member, parents.to_vec()),
            "expression_statement" => push_statement(member, parents, walk, source),
            "function_definition" => {
                if let Some(name) = member.child_by_field_name("name") {
                    walk.add_definition(parents, name, member.byte_range(), None);
                }
                push_defines(member, walk, source);
            }
            "method_declaration" => {
                if let Some(name) = member.child_by_field_name("name") {
                    walk.add_definition(parents, name, member.byte_range(), None);
                }
                push_promoted_properties(member, parents, walk);
                push_defines(member, walk, source);
            }
            "class_declaration"
            | "interface_declaration"
            | "trait_declaration"
            | "enum_declaration" => {
                if let Some(name) = member.child_by_field_name("name") {
                    let body = member.child_by_field_name("body");
                    walk.add_definition_with_body(parents, name, member.byte_range(), None, body);
                }
            }
            "const_declaration" => push_elements(member, "const_element", parents, walk),
            "property_declaration" => push_elements(member, "property_element", parents, walk),
            "enum_case" => {
                if let Some(name) = member.child_by_field_name("name") {
                    let value = member.child_by_field_name("value");
                    walk.add_definition(parents, name, member.byte_range(), value);
                }
            }
            // `use` imports, trait uses and `return` define no name.
            _ => {}
        }
    }
}

/// Whether a statement of the `kind` is a block, whose statements are read as part of the body
/// around it.
fn is_block(kind: &str) -> bool {
    // Code often defines a function or a class only if it doesn't exist yet. The condition of an
    // `if` and the value of a `case` are not statements, so the walk skips them. A loop can run
    // its body many times or never, so its definitions are not symbols.
    matches!(
        kind,
        "compound_statement"
            | "colon_block"
            | "if_statement"
            | "else_if_clause"
            | "else_clause"
            | "try_statement"
            | "catch_clause"
            | "finally_clause"
            | "switch_statement"
            | "switch_block"
            | "case_statement"
            | "default_statement"
            | "declare_statement"
            | "namespace_definition"
    )
}

/// Adds the definition that `statement`, an expression statement in the type at `parents`, makes
/// to `walk`: a constant for `define('NAME', value);`, or a variable for `$name = value;`.
fn push_statement<'tree>(
    statement: Node<'tree>,
    parents: &[Node<'tree>],
    walk: &mut NestedWalk<'tree>,
    source: &str,
) {
    let Some(expression) = statement.named_child(0) else {
        return;
    };
    match expression.kind() {
        "function_call_expression" => {
            if let Some((name, value)) = define(statement, source) {
                walk.add_definition(parents, name, statement.byte_range(), value);
            }
        }
        "assignment_expression" => {
            // An assignment to an array item or a property changes a value that is defined
            // elsewhere.
            let left = expression
                .child_by_field_name("left")
                .filter(|left| left.kind() == "variable_name");
            if let Some(left) = left {
                let value = expression.child_by_field_name("right");
                walk.add_definition(parents, left, statement.byte_range(), value);
            }
        }
        _ => {}
    }
}

/// Adds the constants that `define` calls in the body of `function`, a function or a method,
/// define to `walk`, with no parents.
fn push_defines<'tree>(function: Node<'tree>, walk: &mut NestedWalk<'tree>, source: &str) {
    // A function runs only when called, so its own definitions are local. A `define` in it still
    // defines a global constant. The walk keeps its own stack, as deeply nested blocks could
    // overflow the call stack.
    let mut bodies: Vec<Node> = function.child_by_field_name("body").into_iter().collect();
    while let Some(body) = bodies.pop() {
        let mut cursor = body.walk();
        for member in body.named_children(&mut cursor) {
            match member.kind() {
                kind if is_block(kind) => bodies.push(member),
                "expression_statement" => {
                    if let Some((name, value)) = define(member, source) {
                        walk.add_definition(&[], name, member.byte_range(), value);
                    }
                }
                "function_definition" => bodies.extend(member.child_by_field_name("body")),
                // Classes declared in a function are rare, so the walk doesn't read their methods.
                _ => {}
            }
        }
    }
}

/// The name and the value of the constant that `statement`, an expression statement, defines,
/// when it is `define('NAME', value);` with a name that is plain text. The name is the text
/// between the quotes. The value is `None` when the call has only a name. Returns `None` for any
/// other statement.
fn define<'tree>(
    statement: Node<'tree>,
    source: &str,
) -> Option<(Node<'tree>, Option<Node<'tree>>)> {
    // Only a function call has a function.
    let call = statement.named_child(0)?;
    // PHP ignores the case of a function's name. A name in a namespace may be a function of that
    // namespace, unless it starts with `\`.
    let function = call.child_by_field_name("function")?;
    let called = node_text(&function, source);
    let is_define = match function.kind() {
        "name" => called.eq_ignore_ascii_case("define"),
        "qualified_name" => called.eq_ignore_ascii_case("\\define"),
        _ => false,
    };
    if !is_define {
        return None;
    }
    let arguments = call.child_by_field_name("arguments")?;
    let mut values = Vec::new();
    let mut cursor = arguments.walk();
    // Comments sit between the arguments.
    for argument in arguments
        .named_children(&mut cursor)
        .filter(|argument| argument.kind() == "argument")
    {
        // Named arguments can come in any order, as in `define(value: 1, constant_name: 'X')`.
        if argument.child_by_field_name("name").is_some() {
            return None;
        }
        values.push(argument.named_child(0)?);
    }
    let mut values = values.into_iter();
    // A command in backticks holds text too.
    let name = values
        .next()
        .filter(|name| matches!(name.kind(), "string" | "encapsed_string"))?;
    Some((plain_text(name)?, values.next()))
}

/// The text of `string`, a quoted string, when it holds only text, with no escape and no
/// variable. Returns `None` for an empty string too.
fn plain_text(string: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = string.walk();
    let mut parts = string.named_children(&mut cursor);
    match (parts.next(), parts.next()) {
        (Some(text), None) if text.kind() == "string_content" => Some(text),
        _ => None,
    }
}

/// Whether `string`, a double-quoted string or the body of a heredoc, holds only text and
/// escapes, with no variable and no expression.
fn holds_only_text(string: Node<'_>) -> bool {
    let mut cursor = string.walk();
    string
        .named_children(&mut cursor)
        .all(|part| matches!(part.kind(), "string_content" | "escape_sequence"))
}

/// The text of `content`, the inside of a single-quoted string. Only `\'` and `\\` are escapes.
fn decode_single_quoted(content: &str) -> String {
    let mut text = String::with_capacity(content.len());
    let mut characters = content.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '\\'
            && let Some(escaped) = characters.next_if(|next| matches!(next, '\\' | '\''))
        {
            text.push(escaped);
        } else {
            text.push(character);
        }
    }
    text
}

/// The text of `literal`, a heredoc or a nowdoc, without its markers, with the closing marker's
/// indent removed from each line. Escapes are left as written. Returns `None` when PHP rejects the
/// indent: the closing marker's indent mixes tabs and spaces, or a line doesn't start with it.
fn heredoc_text(literal: &str) -> Option<String> {
    let (_, rest) = literal.split_once('\n')?;
    // The body ends with the line end before the closing marker, which is not part of the text.
    // An empty body has no line end of its own.
    let (body, closing) = rest.rsplit_once('\n').unwrap_or(("", rest));
    let body = body.strip_suffix('\r').unwrap_or(body);
    let indent = closing
        .strip_suffix(closing.trim_start_matches([' ', '\t']))
        .unwrap_or_default();
    if indent.contains(' ') && indent.contains('\t') {
        return None;
    }
    let lines = body.split('\n').map(|line| {
        // A line can end with the `\r` of a `\r\n`, which stays.
        let content = line.strip_suffix('\r').unwrap_or(line);
        match line.strip_prefix(indent) {
            Some(rest) => Some(rest),
            // PHP empties a line of white space that is shorter than the indent.
            None if indent.starts_with(content) => line.strip_prefix(content),
            None => None,
        }
    });
    Some(lines.collect::<Option<Vec<_>>>()?.join("\n"))
}

/// The text of `content`, the inside of a double-quoted string or of a heredoc, with its escapes
/// decoded. `\"` is an escape only when `quote_escape` is true. A backslash that starts no
/// escape stays. Returns `None` when the escapes give bytes that are not UTF-8, or when PHP rejects
/// an escape, such as `\u{zz}`.
fn decode_escapes(content: &str, quote_escape: bool) -> Option<String> {
    // An escape can give one byte of a character, as `"\xC3\xA9"` gives `é`.
    let mut bytes = Vec::with_capacity(content.len());
    let mut characters = content.chars().peekable();
    while let Some(character) = characters.next() {
        if character != '\\' {
            bytes.extend_from_slice(character.encode_utf8(&mut [0; 4]).as_bytes());
            continue;
        }
        let simple = match characters.peek() {
            Some('n') => Some(b'\n'),
            Some('r') => Some(b'\r'),
            Some('t') => Some(b'\t'),
            Some('v') => Some(0x0b),
            Some('e') => Some(0x1b),
            Some('f') => Some(0x0c),
            Some('\\') => Some(b'\\'),
            Some('$') => Some(b'$'),
            Some('"') if quote_escape => Some(b'"'),
            _ => None,
        };
        if let Some(byte) = simple {
            characters.next();
            bytes.push(byte);
        } else if let Some(digits) = digits(&mut characters, 8, 3) {
            // PHP warns about `\400` and above, and keeps only the low byte. That byte is not the
            // number the code shows, so the string is kept as written.
            bytes.push(u8::try_from(digits).ok()?);
        } else if characters.next_if_eq(&'x').is_some() {
            match digits(&mut characters, 16, 2) {
                Some(digits) => bytes.push(u8::try_from(digits).ok()?),
                None => bytes.extend_from_slice(b"\\x"),
            }
        } else if characters.peek() == Some(&'u') && characters.clone().nth(1) == Some('{') {
            characters.nth(1);
            // PHP rejects `\u{41` without its closing brace, but this reads it as `A`. Such a
            // file doesn't run, and catching it would take more code than it's worth.
            let digits: String = characters.by_ref().take_while(|&c| c != '}').collect();
            // PHP rejects a sign, as in `\u{+41}`, which `from_str_radix` accepts. A surrogate,
            // such as `\u{D800}`, is not a character.
            let escaped = Some(digits)
                .filter(|digits| digits.chars().all(|digit| digit.is_ascii_hexdigit()))
                .and_then(|digits| u32::from_str_radix(&digits, 16).ok())
                .and_then(char::from_u32)?;
            bytes.extend_from_slice(escaped.encode_utf8(&mut [0; 4]).as_bytes());
        } else {
            bytes.push(b'\\');
        }
    }
    String::from_utf8(bytes).ok()
}

/// Reads up to `max` digits in `radix` from the start of `characters`, and returns their value.
/// Returns `None` when `characters` doesn't start with a digit.
fn digits(characters: &mut Peekable<Chars>, radix: u32, max: usize) -> Option<u32> {
    let mut value = None;
    for _ in 0..max {
        let Some(digit) = characters.peek().and_then(|c| c.to_digit(radix)) else {
            break;
        };
        characters.next();
        value = Some(value.unwrap_or(0) * radix + digit);
    }
    value
}

/// Adds a definition for each element of `declaration`, a `const` or a property declaration in the
/// type at `parents`, to `walk`. `element` is the kind of its elements.
fn push_elements<'tree>(
    declaration: Node<'tree>,
    element: &str,
    parents: &[Node<'tree>],
    walk: &mut NestedWalk<'tree>,
) {
    let mut cursor = declaration.walk();
    let elements: Vec<Node> = declaration
        .named_children(&mut cursor)
        .filter(|child| child.kind() == element)
        .collect();
    // In `const A = 1, B = 2;`, each name has its own part, so a change to one does not touch the
    // other.
    let alone = elements.len() == 1;
    for element in elements {
        // A constant's name has no field name, and a property's value has one.
        let name = element
            .child_by_field_name("name")
            .or_else(|| child(element, "name"));
        let Some(name) = name else {
            continue;
        };
        let value = element
            .child_by_field_name("default_value")
            .or_else(|| code::value_after(element, "="));
        let range = if alone {
            declaration.byte_range()
        } else {
            element.byte_range()
        };
        walk.add_definition(parents, name, range, value);
    }
}

/// Adds the definitions of the properties that `method`, a method in the type at `parents`,
/// declares with its parameters, such as `private int $count` in `__construct(private int $count)`,
/// to `walk`.
fn push_promoted_properties<'tree>(
    method: Node<'tree>,
    parents: &[Node<'tree>],
    walk: &mut NestedWalk<'tree>,
) {
    let Some(parameters) = method.child_by_field_name("parameters") else {
        return;
    };
    // PHP allows such a parameter only in a constructor, so any method may be read.
    let mut cursor = parameters.walk();
    for parameter in parameters.named_children(&mut cursor) {
        if parameter.kind() != "property_promotion_parameter" {
            continue;
        }
        if let Some(name) = parameter.child_by_field_name("name") {
            let value = parameter.child_by_field_name("default_value");
            walk.add_definition(parents, name, parameter.byte_range(), value);
        }
    }
}

#[cfg(test)]
mod symbols_tests {
    use super::*;
    use crate::symbols::code::test_utils::{
        symbol, symbols, syntax_error, text_of, unresolved, value, values,
    };
    use crate::symbols::{Symbols, SymbolsParser};

    /// The symbols of `source`, a PHP file.
    fn php(source: &str) -> anyhow::Result<Symbols> {
        CodeSymbolsParser::new(&tree_sitter_php::LANGUAGE_PHP.into(), Php)?.parse(source)
    }

    #[test]
    fn file_with_every_shape_parse_symbols_returns_every_symbol_in_order() -> anyhow::Result<()> {
        let source = r#"<?php
declare(strict_types=1);

namespace App\Billing;

use App\Support\Model;

/** Doc. */
const MAX_RETRIES = 3;
const LOW = 1, HIGH = (2);
define('API_URL', 'https://api.example.com');
\DEFINE("TIMEOUT", 30, true);
Define('RETRIES', /* times */ 5);
$version = '2.4.1';
$handler = function () { return 1; };

function money(int $cents): string { return ''; }

#[Entity]
abstract class Invoice extends Model
{
    use Auditable;

    /** Doc. */
    public const STATUS_PAID = 'paid';
    const A = 1, B = 2;
    #[Column]
    protected static int $perPage = 25;
    public $first = 1, $second;
    protected $table;

    public function __construct(
        protected int $count,
        private readonly string $currency = "EUR",
        $plain = 0,
    ) {}

    public function table(): string { return $this->table; }

    abstract protected function total(): int;
}

interface Payable
{
    const LIMIT = 10;
    public function pay(): void;
}

trait Auditable
{
    public static $log = [];
    public function audit() {}
}

enum Status: string
{
    case Open = 'open';
    #[Deprecated]
    case Paid = 'paid';
    const DEFAULT = self::Open;
    public function label() {}
}

enum Suit
{
    case Hearts;
}
"#;

        // <block affects="docs/symbols.md:php-paths, src/skill.md#/blockwatch/Symbols/PHP">
        assert_eq!(
            symbols(&php(source)?, source),
            vec![
                symbol("/MAX_RETRIES", "const MAX_RETRIES = 3;", Some("3")),
                symbol("/LOW", "LOW = 1", Some("1")),
                symbol("/HIGH", "HIGH = (2)", Some("2")),
                symbol(
                    "/API_URL",
                    "define('API_URL', 'https://api.example.com');",
                    Some("https://api.example.com")
                ),
                symbol("/TIMEOUT", r#"\DEFINE("TIMEOUT", 30, true);"#, Some("30")),
                symbol("/RETRIES", "Define('RETRIES', /* times */ 5);", Some("5")),
                symbol("/$version", "$version = '2.4.1';", Some("2.4.1")),
                symbol(
                    "/$handler",
                    "$handler = function () { return 1; };",
                    Some("function () { return 1; }")
                ),
                symbol(
                    "/money",
                    "function money(int $cents): string { return ''; }",
                    None
                ),
                symbol("/Invoice", text_of(source, "#[Entity]", "\n}")?, None),
                symbol(
                    "/Invoice/STATUS_PAID",
                    "public const STATUS_PAID = 'paid';",
                    Some("paid")
                ),
                symbol("/Invoice/A", "A = 1", Some("1")),
                symbol("/Invoice/B", "B = 2", Some("2")),
                symbol(
                    "/Invoice/$perPage",
                    "#[Column]\n    protected static int $perPage = 25;",
                    Some("25")
                ),
                symbol("/Invoice/$first", "$first = 1", Some("1")),
                symbol("/Invoice/$second", "$second", None),
                symbol("/Invoice/$table", "protected $table;", None),
                symbol(
                    "/Invoice/__construct",
                    text_of(source, "public function __construct", "{}")?,
                    None
                ),
                symbol("/Invoice/$count", "protected int $count", None),
                symbol(
                    "/Invoice/$currency",
                    r#"private readonly string $currency = "EUR""#,
                    Some("EUR")
                ),
                symbol(
                    "/Invoice/table",
                    "public function table(): string { return $this->table; }",
                    None
                ),
                symbol(
                    "/Invoice/total",
                    "abstract protected function total(): int;",
                    None
                ),
                symbol(
                    "/Payable",
                    text_of(source, "interface Payable", "\n}")?,
                    None
                ),
                symbol("/Payable/LIMIT", "const LIMIT = 10;", Some("10")),
                symbol("/Payable/pay", "public function pay(): void;", None),
                symbol(
                    "/Auditable",
                    text_of(source, "trait Auditable", "\n}")?,
                    None
                ),
                symbol("/Auditable/$log", "public static $log = [];", Some("[]")),
                symbol("/Auditable/audit", "public function audit() {}", None),
                symbol("/Status", text_of(source, "enum Status", "\n}")?, None),
                symbol("/Status/Open", "case Open = 'open';", Some("open")),
                symbol(
                    "/Status/Paid",
                    "#[Deprecated]\n    case Paid = 'paid';",
                    Some("paid")
                ),
                symbol(
                    "/Status/DEFAULT",
                    "const DEFAULT = self::Open;",
                    Some("self::Open")
                ),
                symbol("/Status/label", "public function label() {}", None),
                symbol("/Suit", "enum Suit\n{\n    case Hearts;\n}", None),
                symbol("/Suit/Hearts", "case Hearts;", None),
            ]
        );
        // </block>
        Ok(())
    }

    #[test]
    fn blocks_outside_functions_parse_symbols_returns_their_definitions() -> anyhow::Result<()> {
        let source = r#"<?php
if (!function_exists('money')) {
    function money() {}
} elseif (PHP_OS === 'Linux') {
    define('IN_ELSEIF', 1);
} else if (PHP_OS === 'Darwin') {
    define('IN_ELSE_IF', 1);
} else {
    $inElse = 1;
}
if (PHP_VERSION_ID > 80000)
    define('ONE_STATEMENT', 1);
if (PHP_VERSION_ID > 80100):
    $inColonBlock = 1;
endif;
try {
    $inTry = 1;
} catch (Exception $e) {
    $inCatch = 1;
} finally {
    $inFinally = 1;
}
switch (PHP_OS) {
    case 'Linux':
        $inCase = 1;
        break;
    default:
        $inDefault = 1;
}
{
    $inBlock = 1;
}
declare(ticks=1) {
    $inDeclare = 1;
}
namespace Braced {
    $inNamespace = 1;
}
"#;

        assert_eq!(
            values(&php(source)?, source),
            [
                ("/money".to_string(), None),
                value("/IN_ELSEIF", "1"),
                value("/IN_ELSE_IF", "1"),
                value("/$inElse", "1"),
                value("/ONE_STATEMENT", "1"),
                value("/$inColonBlock", "1"),
                value("/$inTry", "1"),
                value("/$inCatch", "1"),
                value("/$inFinally", "1"),
                value("/$inCase", "1"),
                value("/$inDefault", "1"),
                value("/$inBlock", "1"),
                value("/$inDeclare", "1"),
                value("/$inNamespace", "1"),
            ]
        );
        Ok(())
    }

    #[test]
    fn loops_parse_symbols_skips_their_definitions() -> anyhow::Result<()> {
        let source = r#"<?php
for ($i = 0; $i < 1; $i++) {
    $inFor = 1;
}
foreach ([1] as $item) {
    define('IN_FOREACH', 1);
}
while (false) {
    function inWhile() {}
}
do {
    $inDo = 1;
} while (false);
"#;

        assert_eq!(values(&php(source)?, source), []);
        Ok(())
    }

    #[test]
    fn copies_in_if_branches_parse_symbols_returns_each_copy() -> anyhow::Result<()> {
        let source = r#"<?php
if (PHP_VERSION_ID >= 80000) {
    class Client { public function get() { return 8; } }
    function helper() { return 8; }
} else {
    class Client { public function get() { return 7; } }
    function helper() { return 7; }
}
"#;

        assert_eq!(
            values(&php(source)?, source),
            [
                ("/Client".to_string(), None),
                ("/Client/get".to_string(), None),
                ("/helper".to_string(), None),
                ("/Client".to_string(), None),
                ("/Client/get".to_string(), None),
                ("/helper".to_string(), None),
            ]
        );
        Ok(())
    }

    #[test]
    fn namespaces_parse_symbols_leaves_them_out_of_the_path() -> anyhow::Result<()> {
        let source = r#"<?php
namespace First;
const LEVEL = 1;
class Config {}

namespace Second;
const LEVEL = 2;
"#;

        assert_eq!(
            values(&php(source)?, source),
            [
                value("/LEVEL", "1"),
                ("/Config".to_string(), None),
                value("/LEVEL", "2"),
            ]
        );
        Ok(())
    }

    #[test]
    fn define_in_functions_parse_symbols_returns_it_at_the_top_level() -> anyhow::Result<()> {
        let source = r#"<?php
function init() {
    $local = 1;
    static $cache = 2;
    global $config;
    define('IN_FUNCTION', 1);
    if (!defined('IN_IF')) {
        $inIf = 1;
        define('IN_IF', 2);
    }
    function nested() { define('IN_NESTED', 3); }
    class Local { const IN_CLASS = 4; public function f() { define('IN_LOCAL_METHOD', 5); } }
    foreach ([1] as $i) { define('IN_LOOP', 6); }
    $closure = function () { define('IN_CLOSURE', 7); };
    $arrow = fn () => define('IN_ARROW', 8);
    $object = new class { public function f() { define('IN_ANONYMOUS', 9); } };
}

class Boot {
    public function run() { define('IN_METHOD', 10); }
}
"#;

        assert_eq!(
            values(&php(source)?, source),
            [
                ("/init".to_string(), None),
                value("/IN_FUNCTION", "1"),
                value("/IN_IF", "2"),
                value("/IN_NESTED", "3"),
                ("/Boot".to_string(), None),
                ("/Boot/run".to_string(), None),
                value("/IN_METHOD", "10"),
            ]
        );
        Ok(())
    }

    #[test]
    fn define_without_a_plain_name_parse_symbols_skips_it() -> anyhow::Result<()> {
        let source = r#"<?php
define($name, 1);
define(PREFIX . 'CONCAT', 1);
define("HOLE_$suffix", 1);
define('', 1);
define('ESCAPED\\', 1);
define();
define("$whole", 1);
define(`command`, 1);
define(constant_name: 'NAMED', value: 1);
Other\define('QUALIFIED', 1);
Util::define('STATIC_CALL', 1);
$define('VARIABLE_CALL', 1);
$object->define('METHOD_CALL', 1);
defined('OR_FORM') or define('OR_FORM', 1);
"#;

        assert_eq!(values(&php(source)?, source), []);
        Ok(())
    }

    #[test]
    fn assignment_to_anything_but_a_variable_parse_symbols_skips_it() -> anyhow::Result<()> {
        let source = r#"<?php
$items['key'] = 1;
$object->property = 1;
Config::$shared = 1;
list($first) = [1];
[$second] = [1];
$$dynamic = 1;
$text .= 'more';
$alias = &$items;
"#;

        assert_eq!(values(&php(source)?, source), []);
        Ok(())
    }

    #[test]
    fn chained_assignment_parse_symbols_returns_the_first_variable() -> anyhow::Result<()> {
        let source = r#"<?php
$a = $b = 1;
"#;

        assert_eq!(values(&php(source)?, source), [value("/$a", "$b = 1")]);
        Ok(())
    }

    #[test]
    fn template_with_html_parse_symbols_returns_the_php_definitions() -> anyhow::Result<()> {
        let source = r#"<html>
<?php $title = 'Home'; ?>
<h1><?= $title ?></h1>
<?php if ($show): ?>
<p><?php define('SHOWN', 1); ?></p>
<?php endif; ?>
</html>
"#;

        assert_eq!(
            values(&php(source)?, source),
            [value("/$title", "Home"), value("/SHOWN", "1")]
        );
        Ok(())
    }

    #[test]
    fn syntax_error_parse_symbols_makes_the_definitions_that_hold_it_unreadable()
    -> anyhow::Result<()> {
        let source = r#"<?php
class Cart {
    public $items = [];
    public function broken() { $x = ; }
    public $total = 0;
}

$after = 5;
"#;

        assert_eq!(
            symbols(&php(source)?, source),
            vec![
                symbol("/Cart/$items", "public $items = [];", Some("[]")),
                symbol("/Cart/$total", "public $total = 0;", Some("0")),
                symbol("/$after", "$after = 5;", Some("5")),
            ]
        );
        assert_eq!(
            unresolved(&php(source)?, source, "/Cart")?,
            syntax_error(4, 35, "Cart")
        );
        assert_eq!(
            unresolved(&php(source)?, source, "/Cart/broken")?,
            syntax_error(4, 35, "broken")
        );
        Ok(())
    }

    #[test]
    fn variable_in_a_skipped_span_parse_symbols_makes_it_unreadable() -> anyhow::Result<()> {
        // The parser skips `$x = ) $total`.
        let source = r#"<?php
class Cart {
    public function f() { $x = ) $total }
}
"#;

        assert_eq!(
            unresolved(&php(source)?, source, "/Cart/$total")?,
            syntax_error(3, 27, "$total")
        );
        Ok(())
    }

    #[test]
    fn readonly_parameter_without_visibility_parse_symbols_makes_the_class_unreadable()
    -> anyhow::Result<()> {
        // PHP reads `readonly int $id` as a property. The parser reads `readonly` as a type and
        // skips `int`, so the class holds a syntax error.
        let source = r#"<?php
class Order {
    public function __construct(readonly int $id = 3) {}
}
"#;

        assert_eq!(
            unresolved(&php(source)?, source, "/Order")?,
            syntax_error(3, 42, "Order")
        );
        Ok(())
    }

    #[test]
    fn string_kinds_parse_symbols_returns_decoded_values() -> anyhow::Result<()> {
        let source = r#"<?php
const SINGLE = 'it\'s \\ a\d \n';
const DOUBLE = "\n\r\t\v\e\f\\\$\"\1012\0\x414\x4\u{1F600}\q\x\u";
const BYTES = "\xC3\xA9";
const LITERAL = "a $ b {";
const HEREDOC = <<<EOT
    one \"q\" \x41
      two\t
    EOT;
const QUOTED = <<<"EOT"
  a
  EOT;
const NOWDOC = <<<'EOT'
    raw \n $x
      more
    EOT;
const EMPTY_HEREDOC = <<<EOT
EOT;
const EMPTY_SINGLE = '';
const EMPTY_DOUBLE = "";
"#;

        assert_eq!(
            values(&php(source)?, source),
            [
                value("/SINGLE", r"it's \ a\d \n"),
                value(
                    "/DOUBLE",
                    "\n\r\t\u{b}\u{1b}\u{c}\\$\"A2\0A4\u{4}😀\\q\\x\\u"
                ),
                value("/BYTES", "é"),
                value("/LITERAL", "a $ b {"),
                value("/HEREDOC", "one \\\"q\\\" A\n  two\t"),
                value("/QUOTED", "a"),
                value("/NOWDOC", "raw \\n $x\n  more"),
                value("/EMPTY_HEREDOC", ""),
                value("/EMPTY_SINGLE", ""),
                value("/EMPTY_DOUBLE", ""),
            ]
        );
        Ok(())
    }

    #[test]
    fn heredoc_indent_parse_symbols_removes_the_closing_markers_indent() -> anyhow::Result<()> {
        // The third line of `BLANK` holds 2 spaces and the fourth 6, against an indent of 4. Each
        // line is a string of its own, as editors remove the spaces at the end of a line.
        let source = concat!(
            "<?php\n",
            "const BLANK = <<<EOT\n",
            "    a\n",
            "\n",
            "  \n",
            "      \n",
            "    b\n",
            "    EOT;\n",
            "const TABS = <<<EOT\n",
            "\t\ta\n",
            "\t\tEOT;\n",
        );

        assert_eq!(
            values(&php(source)?, source),
            [value("/BLANK", "a\n\n\n  \nb"), value("/TABS", "a")]
        );
        Ok(())
    }

    #[test]
    fn heredoc_with_crlf_line_ends_parse_symbols_keeps_them() -> anyhow::Result<()> {
        let source = "<?php\r\nconst CRLF = <<<EOT\r\n  a\r\n\r\n  b\r\n  EOT;\r\n";

        assert_eq!(
            values(&php(source)?, source),
            [value("/CRLF", "a\r\n\r\nb")]
        );
        Ok(())
    }

    #[test]
    fn interpolation_parse_symbols_keeps_the_string_as_written() -> anyhow::Result<()> {
        let source = r#"<?php
$name = 'x';
$simple = "a $name";
$braced = "a {$name}";
$dollarBrace = "a ${name}";
$heredoc = <<<EOT
  a $name
  EOT;
$escaped = "a \$name";
"#;

        assert_eq!(
            values(&php(source)?, source),
            [
                value("/$name", "x"),
                value("/$simple", r#""a $name""#),
                value("/$braced", r#""a {$name}""#),
                value("/$dollarBrace", r#""a ${name}""#),
                value("/$heredoc", "<<<EOT\n  a $name\n  EOT"),
                value("/$escaped", "a $name"),
            ]
        );
        Ok(())
    }

    #[test]
    fn string_that_is_not_utf8_parse_symbols_keeps_it_as_written() -> anyhow::Result<()> {
        let source = r#"<?php
const BYTE = "\xFF";
const HALF = "\xC3";
const SURROGATE = "\u{D800}";
"#;

        assert_eq!(
            values(&php(source)?, source),
            [
                value("/BYTE", r#""\xFF""#),
                value("/HALF", r#""\xC3""#),
                value("/SURROGATE", r#""\u{D800}""#),
            ]
        );
        Ok(())
    }

    #[test]
    fn string_that_php_rejects_or_warns_about_parse_symbols_keeps_it_as_written()
    -> anyhow::Result<()> {
        // PHP warns about `OCTAL`, and stops at each of the others with a parse error.
        let source = "<?php
const OCTAL = \"\\400\";
const SIGN = \"\\u{+41}\";
const TOO_BIG = \"\\u{110000}\";
const NOT_HEX = \"\\u{zz}\";
const NO_DIGITS = \"\\u{}\";
const LESS_INDENT = <<<EOT
    a
  b
    EOT;
const MIXED_MARKER = <<<EOT
\t a
\t EOT;
const OTHER_INDENT = <<<EOT
  a
\t\tb
  EOT;
";

        assert_eq!(
            values(&php(source)?, source),
            [
                value("/OCTAL", r#""\400""#),
                value("/SIGN", r#""\u{+41}""#),
                value("/TOO_BIG", r#""\u{110000}""#),
                value("/NOT_HEX", r#""\u{zz}""#),
                value("/NO_DIGITS", r#""\u{}""#),
                value("/LESS_INDENT", "<<<EOT\n    a\n  b\n    EOT"),
                value("/MIXED_MARKER", "<<<EOT\n\t a\n\t EOT"),
                value("/OTHER_INDENT", "<<<EOT\n  a\n\t\tb\n  EOT"),
            ]
        );
        Ok(())
    }

    #[test]
    fn deeply_nested_blocks_parse_symbols_does_not_overflow_stack() -> anyhow::Result<()> {
        // This depth is enough to overflow a recursive walk.
        let depth = 3_000;
        let blocks = format!(
            "{}define('X', 1);\n{}",
            "if (true) {\n".repeat(depth),
            "}\n".repeat(depth)
        );
        let at_top = format!("<?php\n{blocks}");
        let in_function = format!("<?php\nfunction f() {{\n{blocks}}}\n");

        assert_eq!(values(&php(&at_top)?, &at_top), [value("/X", "1")]);
        assert_eq!(
            values(&php(&in_function)?, &in_function),
            [("/f".to_string(), None), value("/X", "1")]
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Position, language_parsers::Comment};

    #[test]
    fn parses_html_comments_in_template_sections_correctly() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser()?;

        let blocks: Vec<Comment> = comments_parser
            .parse(
                r#"<?php
$colors = ['blue']; // php comment
?><!-- same-line html comment -->
<!-- multi-line html comment
spanning two lines -->
<ul>
    <li>blue</li>
</ul>
<!-- html comment before php -->
<?php # trailing php comment ?>
"#,
            )
            .collect();

        assert_eq!(
            blocks,
            vec![
                Comment {
                    position_range: Position::new(2, 21)..Position::new(2, 35),
                    source_range: 26..40,
                    comment_text: "   php comment".to_string()
                },
                Comment {
                    position_range: Position::new(3, 3)..Position::new(3, 34),
                    source_range: 43..74,
                    comment_text: "     same-line html comment    ".to_string()
                },
                Comment {
                    position_range: Position::new(4, 1)..Position::new(5, 23),
                    source_range: 75..126,
                    comment_text: "     multi-line html comment\nspanning two lines    "
                        .to_string()
                },
                Comment {
                    position_range: Position::new(9, 1)..Position::new(9, 33),
                    source_range: 156..188,
                    comment_text: "     html comment before php    ".to_string()
                },
                Comment {
                    position_range: Position::new(10, 7)..Position::new(10, 30),
                    source_range: 195..218,
                    comment_text: "  trailing php comment ".to_string()
                },
            ]
        );

        Ok(())
    }

    #[test]
    fn html_comment_after_a_php_island_holding_non_ascii_reports_a_character_column()
    -> anyhow::Result<()> {
        let mut parser = parser()?;
        // PHP regions are blanked byte by byte to build the HTML view, which turns one multi-byte
        // character into several spaces; the column must still count the source's characters.
        let content =
            "<?php $x = \"café\"; ?><!-- <block name=\"x\"> -->\n<li>a</li>\n<!-- </block> -->\n";

        let blocks = parser
            .parse_blocks(content)
            .collect::<anyhow::Result<Vec<_>>>()?;

        assert_eq!(
            blocks[0].start_tag_position_range.start,
            Position::new(1, 27)
        );
        Ok(())
    }

    #[test]
    fn extracts_html_comment_split_by_a_php_island() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser()?;

        let comments: Vec<Comment> = comments_parser
            .parse("<ul>\n<!-- <block name=\"x\"> <?php echo 1; ?> </block>-->\n</ul>\n")
            .collect();

        assert_eq!(
            comments,
            vec![Comment {
                position_range: Position::new(2, 1)..Position::new(2, 51),
                source_range: 5..55,
                comment_text: "     <block name=\"x\">                  </block>   ".to_string()
            }]
        );

        Ok(())
    }

    #[test]
    fn parses_php_comments_correctly() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser()?;

        let blocks: Vec<Comment> = comments_parser
            .parse(
                r#"<?php
            // This is a single-line comment in PHP
    
            /*
             * This is a multi-line comment.
             * It spans multiple lines in PHP.
             */
    
            function main() {
                echo "Hello, PHP!"; # Prints a message to the console.
    
                /* Another comment
                 * split into
                 * multiple lines.
                 */
                 
                return 0;
            }
            ?>
            <h1>This is an <?php # inlined comment ?> example</h1>
            <p>The header above will say 'This is an  example'.</p>
<?php
/// Triple slash comment.
?>
            "#,
            )
            .collect();

        assert_eq!(
            blocks,
            vec![
                Comment {
                    position_range: Position::new(2, 13)..Position::new(2, 52),
                    source_range: 18..57,
                    comment_text: "   This is a single-line comment in PHP".to_string()
                },
                Comment {
                    position_range: Position::new(4, 13)..Position::new(7, 16),
                    source_range: 75..185,
                    comment_text:
                        "  \n               This is a multi-line comment.\n               It spans multiple lines in PHP.\n               "
                            .to_string()
                },
                Comment {
                    position_range: Position::new(10, 37)..Position::new(10, 71),
                    source_range: 257..291,
                    comment_text: "  Prints a message to the console.".to_string()
                },
                Comment {
                    position_range: Position::new(12, 17)..Position::new(15, 20),
                    source_range: 313..416,
                    comment_text: "   Another comment\n                   split into\n                   multiple lines.\n                   ".to_string()
                },
                Comment {
                    position_range: Position::new(20, 34)..Position::new(20, 52),
                    source_range: 523..541,
                    comment_text: "  inlined comment ".to_string()
                },
                Comment {
                    position_range: Position::new(23, 1)..Position::new(23, 26),
                    source_range: 631..656,
                    comment_text: "  / Triple slash comment.".to_string()
                },
            ]
        );

        Ok(())
    }
}
