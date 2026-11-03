use crate::language_parsers;
use crate::language_parsers::{CommentsParser, LanguageParser, LanguageParserImpl, node_text};
use crate::symbols::code::{self, CodeLanguage, CodeSymbolsParser, Definition, child};
use anyhow::bail;
use std::str::Chars;
use tree_sitter::Node;

/// Returns a [`LanguageParser`] for HCL (Terraform), with its symbols.
pub(super) fn parser() -> anyhow::Result<impl LanguageParser> {
    Ok(
        LanguageParserImpl::new(comments_parser()?).with_symbols(CodeSymbolsParser::new(
            &tree_sitter_hcl::LANGUAGE.into(),
            Hcl,
        )?),
    )
}

/// HCL, whose files hold attributes and blocks, and whose blocks hold their own attributes and
/// blocks. Terraform, Terragrunt, Nomad and Packer files are HCL.
struct Hcl;

impl CodeLanguage for Hcl {
    /// The definitions under `root`, the root of an HCL file's syntax tree, in document order:
    /// every block, attribute and object key, at any depth. A definition comes before its
    /// members. A block's path is its type, then each of its labels.
    fn definitions<'tree>(&self, root: Node<'tree>, source: &str) -> Vec<Definition<'tree>> {
        let Some(body) = child(root, "body") else {
            return Vec::new();
        };
        code::nested_definitions(body, |body, parents, definitions, bodies| {
            push_members(body, parents, source, definitions, bodies);
        })
    }

    /// The text of `node` when it is a quoted string or a heredoc without `${...}` or `%{...}`,
    /// with its escapes decoded. A `<<-` heredoc loses its indent. Returns `None` for any other
    /// node.
    ///
    /// # Errors
    /// Returns an error that shows the escape when a `\u` or `\U` escape stands for no character.
    fn decode_string(&self, node: &Node, source: &str) -> anyhow::Result<Option<String>> {
        match node.kind() {
            "literal_value" => child(*node, "string_lit")
                .and_then(|string| {
                    node_text(&string, source)
                        .strip_prefix('"')?
                        .strip_suffix('"')
                })
                .map(|content| decode(content, true))
                .transpose(),
            // A quoted string with `${...}` or `%{...}` is a `quoted_template`, which stays as
            // written.
            "template_expr" => child(*node, "heredoc_template")
                .and_then(|heredoc| heredoc_text(heredoc, source))
                .map(|text| decode(&text, false))
                .transpose(),
            _ => Ok(None),
        }
    }

    const NAME_QUOTE: Option<char> = Some('"');
}

/// Adds the definitions of the members of `body` to `definitions`. `body` is the body of a file
/// or a block, or an object. `parents` are the segments of the path it sits at, and `source` is
/// the text of the file. Adds each body and object in `body` to `bodies`, with its own parents.
fn push_members<'tree>(
    body: Node<'tree>,
    parents: Vec<Node<'tree>>,
    source: &str,
    definitions: &mut Vec<Definition<'tree>>,
    bodies: &mut Vec<(Node<'tree>, Vec<Node<'tree>>)>,
) {
    let mut cursor = body.walk();
    for member in body.named_children(&mut cursor) {
        let (name, value) = match member.kind() {
            "attribute" => (
                member.named_child(0),
                child(member, "expression").map(value),
            ),
            "object_elem" => (
                member
                    .child_by_field_name("key")
                    .and_then(|key| key_name(key, source)),
                member.child_by_field_name("val").map(value),
            ),
            "block" => {
                push_block(member, &parents, source, definitions, bodies);
                continue;
            }
            // Comments, the braces of an object, and the text the parser skipped define no name.
            _ => continue,
        };
        let Some(name) = name else {
            continue;
        };
        definitions.push(Definition::new(parents.clone(), name, range(member), value));
        if let Some(object) = value
            .filter(|value| value.kind() == "collection_value")
            .and_then(|value| child(value, "object"))
        {
            bodies.push((object, [parents.as_slice(), &[name]].concat()));
        }
    }
}

