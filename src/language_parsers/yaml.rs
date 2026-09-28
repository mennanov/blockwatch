use crate::Position;
use crate::language_parsers::{
    CommentsParser, LanguageParser, LanguageParserImpl, python_style_comments_parser,
};
use crate::symbol_path::SymbolPath;
use crate::symbols::{Symbol, SymbolsParser, ensure_no_syntax_error};
use anyhow::{Context, bail};
use std::ops::Range;
use tree_sitter::Node;

/// Returns a [`LanguageParser`] for Yaml.
pub(super) fn parser() -> anyhow::Result<impl LanguageParser> {
    Ok(LanguageParserImpl::new(comments_parser()?).with_symbols(YamlSymbolsParser::new()?))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let yaml_language = tree_sitter_yaml::LANGUAGE.into();
    let parser = python_style_comments_parser(&yaml_language, "comment");
    Ok(parser)
}

/// Finds the symbols of a YAML file by walking its syntax tree.
///
/// Every key and every list item is a symbol. Strings are decoded, other values are kept as
/// written. Aliases get no symbol.
struct YamlSymbolsParser {
    parser: tree_sitter::Parser,
}

impl YamlSymbolsParser {
    /// # Errors
    /// Returns an error if the YAML grammar cannot be loaded.
    fn new() -> anyhow::Result<Self> {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_yaml::LANGUAGE.into())
            .context("failed to set tree-sitter language")?;
        Ok(Self { parser })
    }
}

impl SymbolsParser for YamlSymbolsParser {
    /// # Errors
    /// Returns an error if the file has a syntax error, holds several documents, or has a string
    /// YAML can't decode.
    fn parse(&mut self, source: &str) -> anyhow::Result<Vec<Symbol>> {
        // A query can't give an alias a list position without making it a symbol, so the tree is
        // walked by hand.
        let tree = self
            .parser
            .parse(source, None)
            .context("failed to parse syntax tree")?;
        ensure_no_syntax_error(&tree, source)?;

        // The value of each document. Documents without one, such as the empty document after a
        // trailing `---`, are left out, so they don't count as a second document.
        let contents: Vec<Node> = members(tree.root_node())
            .into_iter()
            .filter_map(|document| {
                members(document)
                    .into_iter()
                    .find(|child| matches!(child.kind(), "block_node" | "flow_node"))
            })
            .collect();
        match contents.as_slice() {
            [] => Ok(Vec::new()),
            [content] => Walk::symbols_in(source, *content),
            // A path can't say which document it means.
            [_, _, ..] => bail!(
                "files with several YAML documents are not supported yet; use a named block instead"
            ),
        }
    }
}

/// What a YAML value holds, ignoring its anchor and tag.
enum Content<'tree> {
    Mapping(Node<'tree>),
    Sequence(Node<'tree>),
    Scalar(Node<'tree>, ScalarStyle),
    /// An alias, such as `*defaults`.
    Alias,
    /// Nothing, as after `key:`.
    Empty,
}

impl<'tree> Content<'tree> {
    /// What `node` holds. `node` wraps a value together with its anchor and tag.
    ///
    /// # Errors
    /// Returns an error for a node kind the grammar isn't expected to produce here.
    fn of(node: Node<'tree>) -> anyhow::Result<Self> {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            let content = match child.kind() {
                "anchor" | "tag" => continue,
                _ if child.is_extra() => continue,
                "block_mapping" | "flow_mapping" => Self::Mapping(child),
                "block_sequence" | "flow_sequence" => Self::Sequence(child),
                "plain_scalar" => Self::Scalar(child, ScalarStyle::Plain),
                "single_quote_scalar" => Self::Scalar(child, ScalarStyle::SingleQuoted),
                "double_quote_scalar" => Self::Scalar(child, ScalarStyle::DoubleQuoted),
                "block_scalar" => Self::Scalar(child, ScalarStyle::Block),
                "alias" => Self::Alias,
                other => bail!("unexpected {other} in a YAML {}", node.kind()),
            };
            return Ok(content);
        }
        Ok(Self::Empty)
    }
}