/// Adds the definition of `block`, a block in the body at `parents`, to `definitions`, and its
/// body to `bodies`. A block with a label that holds an escape gives no definition, and neither
/// does anything in it.
fn push_block<'tree>(
    block: Node<'tree>,
    parents: &[Node<'tree>],
    source: &str,
    definitions: &mut Vec<Definition<'tree>>,
    bodies: &mut Vec<(Node<'tree>, Vec<Node<'tree>>)>,
) {
    let mut cursor = block.walk();
    // The type comes first, then the labels, up to the `{`.
    let names: Vec<Node> = block
        .named_children(&mut cursor)
        .take_while(|node| matches!(node.kind(), "identifier" | "string_lit"))
        .collect();
    // blockwatch doesn't decode a label. Kept as written, it would give a path that HCL reads
    // differently, and a path fixed now can't change later without breaking configs.
    if names.iter().any(|name| has_escape(node_text(name, source))) {
        return;
    }
    let Some((&name, labels)) = names.split_last() else {
        return;
    };
    definitions.push(Definition::new(
        [parents, labels].concat(),
        name,
        block.byte_range(),
        None,
    ));
    if let Some(body) = child(block, "body") {
        bodies.push((body, [parents, &names].concat()));
    }
}

/// The name that `key`, the key of an object's item, gives the item: a name such as `Team`, or a
/// string such as `"cost center"`. Returns `None` for any other key. HCL works out the key from
/// an expression such as `(var.k)` or `"${x}"`, and turns a number such as `3` into text, which
/// blockwatch doesn't do.
fn key_name<'tree>(key: Node<'tree>, source: &str) -> Option<Node<'tree>> {
    // A key in parentheses is an expression, even when it is a name: `(k)` is the value of `k`.
    // So a key is a name only when it is the only thing in its `expression`.
    let mut cursor = key.walk();
    let mut parts = key.children(&mut cursor);
    let (Some(only), None) = (parts.next(), parts.next()) else {
        return None;
    };
    match only.kind() {
        "variable_expr" => child(only, "identifier"),
        "literal_value" => {
            child(only, "string_lit").filter(|name| !has_escape(node_text(name, source)))
        }
        _ => None,
    }
}