/// How a scalar is written, which decides how to decode it.
#[derive(Clone, Copy)]
enum ScalarStyle {
    Plain,
    SingleQuoted,
    DoubleQuoted,
    /// A `|` or `>` block.
    Block,
}

/// The children of `node`, without comments.
fn members(node: Node) -> Vec<Node> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|child| !child.is_extra())
        .collect()
}

/// The state of a walk over a YAML document.
struct Walk<'s> {
    source: &'s str,
    /// The path to the node being walked.
    path: Vec<String>,
    symbols: Vec<Symbol>,
}

impl<'s> Walk<'s> {
    /// The symbols of everything inside `content`, a document's value in `source`, in document
    /// order.
    ///
    /// # Errors
    /// Returns an error if a string can't be decoded.
    fn symbols_in(source: &'s str, content: Node) -> anyhow::Result<Vec<Symbol>> {
        let mut walk = Walk {
            source,
            path: Vec::new(),
            symbols: Vec::new(),
        };
        walk.add_member_symbols(Content::of(content)?)?;
        Ok(walk.symbols)
    }

    /// Adds a symbol for everything inside `content`.
    fn add_member_symbols(&mut self, content: Content) -> anyhow::Result<()> {
        match content {
            Content::Mapping(mapping) => {
                for entry in members(mapping) {
                    match entry.kind() {
                        "block_mapping_pair" | "flow_pair" => self.add_pair_symbol(entry)?,
                        // A key without a value, such as `a` in `{a, b}`.
                        "flow_node" => self.add_key_symbol(entry, Some(entry), None)?,
                        other => bail!("unexpected {other} in a YAML {}", mapping.kind()),
                    }
                }
            }
            Content::Sequence(sequence) => {
                for (index, item) in members(sequence).into_iter().enumerate() {
                    self.path.push(index.to_string());
                    match item.kind() {
                        "block_sequence_item" => {
                            self.add_value_symbol(item, members(item).first().copied())?;
                        }
                        "flow_node" => self.add_value_symbol(item, Some(item))?,
                        // `[a: 1]` holds a mapping with one pair.
                        "flow_pair" => {
                            let range = self.written_range(item);
                            self.add_symbol(range, None);
                            self.add_pair_symbol(item)?;
                        }
                        other => bail!("unexpected {other} in a YAML {}", sequence.kind()),
                    }
                    self.path.pop();
                }
            }
            Content::Scalar(..) | Content::Alias | Content::Empty => {}
        }
        Ok(())
    }

    /// Adds symbols for `pair` and everything inside it.
    fn add_pair_symbol(&mut self, pair: Node) -> anyhow::Result<()> {
        self.add_key_symbol(
            pair,
            pair.child_by_field_name("key"),
            pair.child_by_field_name("value"),
        )
    }

    /// Adds symbols for the entry `entry` and everything inside its value.
    fn add_key_symbol(
        &mut self,
        entry: Node,
        key: Option<Node>,
        value: Option<Node>,
    ) -> anyhow::Result<()> {
        let key = match key {
            Some(key) => Content::of(key)?,
            None => Content::Empty,
        };
        let name = match key {
            Content::Scalar(scalar, style) => self.decoded(scalar, style)?,
            // A key like `? [a, b]` can't be part of a path, so it is skipped with everything
            // under it.
            Content::Mapping(_) | Content::Sequence(_) | Content::Alias | Content::Empty => {
                return Ok(());
            }
        };
        self.path.push(name);
        self.add_value_symbol(entry, value)?;
        self.path.pop();
        Ok(())
    }

    /// Adds a symbol for `written` at the current path, and symbols for everything inside `value`.
    /// `value` is `None` when nothing follows the key or `-`.
    fn add_value_symbol(&mut self, written: Node, value: Option<Node>) -> anyhow::Result<()> {
        let content = match value {
            Some(value) => Content::of(value)?,
            None => Content::Empty,
        };
        let decoded = match &content {
            Content::Scalar(scalar, style) => Some(self.decoded(*scalar, *style)?),
            Content::Mapping(_) | Content::Sequence(_) => None,
            Content::Empty => Some(String::new()),
            // An alias only points at content elsewhere, so it gets no symbol.
            Content::Alias => return Ok(()),
        };
        let range = self.written_range(written);
        self.add_symbol(range, decoded);
        self.add_member_symbols(content)
    }

    fn add_symbol(&mut self, def_byte_range: Range<usize>, value: Option<String>) {
        self.symbols.push(Symbol {
            path: SymbolPath::from_segments(self.path.clone()),
            def_byte_ranges: vec![def_byte_range],
            value,
        });
    }

    /// Where `node` is written. A node on several lines also covers its last line break.
    fn written_range(&self, node: Node) -> Range<usize> {
        let range = node.byte_range();
        if !self.source[range.clone()].contains('\n') {
            return range;
        }
        // Covering the line break makes deleting a mapping's last line count as a change to it.
        let end = self.source[range.end..]
            .find('\n')
            .map_or(self.source.len(), |offset| range.end + offset + 1);
        range.start..end
    }

    /// The text that `scalar` stands for.
    ///
    /// # Errors
    /// Returns an error if the scalar has an invalid escape. The error shows where it starts.
    fn decoded(&self, scalar: Node, style: ScalarStyle) -> anyhow::Result<String> {
        // The `saphyr-parser` crate was tried instead, but it decodes a scalar read on its own
        // wrongly (`v: ---` gives an empty string), and it is slower.
        let text = &self.source[scalar.byte_range()];
        let decoded = match style {
            ScalarStyle::Plain => Ok(folded(text)),
            ScalarStyle::SingleQuoted => Ok(folded(unquoted(text)).replace("''", "'")),
            ScalarStyle::DoubleQuoted => double_quoted_decoded(unquoted(text)),
            ScalarStyle::Block => Ok(block_scalar_decoded(self.source, scalar)),
        };
        decoded.with_context(|| {
            let position = Position::from_byte_offset(self.source, scalar.start_byte());
            format!(
                "the string at line {}, column {} is not valid YAML",
                position.line, position.character
            )
        })
    }
}

/// `text` without its quotes.
fn unquoted(text: &str) -> &str {
    &text[1..text.len() - 1]
}

/// `text` with its lines joined the way YAML joins them: a line break becomes a space, and an
/// empty line becomes a line break.
fn folded(text: &str) -> String {
    let text = text.replace("\r\n", "\n");
    let lines: Vec<&str> = text.split('\n').collect();
    let last = lines.len() - 1;
    let mut folded = String::new();
    let mut empty_lines = 0;
    for (index, line) in lines.into_iter().enumerate() {
        let line = if index > 0 {
            line.trim_start_matches([' ', '\t'])
        } else {
            line
        };
        let line = if index < last {
            line.trim_end_matches([' ', '\t'])
        } else {
            line
        };
        if index > 0 {
            if line.is_empty() && index < last {
                empty_lines += 1;
                continue;
            }
            folded.push_str(&line_breaks_or_space(empty_lines));
            empty_lines = 0;
        }
        folded.push_str(line);
    }
    folded
}

/// A space, or one line break per empty line.
fn line_breaks_or_space(empty_lines: usize) -> String {
    if empty_lines == 0 {
        " ".to_string()
    } else {
        "\n".repeat(empty_lines)
    }
}