/// `expression`, the value of an attribute or a key, without the `expression` nodes and the
/// parentheses around it. The grammar puts every value in an `expression`, and writes `(3)` as
/// an `expression` that holds `(`, an `expression` and `)`.
fn value(mut expression: Node<'_>) -> Node<'_> {
    while expression.kind() == "expression" {
        let mut cursor = expression.walk();
        // A comment inside the parentheses is a child too, but not part of the value.
        let mut parts = expression
            .children(&mut cursor)
            .filter(|part| !part.is_extra() && !matches!(part.kind(), "(" | ")"));
        match (parts.next(), parts.next()) {
            (Some(only), None) => expression = only,
            _ => break,
        }
    }
    expression
}

/// The byte range of `member`, an attribute or an object's item. It also covers the text that
/// the parser skipped right after it, on the same line.
fn range(member: Node) -> std::ops::Range<usize> {
    // In `x = 1 +`, the parser reads `x = 1` and skips the `+` after it. The `+` belongs to the
    // value, so it makes the value unreadable. When `x = 1` ends its body, the skipped text comes
    // after the body.
    let mut node = member;
    let next = loop {
        match node.next_sibling() {
            Some(next) => break Some(next),
            None => match node.parent() {
                Some(parent) => node = parent,
                None => break None,
            },
        }
    };
    match next {
        Some(next) if next.is_error() && next.start_position().row == member.end_position().row => {
            member.start_byte()..next.end_byte()
        }
        _ => member.byte_range(),
    }
}

/// Whether `string`, a quoted string as written, holds an escape: a `\`, `$${` or `%%{`.
fn has_escape(string: &str) -> bool {
    string.contains('\\') || string.contains("$${") || string.contains("%%{")
}

/// The text of `heredoc`, a heredoc in `source`, as written between its markers. A `<<-` heredoc
/// loses its indent. Returns `None` when the heredoc holds `${...}` or `%{...}`, or lacks a marker.
fn heredoc_text(heredoc: Node, source: &str) -> Option<String> {
    let mut cursor = heredoc.walk();
    let mut markers = Vec::new();
    for part in heredoc.named_children(&mut cursor) {
        match part.kind() {
            "template_interpolation" | "template_directive" => return None,
            "heredoc_identifier" => markers.push(part),
            _ => {}
        }
    }
    let [opening, closing] = markers[..] else {
        return None;
    };
    // The grammar's text node leaves out the indent of the first line and the last line end, so
    // the text is read from the line after the opening marker up to the line of the closing one.
    let start = opening.end_byte() + source.get(opening.end_byte()..)?.find('\n')? + 1;
    let end = closing.start_byte() - closing.start_position().column;
    let text = source.get(start..end)?;
    let flush =
        child(heredoc, "heredoc_start").is_some_and(|start| node_text(&start, source) == "<<-");
    Some(if flush {
        without_indent(text)
    } else {
        text.to_string()
    })
}

/// `text`, the lines of a `<<-` heredoc, each without as much leading white space as the least
/// indented line has. A line of only white space doesn't count, and stays as it is.
fn without_indent(text: &str) -> String {
    // HCL counts the white space in grapheme clusters. They differ from characters only when a
    // combining mark follows the indent, which no heredoc has.
    let is_blank = |line: &str| line.trim_start().is_empty();
    let indent = text
        .split_inclusive('\n')
        .filter(|line| !is_blank(line))
        .map(|line| line.chars().take_while(|c| c.is_whitespace()).count())
        .min()
        .unwrap_or(0);
    text.split_inclusive('\n')
        .flat_map(|line| line.chars().skip(if is_blank(line) { 0 } else { indent }))
        .collect()
}

/// Decodes `content`, the text of a string or a heredoc, as HCL does: `$${` is `${`, and `%%{`
/// is `%{`. With `backslash`, the text is a quoted string, where a backslash starts an escape.
///
/// # Errors
/// Returns an error that shows the escape when an escape is not one of HCL's, or a `\u` or `\U`
/// escape stands for no character.
fn decode(content: &str, backslash: bool) -> anyhow::Result<String> {
    let mut text = String::with_capacity(content.len());
    let mut characters = content.chars();
    while let Some(character) = characters.next() {
        match character {
            '$' | '%'
                if characters
                    .as_str()
                    .strip_prefix(character)
                    .is_some_and(|rest| rest.starts_with('{')) =>
            {
                text.push(character);
                text.push('{');
                characters.nth(1);
            }
            '\\' if backslash => text.push(escaped(&mut characters)?),
            _ => text.push(character),
        }
    }
    Ok(text)
}

/// The character that an escape stands for. `characters` are the characters right after its
/// backslash. Reads the escape from them.
///
/// # Errors
/// Returns an error that shows the escape when it is not one of HCL's, or a `\u` or `\U` escape
/// stands for no character.
fn escaped(characters: &mut Chars) -> anyhow::Result<char> {
    Ok(match characters.next() {
        Some('n') => '\n',
        Some('r') => '\r',
        Some('t') => '\t',
        Some('"') => '"',
        Some('\\') => '\\',
        Some(kind @ ('u' | 'U')) => {
            let length = if kind == 'u' { 4 } else { 8 };
            let digits: String = characters.by_ref().take(length).collect();
            code::char_from_digits(&format!("\\{kind}{digits}"), &digits, 16)?
        }
        // The grammar rejects any other escape, so a file with one has a syntax error.
        other => bail!(
            "`\\{}` is not an escape in HCL",
            other.into_iter().collect::<String>()
        ),
    })
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let hcl_language = tree_sitter_hcl::LANGUAGE.into();
    let parser = language_parsers::hash_and_c_style_comments_parser(&hcl_language, "comment")?;
    Ok(parser)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::symbols::code::CodeSymbolsParser;
    use crate::symbols::code::test_utils::{
        decode_error, symbol, symbols, syntax_error, text_of, unresolved, value, values,
    };
    use crate::symbols::{Symbols, SymbolsParser};
    use crate::{Position, language_parsers::Comment};

    /// The symbols of `source`, an HCL file.
    fn hcl(source: &str) -> anyhow::Result<Symbols> {
        CodeSymbolsParser::new(&tree_sitter_hcl::LANGUAGE.into(), Hcl)?.parse(source)
    }

    #[test]
    fn file_with_every_shape_parse_symbols_returns_every_symbol_in_order() -> anyhow::Result<()> {
        let source = r#"region = "eu-west-1"

terraform {
  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.0"
    }
  }
}

# The region to deploy to.
variable "region" {
  type    = string
  default = "us-east-1"
}

locals {
  name = "app-${var.env}"
  tags = { Team = "core", "cost center" = 42 }
}

resource "aws_instance" web {
  ami   = data.aws_ami.ubuntu.id
  count = (3)
  root_block_device {
    volume_size = 8
  }
  dynamic "ingress" {
    for_each = var.ports
    content {
      from_port = ingress.value
    }
  }
  lifecycle {}
}
"#;
        let aws = text_of(source, "aws = {", "\n    }")?;
        let tags = r#"{ Team = "core", "cost center" = 42 }"#;

        // <block affects="docs/symbols.md:hcl-paths, src/skill.md#/blockwatch/Symbols/HCL">
        assert_eq!(
            symbols(&hcl(source)?, source),
            vec![
                symbol("/region", r#"region = "eu-west-1""#, Some("eu-west-1")),
                symbol("/terraform", text_of(source, "terraform {", "\n}")?, None),
                symbol(
                    "/terraform/required_providers",
                    text_of(source, "required_providers {", "\n  }")?,
                    None
                ),
                symbol(
                    "/terraform/required_providers/aws",
                    aws,
                    Some(aws.trim_start_matches("aws = "))
                ),
                symbol(
                    "/terraform/required_providers/aws/source",
                    r#"source  = "hashicorp/aws""#,
                    Some("hashicorp/aws")
                ),
                symbol(
                    "/terraform/required_providers/aws/version",
                    r#"version = "~> 5.0""#,
                    Some("~> 5.0")
                ),
                symbol(
                    "/variable/region",
                    text_of(source, "variable", "\n}")?,
                    None
                ),
                symbol("/variable/region/type", "type    = string", Some("string")),
                symbol(
                    "/variable/region/default",
                    r#"default = "us-east-1""#,
                    Some("us-east-1")
                ),
                symbol("/locals", text_of(source, "locals {", "\n}")?, None),
                symbol(
                    "/locals/name",
                    r#"name = "app-${var.env}""#,
                    Some(r#""app-${var.env}""#)
                ),
                symbol("/locals/tags", &format!("tags = {tags}"), Some(tags)),
                symbol("/locals/tags/Team", r#"Team = "core""#, Some("core")),
                symbol(
                    "/locals/tags/cost center",
                    r#""cost center" = 42"#,
                    Some("42")
                ),
                symbol(
                    "/resource/aws_instance/web",
                    text_of(source, "resource", "\n}")?,
                    None
                ),
                symbol(
                    "/resource/aws_instance/web/ami",
                    "ami   = data.aws_ami.ubuntu.id",
                    Some("data.aws_ami.ubuntu.id")
                ),
                symbol("/resource/aws_instance/web/count", "count = (3)", Some("3")),
                symbol(
                    "/resource/aws_instance/web/root_block_device",
                    text_of(source, "root_block_device {", "\n  }")?,
                    None
                ),
                symbol(
                    "/resource/aws_instance/web/root_block_device/volume_size",
                    "volume_size = 8",
                    Some("8")
                ),
                symbol(
                    "/resource/aws_instance/web/dynamic/ingress",
                    text_of(source, "dynamic", "\n  }")?,
                    None
                ),
                symbol(
                    "/resource/aws_instance/web/dynamic/ingress/for_each",
                    "for_each = var.ports",
                    Some("var.ports")
                ),
                symbol(
                    "/resource/aws_instance/web/dynamic/ingress/content",
                    text_of(source, "content {", "\n    }")?,
                    None
                ),
                symbol(
                    "/resource/aws_instance/web/dynamic/ingress/content/from_port",
                    "from_port = ingress.value",
                    Some("ingress.value")
                ),
                symbol("/resource/aws_instance/web/lifecycle", "lifecycle {}", None),
            ]
        );
        // </block>
        Ok(())
    }

    #[test]
    fn repeated_blocks_parse_symbols_returns_each_copy_at_one_path() -> anyhow::Result<()> {
        let source = r#"locals {
  a = 1
}

locals {
  b = 2
}

resource "x" "y" {
  ingress {
    port = 80
  }
  ingress {
    port = 443
  }
}
"#;

        assert_eq!(
            values(&hcl(source)?, source),
            vec![
                ("/locals".to_string(), None),
                value("/locals/a", "1"),
                ("/locals".to_string(), None),
                value("/locals/b", "2"),
                ("/resource/x/y".to_string(), None),
                ("/resource/x/y/ingress".to_string(), None),
                value("/resource/x/y/ingress/port", "80"),
                ("/resource/x/y/ingress".to_string(), None),
                value("/resource/x/y/ingress/port", "443"),
            ]
        );
        Ok(())
    }

    #[test]
    fn keys_that_are_not_names_parse_symbols_skips_them() -> anyhow::Result<()> {
        let source = r#"m = {
  (var.k) = 1
  "${x}" = 2
  3 = 4
  k.j = 5
  ok = 6
}
"#;

        assert_eq!(
            values(&hcl(source)?, source),
            vec![
                value("/m", source.trim_start_matches("m = ").trim_end()),
                value("/m/ok", "6"),
            ]
        );
        Ok(())
    }

    #[test]
    fn label_or_key_with_an_escape_parse_symbols_skips_it_and_everything_under_it()
    -> anyhow::Result<()> {
        let source = r#"resource "x" "a\"b" {
  c = 1
}

resource "x" "$${y}" {
  c = 2
}

m = {
  "a\nb" = { c = 3 }
  "%%{d}" = 4
  ok = 5
}
"#;

        assert_eq!(
            values(&hcl(source)?, source)
                .into_iter()
                .map(|(path, _)| path)
                .collect::<Vec<_>>(),
            vec!["/m", "/m/ok"]
        );
        Ok(())
    }

    #[test]
    fn quoted_string_parse_symbols_decodes_its_escapes() -> anyhow::Result<()> {
        let source = r#"newline = "a\nb"
cr = "a\rb"
tab = "a\tb"
quote = "a\"b"
backslash = "a\\b"
short = "caf\u00e9F"
long = "\U0001F600"
dollar = "$${x}"
percent = "%%{x}"
doubled = "$$x %%y $$$${z}"
built = "$\u0024{x}"
empty = ""
"#;

        assert_eq!(
            values(&hcl(source)?, source),
            vec![
                value("/newline", "a\nb"),
                value("/cr", "a\rb"),
                value("/tab", "a\tb"),
                value("/quote", "a\"b"),
                value("/backslash", "a\\b"),
                value("/short", "caféF"),
                value("/long", "😀"),
                value("/dollar", "${x}"),
                value("/percent", "%{x}"),
                // HCL pairs the `$` from the left, so only the last pair before `{` is `$${`.
                value("/doubled", "$$x %%y $$${z}"),
                // HCL reads `$${` only as written, not when an escape gives one of its characters.
                value("/built", "$${x}"),
                value("/empty", ""),
            ]
        );
        Ok(())
    }

    #[test]
    fn heredoc_parse_symbols_returns_its_text() -> anyhow::Result<()> {
        // A `·` stands for a space, so that no editor strips the spaces at the end of a line.
        let source = r#"plain = <<EOT
  a
    b
EOT
flush = <<-EOT
    a

      b
···
    $${c} %%{d} \n
  EOT
empty = <<EOT
EOT
flush_empty = <<-EOT
  EOT
"#
        .replace('·', " ");

        assert_eq!(
            values(&hcl(&source)?, &source),
            vec![
                value("/plain", "  a\n    b\n"),
                // HCL leaves a line of only white space out of the count, and as it is.
                value("/flush", "a\n\n  b\n   \n${c} %{d} \\n\n"),
                value("/empty", ""),
                value("/flush_empty", ""),
            ]
        );
        Ok(())
    }

    #[test]
    fn flush_heredoc_with_tabs_parse_symbols_counts_a_tab_as_one_space() -> anyhow::Result<()> {
        let source = "tabs = <<-EOT\n\t\ta\n    b\n  EOT\n";

        assert_eq!(
            values(&hcl(source)?, source),
            vec![value("/tabs", "a\n  b\n")]
        );
        Ok(())
    }

    #[test]
    fn heredoc_with_crlf_line_ends_parse_symbols_keeps_them() -> anyhow::Result<()> {
        let source = "a = <<-EOT\r\n  x\r\n\r\n  y\r\n  EOT\r\n";

        assert_eq!(
            values(&hcl(source)?, source),
            vec![value("/a", "x\r\n\r\ny\r\n")]
        );
        Ok(())
    }

    #[test]
    fn template_parse_symbols_keeps_it_as_written() -> anyhow::Result<()> {
        let source = r#"odd = "$$${x}"
quoted = "a%{if true}b%{endif}"
heredoc = <<-EOT
    ${x}
  EOT
directive = <<-EOT
    %{if true}a%{endif}
  EOT
"#;

        assert_eq!(
            values(&hcl(source)?, source),
            vec![
                // `$$` is a `$`, and the `${` after it starts a template.
                value("/odd", r#""$$${x}""#),
                value("/quoted", r#""a%{if true}b%{endif}""#),
                value("/heredoc", "<<-EOT\n    ${x}\n  EOT"),
                value("/directive", "<<-EOT\n    %{if true}a%{endif}\n  EOT"),
            ]
        );
        Ok(())
    }

    #[test]
    fn escape_of_no_character_parse_symbols_makes_the_path_unreadable() -> anyhow::Result<()> {
        let source = r#"low = "\uD800"
high = "\U00110000"
ok = "fine"
"#;

        assert_eq!(values(&hcl(source)?, source), vec![value("/ok", "fine")]);
        assert_eq!(
            unresolved(&hcl(source)?, source, "/low")?,
            decode_error(1, 7, r"`\uD800` stands for no character")
        );
        assert_eq!(
            unresolved(&hcl(source)?, source, "/high")?,
            decode_error(2, 8, r"`\U00110000` stands for no character")
        );
        Ok(())
    }

    #[test]
    fn parentheses_around_value_parse_symbols_removes_them() -> anyhow::Result<()> {
        let source = r#"a = ((var.x))
b = (1) + (2)
c = ( /* three */ 3 )
"#;

        assert_eq!(
            values(&hcl(source)?, source),
            vec![
                value("/a", "var.x"),
                value("/b", "(1) + (2)"),
                value("/c", "3"),
            ]
        );
        Ok(())
    }

    #[test]
    fn deeply_nested_blocks_parse_symbols_does_not_overflow_stack() -> anyhow::Result<()> {
        // Each block has a path one segment longer than the block around it, so the paths grow
        // with the square of the depth. This depth is enough to overflow a recursive walk.
        let depth = 3_000;
        let source = format!("{}{}", "b {\n".repeat(depth), "}\n".repeat(depth));

        let symbols = hcl(&source)?;

        assert_eq!(symbols.symbols.len(), depth);
        Ok(())
    }

    #[test]
    fn deeply_nested_objects_parse_symbols_does_not_overflow_stack() -> anyhow::Result<()> {
        let depth = 3_000;
        let source = format!(
            "{}a = 1{}",
            "a = { ".repeat(depth - 1),
            " }".repeat(depth - 1)
        );

        let symbols = hcl(&source)?;

        assert_eq!(symbols.symbols.len(), depth);
        Ok(())
    }

    #[test]
    fn syntax_error_parse_symbols_makes_the_definitions_that_hold_it_unreadable()
    -> anyhow::Result<()> {
        // The parser skips the `+`, which has no right side. It puts the skipped text after the
        // attribute, not in it.
        let source = r#"variable "a" {
  default = 1
}

variable "broken" {
  default = 1 +
}

variable "after" {
  default = 2
}
"#;

        assert_eq!(
            values(&hcl(source)?, source),
            vec![
                ("/variable/a".to_string(), None),
                value("/variable/a/default", "1"),
                ("/variable/after".to_string(), None),
                value("/variable/after/default", "2"),
            ]
        );
        assert_eq!(
            unresolved(&hcl(source)?, source, "/variable/broken")?,
            syntax_error(6, 15, "broken")
        );
        assert_eq!(
            unresolved(&hcl(source)?, source, "/variable/broken/default")?,
            syntax_error(6, 15, "default")
        );
        Ok(())
    }

    #[test]
    fn parses_comments_correctly() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser()?;

        let comments: Vec<Comment> = comments_parser
            .parse(
                r#"
# This is a hash comment
variable "region" {
  type = string // inline slash comment
}

/* This is a block comment
spanning multiple lines
*/
/// Triple slash comment.
"#,
            )
            .collect();

        assert_eq!(
            comments,
            vec![
                Comment {
                    position_range: Position::new(2, 1)..Position::new(2, 25),
                    source_range: 1..25,
                    comment_text: "  This is a hash comment".to_string()
                },
                Comment {
                    position_range: Position::new(4, 17)..Position::new(4, 40),
                    source_range: 62..85,
                    comment_text: "   inline slash comment".to_string()
                },
                Comment {
                    position_range: Position::new(7, 1)..Position::new(9, 3),
                    source_range: 89..142,
                    comment_text: "   This is a block comment\nspanning multiple lines\n  "
                        .to_string()
                },
                Comment {
                    position_range: Position::new(10, 1)..Position::new(10, 26),
                    source_range: 143..168,
                    comment_text: "  / Triple slash comment.".to_string()
                },
            ]
        );

        Ok(())
    }
}