/// `text`, a double-quoted string without its quotes, with its escapes decoded and its lines
/// joined.
///
/// # Errors
/// Returns an error for an invalid escape.
fn double_quoted_decoded(text: &str) -> anyhow::Result<String> {
    let text = text.replace("\r\n", "\n");
    let mut chars = text.chars().peekable();
    let mut decoded = String::new();
    // An escaped space is kept, so trimming before a line break stops here.
    let mut escaped_end = 0;
    while let Some(char) = chars.next() {
        match char {
            '\\' => {
                let escape = chars.next().context("a string ends in `\\`")?;
                let unescaped = match escape {
                    // An escaped line break joins lines without a space.
                    '\n' => {
                        let empty_lines = skip_line_prefixes(&mut chars);
                        decoded.push_str(&"\n".repeat(empty_lines));
                        escaped_end = decoded.len();
                        continue;
                    }
                    '0' => '\0',
                    'a' => '\u{07}',
                    'b' => '\u{08}',
                    't' | '\t' => '\t',
                    'n' => '\n',
                    'v' => '\u{0B}',
                    'f' => '\u{0C}',
                    'r' => '\r',
                    'e' => '\u{1B}',
                    ' ' => ' ',
                    '"' => '"',
                    '/' => '/',
                    '\\' => '\\',
                    'N' => '\u{85}',
                    '_' => '\u{A0}',
                    'L' => '\u{2028}',
                    'P' => '\u{2029}',
                    'x' => hex_escape(&mut chars, 2)?,
                    'u' => hex_escape(&mut chars, 4)?,
                    'U' => hex_escape(&mut chars, 8)?,
                    other => bail!("`\\{other}` is not an escape"),
                };
                decoded.push(unescaped);
                escaped_end = decoded.len();
            }
            '\n' => {
                let kept = decoded[escaped_end..].trim_end_matches([' ', '\t']).len();
                decoded.truncate(escaped_end + kept);
                let empty_lines = skip_line_prefixes(&mut chars);
                decoded.push_str(&line_breaks_or_space(empty_lines));
            }
            other => decoded.push(other),
        }
    }
    Ok(decoded)
}

/// Skips the indentation of the next line and any empty lines before it. Returns how many empty
/// lines were skipped.
fn skip_line_prefixes(chars: &mut std::iter::Peekable<std::str::Chars>) -> usize {
    let mut empty_lines = 0;
    loop {
        while chars.next_if(|char| matches!(char, ' ' | '\t')).is_some() {}
        if chars.next_if_eq(&'\n').is_none() {
            return empty_lines;
        }
        empty_lines += 1;
    }
}

/// The character the next `digits` hex digits stand for.
///
/// # Errors
/// Returns an error if the digits are missing or stand for no character.
fn hex_escape(chars: &mut impl Iterator<Item = char>, digits: usize) -> anyhow::Result<char> {
    let hex: String = chars.take(digits).collect();
    let code = u32::from_str_radix(&hex, 16)
        .ok()
        .filter(|_| hex.len() == digits)
        .with_context(|| format!("`{hex}` is not {digits} hex digits"))?;
    char::from_u32(code).with_context(|| format!("`{hex}` stands for no character"))
}

/// The text of a `|` or `>` block.
fn block_scalar_decoded(source: &str, scalar: Node) -> String {
    let start = scalar.start_byte();
    let header_line = source[start..]
        .split_inclusive('\n')
        .next()
        .unwrap_or_default();
    let header = BlockHeader::parse(without_line_break(header_line));
    let lines = block_lines(source, start + header_line.len(), scalar.end_byte());
    let indent = block_indent(source, start, header.extra_indent, &lines);

    // The content ends at the last line longer than the indentation.
    let content_end = lines
        .iter()
        .rposition(|line| without_line_break(line).len() > indent)
        .map_or(0, |last| last + 1);
    let (content, trailing) = lines.split_at(content_end);
    let text_lines: Vec<&str> = content
        .iter()
        .map(|line| without_line_break(line).get(indent..).unwrap_or_default())
        .collect();
    let text = match header.style {
        BlockStyle::Literal => text_lines.join("\n"),
        BlockStyle::Folded => folded_block_lines(&text_lines),
    };

    let ends_in_line_break = content.last().is_some_and(|line| line.ends_with('\n'));
    let empty_lines = trailing.iter().filter(|line| line.ends_with('\n')).count();
    header.chomping.apply(text, ends_in_line_break, empty_lines)
}

/// The header of a `|` or `>` block, such as `|-` or `>2`.
struct BlockHeader {
    style: BlockStyle,
    chomping: Chomping,
    /// How much deeper than the header's line the content is indented, if the header says.
    extra_indent: Option<usize>,
}

impl BlockHeader {
    /// Reads `header`, the first line of a block.
    fn parse(header: &str) -> Self {
        let style = if header.starts_with('|') {
            BlockStyle::Literal
        } else {
            BlockStyle::Folded
        };
        let indicators = header[1..]
            .split([' ', '\t', '#'])
            .next()
            .unwrap_or_default();
        let chomping = if indicators.contains('-') {
            Chomping::Strip
        } else if indicators.contains('+') {
            Chomping::Keep
        } else {
            Chomping::Clip
        };
        let extra_indent = indicators
            .chars()
            .find_map(|char| char.to_digit(10))
            .map(|digit| digit as usize);
        Self {
            style,
            chomping,
            extra_indent,
        }
    }
}

/// How a block treats the line breaks inside it.
enum BlockStyle {
    /// `|`: keeps them.
    Literal,
    /// `>`: turns most of them into spaces.
    Folded,
}

/// What to do with the line breaks at the end of a block.
enum Chomping {
    /// `-`: drop them.
    Strip,
    /// No indicator: keep one.
    Clip,
    /// `+`: keep them all.
    Keep,
}

impl Chomping {
    /// `text` with the line breaks this keeps at its end. `ends_in_line_break` says whether the
    /// last line of text has one, and `empty_lines` how many empty lines follow it.
    fn apply(&self, mut text: String, ends_in_line_break: bool, empty_lines: usize) -> String {
        match self {
            Chomping::Strip => {}
            Chomping::Clip => {
                if ends_in_line_break {
                    text.push('\n');
                }
            }
            Chomping::Keep => {
                if ends_in_line_break {
                    text.push('\n');
                }
                text.push_str(&"\n".repeat(empty_lines));
            }
        }
        text
    }
}

/// The lines of a block from `offset` in `source`, each with its line break. They run to the
/// block's `end` and then on through the empty lines after it, since `+` keeps those.
fn block_lines(source: &str, mut offset: usize, end: usize) -> Vec<&str> {
    let mut lines = Vec::new();
    for line in source[offset..].split_inclusive('\n') {
        if offset >= end && !line.trim().is_empty() {
            break;
        }
        lines.push(line);
        offset += line.len();
    }
    lines
}

/// How many spaces indent the content of a block whose header starts at `start` in `source`.
/// `extra_indent` is the header's digit, if any, and `lines` are the block's lines.
fn block_indent(source: &str, start: usize, extra_indent: Option<usize>, lines: &[&str]) -> usize {
    match extra_indent {
        // The digit counts from the indentation of the header's line.
        Some(extra) => line_indent(source[..start].rsplit('\n').next().unwrap_or_default()) + extra,
        // Otherwise the first line with text sets it.
        None => lines
            .iter()
            .find(|line| !line.trim().is_empty())
            .map_or(0, |line| line_indent(line)),
    }
}

/// `line` without its line break.
fn without_line_break(line: &str) -> &str {
    line.strip_suffix('\n')
        .map_or(line, |line| line.strip_suffix('\r').unwrap_or(line))
}

/// How many spaces start `line`.
fn line_indent(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

/// The lines of a `>` block, joined: a line break becomes a space, except next to a line that
/// starts with a space.
fn folded_block_lines(lines: &[&str]) -> String {
    let mut folded = String::new();
    let mut empty_lines = 0;
    let mut previous_more_indented = None;
    for line in lines {
        if line.is_empty() {
            empty_lines += 1;
            continue;
        }
        let more_indented = line.starts_with([' ', '\t']);
        let separator = match previous_more_indented {
            // Empty lines before the first text are kept.
            None => "\n".repeat(empty_lines),
            Some(false) if !more_indented => line_breaks_or_space(empty_lines),
            Some(_) => "\n".repeat(empty_lines + 1),
        };
        folded.push_str(&separator);
        folded.push_str(line);
        empty_lines = 0;
        previous_more_indented = Some(more_indented);
    }
    folded
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Position, language_parsers::Comment};

    #[test]
    fn source_with_comments_parse_returns_every_comment() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser()?;

        let blocks: Vec<Comment> = comments_parser
            .parse(
                r#"
# This is a YAML comment
key: value  # Inline comment on a key-value pair

# Another comment
list:
  - item1  # Comment in a list
  - item2
# End of comments
"#,
            )
            .collect();

        assert_eq!(
            blocks,
            vec![
                Comment {
                    position_range: Position::new(2, 1)..Position::new(2, 25),
                    source_range: 1..25,
                    comment_text: "  This is a YAML comment".to_string()
                },
                Comment {
                    position_range: Position::new(3, 13)..Position::new(3, 49),
                    source_range: 38..74,
                    comment_text: "  Inline comment on a key-value pair".to_string()
                },
                Comment {
                    position_range: Position::new(5, 1)..Position::new(5, 18),
                    source_range: 76..93,
                    comment_text: "  Another comment".to_string()
                },
                Comment {
                    position_range: Position::new(7, 12)..Position::new(7, 31),
                    source_range: 111..130,
                    comment_text: "  Comment in a list".to_string()
                },
                Comment {
                    position_range: Position::new(9, 1)..Position::new(9, 18),
                    source_range: 141..158,
                    comment_text: "  End of comments".to_string()
                }
            ]
        );

        Ok(())
    }

    /// A symbol as `(path, value, the text of its range)`.
    type DerivedSymbol = (String, Option<String>, String);

    /// The symbols `source` derives, in order.
    fn derived_symbols(source: &str) -> anyhow::Result<Vec<DerivedSymbol>> {
        Ok(parser()?
            .parse_symbols(source)?
            .into_iter()
            .map(|symbol| {
                let [range] = symbol.def_byte_ranges.as_slice() else {
                    panic!(
                        "{} has {} ranges",
                        symbol.path,
                        symbol.def_byte_ranges.len()
                    );
                };
                (
                    symbol.path.to_string(),
                    symbol.value,
                    source[range.clone()].to_string(),
                )
            })
            .collect())
    }

    /// The same symbols, written as string literals.
    fn expected_symbols(symbols: &[(&str, Option<&str>, &str)]) -> Vec<DerivedSymbol> {
        symbols
            .iter()
            .map(|(path, value, text)| {
                (
                    path.to_string(),
                    value.map(str::to_string),
                    text.to_string(),
                )
            })
            .collect()
    }

    /// The value of the first symbol at `path` in `source`.
    fn value_at(source: &str, path: &str) -> anyhow::Result<Option<String>> {
        Ok(derived_symbols(source)?
            .into_iter()
            .find(|(derived, _, _)| derived == path)
            .and_then(|(_, value, _)| value))
    }

    #[test]
    fn document_with_every_yaml_shape_parse_symbols_returns_every_symbol_in_order()
    -> anyhow::Result<()> {
        let source = r#"# A comment before any key
plain: hello world
"double": "tab\there \u00e9"
'single': 'it''s'
empty:
number: 1_000
tagged: !!str 123
tag_only: !!null
anchored: &a 5
alias: *a
nested:
  key: value
list:
  # A comment takes no position
  - first
  - *a
  - name: item
flow: {x: 1, "y": [2, *a, 3]}
pairs: [k: v]
keys: {bare, j: 1}
literal: |
  line one
  line two
folded: >-
  folded
  text
multi: this is
  one line
<<: {merged: 1}
? [complex, key]
: skipped
last: end
"#;

        // <block affects="docs/symbols.md:yaml-paths, .agents/skills/blockwatch/SKILL.md:yaml-paths">
        assert_eq!(
            derived_symbols(source)?,
            expected_symbols(&[
                ("/plain", Some("hello world"), "plain: hello world"),
                (
                    "/double",
                    Some("tab\there é"),
                    r#""double": "tab\there \u00e9""#
                ),
                ("/single", Some("it's"), "'single': 'it''s'"),
                ("/empty", Some(""), "empty:"),
                ("/number", Some("1_000"), "number: 1_000"),
                ("/tagged", Some("123"), "tagged: !!str 123"),
                ("/tag_only", Some(""), "tag_only: !!null"),
                ("/anchored", Some("5"), "anchored: &a 5"),
                ("/nested", None, "nested:\n  key: value\n"),
                ("/nested/key", Some("value"), "key: value"),
                (
                    "/list",
                    None,
                    "list:\n  # A comment takes no position\n  - first\n  - *a\n  - name: item\n"
                ),
                ("/list/0", Some("first"), "- first"),
                ("/list/2", None, "- name: item"),
                ("/list/2/name", Some("item"), "name: item"),
                ("/flow", None, r#"flow: {x: 1, "y": [2, *a, 3]}"#),
                ("/flow/x", Some("1"), "x: 1"),
                ("/flow/y", None, r#""y": [2, *a, 3]"#),
                ("/flow/y/0", Some("2"), "2"),
                ("/flow/y/2", Some("3"), "3"),
                ("/pairs", None, "pairs: [k: v]"),
                ("/pairs/0", None, "k: v"),
                ("/pairs/0/k", Some("v"), "k: v"),
                ("/keys", None, "keys: {bare, j: 1}"),
                ("/keys/bare", Some(""), "bare"),
                ("/keys/j", Some("1"), "j: 1"),
                (
                    "/literal",
                    Some("line one\nline two\n"),
                    "literal: |\n  line one\n  line two\n"
                ),
                (
                    "/folded",
                    Some("folded text"),
                    "folded: >-\n  folded\n  text\n"
                ),
                (
                    "/multi",
                    Some("this is one line"),
                    "multi: this is\n  one line\n"
                ),
                ("/<<", None, "<<: {merged: 1}"),
                ("/<</merged", Some("1"), "merged: 1"),
                ("/last", Some("end"), "last: end"),
            ])
        );
        // </block>
        Ok(())
    }

    #[test]
    fn block_and_flow_spellings_parse_symbols_derive_the_same_symbols() -> anyhow::Result<()> {
        let spellings = [
            "image:\n  tags:\n    - '1.0'\n    - latest\n",
            "image: {tags: ['1.0', latest]}\n",
            "image:\n  tags: [\"1.0\", latest]\n",
            "{image: {tags: [1.0, latest]}}\n",
        ];

        for source in spellings {
            let values: Vec<(String, Option<String>)> = derived_symbols(source)?
                .into_iter()
                .map(|(path, value, _)| (path, value))
                .collect();
            assert_eq!(
                values,
                vec![
                    ("/image".to_string(), None),
                    ("/image/tags".to_string(), None),
                    ("/image/tags/0".to_string(), Some("1.0".to_string())),
                    ("/image/tags/1".to_string(), Some("latest".to_string())),
                ],
                "{source:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn every_string_style_parse_symbols_returns_the_text_yaml_reads() -> anyhow::Result<()> {
        let cases = [
            ("v: a\n  b\n\n  c\n", "/v", "a b\nc"),
            // Only at the start of a line are these document markers.
            ("v: ---\n", "/v", "---"),
            ("v: ...\n", "/v", "..."),
            ("v: 'it''s\n  folded'\n", "/v", "it's folded"),
            (
                r#"v: "\x41\u00e9\U0001F600\N\_\t\ \"\\\/""#,
                "/v",
                "A\u{e9}\u{1F600}\u{85}\u{a0}\t \"\\/",
            ),
            (
                r#"v: "\0\a\b\n\v\f\r\e\L\P""#,
                "/v",
                "\0\u{7}\u{8}\n\u{b}\u{c}\r\u{1b}\u{2028}\u{2029}",
            ),
            ("v: \"a\\\n  b\"\n", "/v", "ab"),
            ("v: \"a\\ \n  b\"\n", "/v", "a  b"),
            ("v: \"a   \n\n  b\"\n", "/v", "a\nb"),
            ("v: |+\n  a\n\n\nw: 1\n", "/v", "a\n\n\n"),
            ("v: |-\n  a\n\nw: 1\n", "/v", "a"),
            ("v: |\n\n  a\n", "/v", "\na\n"),
            (
                "v: >\n  a\n  b\n\n    code\n  c\n",
                "/v",
                "a b\n\n  code\nc\n",
            ),
            ("v: |2\n    x\n", "/v", "  x\n"),
            ("a:\n  v: |1\n    x\n", "/a/v", " x\n"),
            ("v: |\n  a\n    \nw: 1\n", "/v", "a\n  \n"),
            ("v: |\n  a\n  \nw: 1\n", "/v", "a\n"),
            ("v: >-\n  a \nw: 1\n", "/v", "a "),
            ("v: |\r\n  a\r\n  b\r\n", "/v", "a\nb\n"),
            ("v: a\r\n  b\r\n", "/v", "a b"),
        ];

        for (source, path, expected) in cases {
            assert_eq!(
                value_at(source, path)?.as_deref(),
                Some(expected),
                "{source:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn block_scalar_at_the_end_of_a_file_without_a_line_break_parse_symbols_keeps_none()
    -> anyhow::Result<()> {
        // As in libyaml, no line break is added when the file has none.
        assert_eq!(value_at("v: |\n  a", "/v")?.as_deref(), Some("a"));
        Ok(())
    }

    #[test]
    fn duplicate_key_parse_symbols_returns_one_symbol_each() -> anyhow::Result<()> {
        let paths: Vec<String> = derived_symbols("v: 1\nv: 2\n")?
            .into_iter()
            .map(|(path, _, _)| path)
            .collect();

        assert_eq!(paths, vec!["/v", "/v"]);
        Ok(())
    }

    #[test]
    fn documents_without_content_parse_symbols_do_not_count() -> anyhow::Result<()> {
        for source in ["---\nv: 1\n---\n", "# only a comment\n---\nv: 1\n...\n"] {
            assert_eq!(value_at(source, "/v")?.as_deref(), Some("1"), "{source:?}");
        }
        assert_eq!(derived_symbols("")?, Vec::new());
        Ok(())
    }

    #[test]
    fn several_documents_parse_symbols_returns_error() -> anyhow::Result<()> {
        let err = parser()?.parse_symbols("v: 1\n---\nv: 2\n").unwrap_err();

        assert_eq!(
            err.to_string(),
            "files with several YAML documents are not supported yet; use a named block instead"
        );
        Ok(())
    }

    #[test]
    fn syntax_error_parse_symbols_returns_error_with_its_position() -> anyhow::Result<()> {
        let err = parser()?.parse_symbols("v: [1, 2\nw: 3\n").unwrap_err();

        assert!(
            err.to_string()
                .starts_with("file has a syntax error at line "),
            "unexpected error message: {err}"
        );
        Ok(())
    }

    #[test]
    fn escape_for_no_character_parse_symbols_returns_error_with_its_position() -> anyhow::Result<()>
    {
        let err = parser()?.parse_symbols("v: \"\\ud800\"\n").unwrap_err();

        assert_eq!(
            format!("{err:#}"),
            "the string at line 1, column 4 is not valid YAML: `d800` stands for no character"
        );
        Ok(())
    }
}
