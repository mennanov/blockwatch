use crate::language_parsers::{
    CommentsParser, LanguageParser, LanguageParserImpl, node_text, tree_sitter_parser,
    xml_style_comments_parser,
};
use crate::symbol_path::SymbolPath;
use crate::symbols::code::{child, position, undecodable_reason, with_line_feeds};
use crate::symbols::{ScalarValue, Symbol, Symbols, SymbolsParser, ensure_no_syntax_error};
use anyhow::{Context, bail};
use std::collections::HashMap;
use std::ops::Range;
use tree_sitter::Node;

/// The characters that XML counts as white space.
const WHITE_SPACE: [char; 4] = [' ', '\t', '\r', '\n'];

/// Returns a [`LanguageParser`] for Xml.
pub(super) fn parser() -> anyhow::Result<impl LanguageParser> {
    Ok(LanguageParserImpl::new(comments_parser()?).with_symbols(XmlSymbolsParser::new()?))
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let xml_language = tree_sitter_xml::LANGUAGE_XML.into();
    let parser = xml_style_comments_parser(&xml_language, "Comment")?;
    Ok(parser)
}

/// Finds the symbols of an XML file by walking its syntax tree.
///
/// Every element and every attribute is a symbol, except the `xmlns` attributes that declare
/// namespaces. An element without child elements has its text as its value, decoded and without
/// the white space around it. An attribute has its decoded value. An element with child elements
/// has no value.
struct XmlSymbolsParser {
    parser: tree_sitter::Parser,
}

impl XmlSymbolsParser {
    /// # Errors
    /// Returns an error if the XML grammar cannot be loaded.
    fn new() -> anyhow::Result<Self> {
        let parser = tree_sitter_parser(&tree_sitter_xml::LANGUAGE_XML.into())?;
        Ok(Self { parser })
    }
}

impl SymbolsParser for XmlSymbolsParser {
    /// A value with an entity other than XML's own five, or a reference to a character that XML
    /// doesn't allow, makes its path unreadable.
    ///
    /// # Errors
    /// Returns an error if the file has a syntax error.
    fn parse(&mut self, source: &str) -> anyhow::Result<Symbols> {
        let tree = self
            .parser
            .parse(source, None)
            .context("failed to parse syntax tree")?;
        ensure_no_syntax_error(&tree, source)?;
        let root = tree
            .root_node()
            .child_by_field_name("root")
            .context("unexpected XML document without a root element")?;
        Walk::symbols_in(source, root)
    }
}

/// The state of a walk over the elements of an XML document.
struct Walk<'s, 'tree> {
    source: &'s str,
    /// The path to the last element walked. Before each element, the walk cuts it back to the
    /// path to the element that holds it.
    path: Vec<String>,
    symbols: Vec<Symbol>,
    /// The paths whose value can't be decoded, each with the reason to report.
    unreadable: HashMap<SymbolPath, String>,
    /// The elements that still need symbols, the next one last. Each comes with the length of the
    /// path to the element that holds it.
    pending: Vec<(usize, Node<'tree>)>,
}

impl<'s, 'tree> Walk<'s, 'tree> {
    /// The symbols of `root`, the root element of `source`, and of everything inside it, in
    /// document order.
    ///
    /// # Errors
    /// Returns an error if the syntax tree has a shape the XML grammar doesn't give.
    fn symbols_in(source: &'s str, root: Node<'tree>) -> anyhow::Result<Symbols> {
        let mut walk = Walk {
            source,
            path: Vec::new(),
            symbols: Vec::new(),
            unreadable: HashMap::new(),
            pending: vec![(0, root)],
        };
        // The walk keeps its own stack, as the walks of other languages do. The grammar reads no
        // file deep enough to overflow the call stack today, but a newer one may.
        while let Some((depth, element)) = walk.pending.pop() {
            // Elements come off the stack in document order. So every element walked since the
            // one that holds `element` is inside it, and the path still starts with the path to
            // that element.
            walk.path.truncate(depth);
            walk.add_element_symbols(element)?;
        }
        Ok(Symbols::with_unreadable(walk.symbols, walk.unreadable))
    }

    /// Adds the symbols of `element` and of its attributes. Its child elements go to `pending`.
    /// The current path is the path to the element that holds it.
    ///
    /// # Errors
    /// Returns an error if the element has a shape the XML grammar doesn't give.
    fn add_element_symbols(&mut self, element: Node<'tree>) -> anyhow::Result<()> {
        let (tag, content) = tag_and_content(element)?;
        let name = child(tag, "Name").context("unexpected XML tag without a name")?;
        self.path.push(node_text(&name, self.source).to_string());
        let children = child_elements(content);
        if children.is_empty() {
            self.add_text_symbol(element, tag, content);
        } else {
            self.add_symbol(self.path.clone(), element, None);
        }
        let mut cursor = tag.walk();
        for attribute in tag.named_children(&mut cursor) {
            match attribute.kind() {
                "Attribute" => self.add_attribute_symbol(attribute)?,
                "Name" => {}
                other => bail!("unexpected {other} in an XML tag"),
            }
        }
        // The stack takes the last element first, so the children go on it in reverse.
        let depth = self.path.len();
        self.pending
            .extend(children.into_iter().rev().map(|child| (depth, child)));
        Ok(())
    }

    /// Adds the symbol of `element`, an element without child elements, at the current path. Its
    /// value is the text of `content`, or empty when it has no content. `tag` is its start tag.
    fn add_text_symbol(&mut self, element: Node, tag: Node, content: Option<Node>) {
        match content {
            Some(content) => {
                let text = element_text(content, self.source);
                // The range leaves out the white space too, so text that needs no decoding is the
                // source itself, and each of its lines keeps its position.
                let written = node_text(&content, self.source);
                let start = content.start_byte() + written.len()
                    - written.trim_start_matches(WHITE_SPACE).len();
                let length = written.trim_matches(WHITE_SPACE).len();
                self.add_value_symbol(
                    self.path.clone(),
                    element,
                    content,
                    text,
                    start..start + length,
                );
            }
            // Nothing is written between the tags, so the value is the point where the start tag
            // ends: right after `<a/>`, or between the tags of `<a></a>`.
            None => {
                let value = ScalarValue {
                    text: String::new(),
                    byte_range: tag.end_byte()..tag.end_byte(),
                };
                self.add_symbol(self.path.clone(), element, Some(value));
            }
        }
    }

    /// Adds the symbol of `attribute`, an attribute of the element at the current path.
    ///
    /// # Errors
    /// Returns an error if the attribute has a shape the XML grammar doesn't give.
    fn add_attribute_symbol(&mut self, attribute: Node<'tree>) -> anyhow::Result<()> {
        let name = child(attribute, "Name").context("unexpected XML attribute without a name")?;
        let name = node_text(&name, self.source);
        // A name keeps its prefix as written, and the namespace a prefix stands for plays no part
        // in a path. So the attributes that declare namespaces hold nothing a path needs.
        if name == "xmlns" || name.starts_with("xmlns:") {
            return Ok(());
        }
        let value =
            child(attribute, "AttValue").context("unexpected XML attribute without a value")?;
        let mut path = self.path.clone();
        path.push(format!("@{name}"));
        let text = attribute_text(node_text(&value, self.source));
        self.add_value_symbol(path, attribute, value, text, value.byte_range());
        Ok(())
    }

    /// Adds the symbol of `written`, at `path`, with `text` as its value, written at `range`
    /// inside `value`. When `text` is an error, the path is unreadable instead.
    fn add_value_symbol(
        &mut self,
        path: Vec<String>,
        written: Node,
        value: Node,
        text: anyhow::Result<String>,
        range: Range<usize>,
    ) {
        match text {
            Ok(text) => {
                let value = ScalarValue {
                    text,
                    byte_range: range,
                };
                self.add_symbol(path, written, Some(value));
            }
            Err(error) => {
                let reason =
                    undecodable_reason(position(&value, self.source), &format!("{error:#}"));
                self.unreadable
                    .entry(SymbolPath::from_segments(path))
                    .or_insert(reason);
            }
        }
    }

    /// Adds the symbol at `path`, defined where `written` is, with `value`.
    fn add_symbol(&mut self, path: Vec<String>, written: Node, value: Option<ScalarValue>) {
        self.symbols.push(Symbol {
            path: SymbolPath::from_segments(path),
            def_byte_ranges: vec![written.byte_range()],
            value,
        });
    }
}

/// The child elements in `content`, the content of an element, in document order. Empty when the
/// element has no content.
fn child_elements(content: Option<Node>) -> Vec<Node> {
    content.map_or_else(Vec::new, |content| {
        let mut cursor = content.walk();
        content
            .named_children(&mut cursor)
            .filter(|child| child.kind() == "element")
            .collect()
    })
}

/// The tag of `element`, its start tag or the whole of `<a/>`, and its content: what is written
/// between its start tag and its end tag. The content is `None` when nothing is, as in `<a></a>`.
///
/// # Errors
/// Returns an error if the element has a shape the XML grammar doesn't give.
fn tag_and_content(element: Node) -> anyhow::Result<(Node, Option<Node>)> {
    let mut tag = None;
    let mut content = None;
    let mut cursor = element.walk();
    for child in element.named_children(&mut cursor) {
        match child.kind() {
            "STag" | "EmptyElemTag" => tag = Some(child),
            "content" => content = Some(child),
            "ETag" => {}
            other => bail!("unexpected {other} in an XML element"),
        }
    }
    Ok((
        tag.context("unexpected XML element without a tag")?,
        content,
    ))
}

/// The text of `content`, the content of an element without child elements, decoded and without
/// XML's white space around it. CDATA is kept as written, and comments and processing
/// instructions are left out.
///
/// # Errors
/// Returns an error if the text holds an entity other than XML's own five, or a reference to a
/// character that XML doesn't allow.
fn element_text(content: Node, source: &str) -> anyhow::Result<String> {
    let mut text = String::new();
    let mut cursor = content.walk();
    for part in content.named_children(&mut cursor) {
        match part.kind() {
            // XML reads every line end as a line feed.
            "CharData" => text.push_str(&with_line_feeds(node_text(&part, source))),
            // An empty CDATA section has no text node.
            "CDSect" => text.push_str(&child(part, "CData").map_or_else(String::new, |cdata| {
                with_line_feeds(node_text(&cdata, source))
            })),
            "EntityRef" | "CharRef" => text.push(decoded_reference(node_text(&part, source))?),
            "Comment" | "PI" => {}
            other => bail!("unexpected {other} in XML text"),
        }
    }
    Ok(text.trim_matches(WHITE_SPACE).to_string())
}

/// The value of an attribute that is written `quoted`, quotes included, decoded. Each tab and
/// line break becomes a space, as XML requires, but a character reference such as `&#10;` stays
/// what it stands for.
///
/// # Errors
/// Returns an error if the value holds an entity other than XML's own five, or a reference to a
/// character that XML doesn't allow.
fn attribute_text(quoted: &str) -> anyhow::Result<String> {
    let written = quoted
        .get(1..quoted.len() - 1)
        .context("unexpected XML attribute value without quotes")?;
    let written = with_line_feeds(written).replace(['\t', '\n'], " ");
    // The grammar takes each `&` for the start of a reference that a `;` ends.
    let mut pieces = written.split('&');
    let mut text = pieces.next().unwrap_or_default().to_string();
    for piece in pieces {
        let (name, rest) = piece
            .split_once(';')
            .context("unexpected XML reference without a `;`")?;
        text.push(decoded_reference(&format!("&{name};"))?);
        text.push_str(rest);
    }
    Ok(text)
}

/// The character that `reference`, such as `&amp;`, `&#65;` or `&#x41;`, stands for.
///
/// # Errors
/// Returns an error if `reference` is an entity other than XML's own five, or a reference to a
/// character that XML doesn't allow.
fn decoded_reference(reference: &str) -> anyhow::Result<char> {
    match reference {
        "&lt;" => Ok('<'),
        "&gt;" => Ok('>'),
        "&amp;" => Ok('&'),
        "&apos;" => Ok('\''),
        "&quot;" => Ok('"'),
        _ if reference.starts_with("&#") => {
            let digits = reference.trim_start_matches("&#").trim_end_matches(';');
            let code = match digits.strip_prefix('x') {
                Some(hex) => u32::from_str_radix(hex, 16),
                None => digits.parse(),
            };
            code.ok()
                .and_then(char::from_u32)
                .filter(|character| {
                    matches!(character, '\t' | '\n' | '\r' | ' '..='\u{d7ff}' | '\u{e000}'..='\u{fffd}' | '\u{10000}'..)
                })
                .with_context(|| format!("`{reference}` is not a character that XML allows"))
        }
        // An entity that the DOCTYPE declares can stand for any text, even child elements.
        _ => bail!("blockwatch reads only the 5 entities that XML defines, not `{reference}`"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Position, language_parsers::Comment};

    #[test]
    fn parses_xml_comments_correctly() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser()?;

        let blocks: Vec<Comment> = comments_parser
            .parse(
                r#"
            <!-- This is a comment -->
            <root>
                <!-- Another comment -->
                <child>Value</child>
                <!-- 
                Multiline comment 
                <foo>bar</foo>
                -->
            </root>
            <!-- Final comment -->
            "#,
            )
            .collect();

        assert_eq!(
            blocks,
            vec![
                Comment {
                    position_range: Position::new(2, 13)..Position::new(2, 39),
                    source_range: 13..39,
                    comment_text: "     This is a comment    ".to_string()
                },
                Comment {
                    position_range: Position::new(4, 17)..Position::new(4, 41),
                    source_range: 75..99,
                    comment_text: "     Another comment    ".to_string()
                },
                Comment {
                    position_range: Position::new(6, 17)..Position::new(9, 20),
                    source_range: 153..244,
                    comment_text: "     \n                Multiline comment \n                <foo>bar</foo>\n                   ".to_string()
                },
                Comment {
                    position_range: Position::new(11, 13)..Position::new(11, 35),
                    source_range: 277..299,
                    comment_text: "     Final comment    ".to_string()
                }
            ]
        );

        Ok(())
    }
}

#[cfg(test)]
mod symbols_tests {
    use super::*;
    use crate::symbols::code::test_utils::{
        decode_error, symbol, symbols, text_of, unresolved, value, values,
    };

    /// The symbols of `source`, an XML file.
    fn xml(source: &str) -> anyhow::Result<Symbols> {
        XmlSymbolsParser::new()?.parse(source)
    }

    /// The definition of each of `symbols`, the symbols of `source`, that has a value, with `[`
    /// before the value's range and `]` after it.
    fn marked_values(symbols: &Symbols, source: &str) -> Vec<String> {
        symbols
            .symbols
            .iter()
            .filter_map(|symbol| {
                let definition = symbol.def_byte_ranges[0].clone();
                let value = symbol.value.as_ref()?.byte_range.clone();
                Some(format!(
                    "{}[{}]{}",
                    &source[definition.start..value.start],
                    &source[value.clone()],
                    &source[value.end..definition.end]
                ))
            })
            .collect()
    }

    #[test]
    fn document_with_every_xml_shape_parse_symbols_returns_every_symbol_in_order()
    -> anyhow::Result<()> {
        let source = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE project>
<!-- The build of the app. -->
<project xmlns="http://maven.apache.org/POM/4.0.0" xmlns:a="urn:a" a:mode="strict" kind='app'>
  <version>1.4.2</version>
  <java.version>17</java.version>
  <a:name>app</a:name>
  <empty flag="on"/>
  <closed></closed>
  <?build skip?>
  <dependencies>
    <dependency>
      <artifactId>junit</artifactId>
    </dependency>
    <!-- Only for tests. -->
    <dependency scope="test">
      <artifactId>guava</artifactId>
    </dependency>
  </dependencies>
  <mixed>text<b>bold</b></mixed>
</project>
"#;

        // <block affects="docs/symbols.md:xml-paths, src/skill.md#/blockwatch/Symbols/XML">
        assert_eq!(
            symbols(&xml(source)?, source),
            vec![
                symbol("/project", text_of(source, "<project", "</project>")?, None),
                symbol("/project/@a%3Amode", r#"a:mode="strict""#, Some("strict")),
                symbol("/project/@kind", "kind='app'", Some("app")),
                symbol(
                    "/project/version",
                    "<version>1.4.2</version>",
                    Some("1.4.2")
                ),
                symbol(
                    "/project/java.version",
                    "<java.version>17</java.version>",
                    Some("17")
                ),
                symbol("/project/a%3Aname", "<a:name>app</a:name>", Some("app")),
                symbol("/project/empty", r#"<empty flag="on"/>"#, Some("")),
                symbol("/project/empty/@flag", r#"flag="on""#, Some("on")),
                symbol("/project/closed", "<closed></closed>", Some("")),
                symbol(
                    "/project/dependencies",
                    text_of(source, "<dependencies>", "</dependencies>")?,
                    None
                ),
                symbol(
                    "/project/dependencies/dependency",
                    text_of(source, "<dependency>", "</dependency>")?,
                    None
                ),
                symbol(
                    "/project/dependencies/dependency/artifactId",
                    "<artifactId>junit</artifactId>",
                    Some("junit")
                ),
                symbol(
                    "/project/dependencies/dependency",
                    text_of(source, "<dependency scope", "</dependency>")?,
                    None
                ),
                symbol(
                    "/project/dependencies/dependency/@scope",
                    r#"scope="test""#,
                    Some("test")
                ),
                symbol(
                    "/project/dependencies/dependency/artifactId",
                    "<artifactId>guava</artifactId>",
                    Some("guava")
                ),
                symbol("/project/mixed", "<mixed>text<b>bold</b></mixed>", None),
                symbol("/project/mixed/b", "<b>bold</b>", Some("bold")),
            ]
        );
        // </block>
        Ok(())
    }

    #[test]
    fn values_parse_symbols_returns_the_range_where_each_is_written() -> anyhow::Result<()> {
        // White space around text is left out of the range too, so text without entities keeps
        // the position of each of its lines.
        let source = r#"<r a=" 1 ">
  <text>x</text>
  <padded> x </padded>
  <entity> &amp; </entity>
  <blank>  </blank>
  <empty/>
  <closed></closed>
</r>"#;

        assert_eq!(
            marked_values(&xml(source)?, source),
            vec![
                r#"a=[" 1 "]"#,
                "<text>[x]</text>",
                "<padded> [x] </padded>",
                "<entity> [&amp;] </entity>",
                "<blank>  []</blank>",
                "<empty/>[]",
                "<closed>[]</closed>",
            ]
        );
        Ok(())
    }

    #[test]
    fn attributes_that_start_with_xmlns_parse_symbols_leaves_out_only_namespace_declarations()
    -> anyhow::Result<()> {
        let source = r#"<r xmlns="urn:r" xmlns:a="urn:a" xmlnsx="1"/>"#;

        assert_eq!(
            symbols(&xml(source)?, source),
            vec![
                symbol("/r", source, Some("")),
                symbol("/r/@xmlnsx", r#"xmlnsx="1""#, Some("1")),
            ]
        );
        Ok(())
    }

    #[test]
    fn elements_nested_past_the_grammar_limit_parse_symbols_returns_error() -> anyhow::Result<()> {
        // The grammar keeps the names of the open elements in a buffer of 1,024 bytes, at one
        // byte more than each name. A file that needs more is a syntax error. That depth is too
        // low to overflow a recursive walk. If a new grammar reads deeper files, this test fails,
        // and a test that the walk doesn't overflow the call stack should replace it.
        let nested = |depth| format!("{}{}", "<a>".repeat(depth), "</a>".repeat(depth));

        assert_eq!(xml(&nested(507))?.symbols.len(), 507);
        assert!(xml(&nested(508)).is_err());
        Ok(())
    }

    #[test]
    fn end_tag_that_does_not_match_its_start_tag_parse_symbols_returns_error() {
        let error = xml("<a>\n  <b>1</c>\n</a>\n").unwrap_err();

        assert!(
            error
                .to_string()
                .starts_with("file has a syntax error at line "),
            "unexpected error message: {error}"
        );
    }

    #[test]
    fn entities_and_character_references_parse_symbols_decodes_them() -> anyhow::Result<()> {
        let source = r#"<r a="&lt;&amp;&gt;&quot;&apos;&#65;&#x42;">
  <t>&lt;&amp;&gt;&quot;&apos;&#65;&#x42;</t>
</r>"#;

        assert_eq!(
            values(&xml(source)?, source),
            vec![
                ("/r".to_string(), None),
                value("/r/@a", r#"<&>"'AB"#),
                value("/r/t", r#"<&>"'AB"#),
            ]
        );
        Ok(())
    }

    #[test]
    fn cdata_parse_symbols_keeps_its_text_as_written() -> anyhow::Result<()> {
        let source = r#"<r>
  <t>a<![CDATA[<b>&amp;</b>]]>c</t>
  <empty><![CDATA[]]></empty>
</r>"#;

        assert_eq!(
            values(&xml(source)?, source),
            vec![
                ("/r".to_string(), None),
                value("/r/t", "a<b>&amp;</b>c"),
                value("/r/empty", ""),
            ]
        );
        Ok(())
    }

    #[test]
    fn comments_and_processing_instructions_in_text_parse_symbols_leaves_them_out()
    -> anyhow::Result<()> {
        let source = r#"<r>
  <comment>1<!-- x -->.2</comment>
  <instruction>3<?pi?>.4</instruction>
  <only><!-- x --></only>
</r>"#;

        assert_eq!(
            values(&xml(source)?, source),
            vec![
                ("/r".to_string(), None),
                value("/r/comment", "1.2"),
                value("/r/instruction", "3.4"),
                value("/r/only", ""),
            ]
        );
        Ok(())
    }

    #[test]
    fn crlf_and_cr_in_text_parse_symbols_turns_them_into_line_feeds() -> anyhow::Result<()> {
        let source = "<r>\r\n  <t>a\r\nb\rc</t>\r\n  <c><![CDATA[a\r\nb\rc]]></c>\r\n</r>";

        assert_eq!(
            values(&xml(source)?, source),
            vec![
                ("/r".to_string(), None),
                value("/r/t", "a\nb\nc"),
                value("/r/c", "a\nb\nc"),
            ]
        );
        Ok(())
    }

    #[test]
    fn white_space_around_text_parse_symbols_removes_it() -> anyhow::Result<()> {
        // Only XML's white space is removed: space, tab, CR and line feed. It goes from both ends
        // of the decoded text, whatever wrote it.
        let source = "<r>
  <lines>
    a
      b
  </lines>
  <cdata> <![CDATA[ x ]]> </cdata>
  <references>&#32;y&#9;&#10;&#13;</references>
  <no-break>\u{a0}z\u{a0}</no-break>
</r>";

        assert_eq!(
            values(&xml(source)?, source),
            vec![
                ("/r".to_string(), None),
                value("/r/lines", "a\n      b"),
                value("/r/cdata", "x"),
                value("/r/references", "y"),
                value("/r/no-break", "\u{a0}z\u{a0}"),
            ]
        );
        Ok(())
    }

    #[test]
    fn white_space_in_attribute_parse_symbols_turns_it_into_spaces_and_keeps_it()
    -> anyhow::Result<()> {
        // A character reference is not white space as written, so it stays as it is.
        let source = "<r a=\" x\ty\r\nz\rw\n \" b=\"1&#10;&#9;&#13;2\"/>";

        assert_eq!(
            values(&xml(source)?, source),
            vec![
                value("/r", ""),
                value("/r/@a", " x y z w  "),
                value("/r/@b", "1\n\t\r2"),
            ]
        );
        Ok(())
    }

    #[test]
    fn entity_that_xml_does_not_define_parse_symbols_makes_the_path_unreadable()
    -> anyhow::Result<()> {
        // The DOCTYPE could make the entity any text, even child elements, so blockwatch can't
        // tell the value.
        let source = r#"<!DOCTYPE r [
  <!ENTITY e "x">
]>
<r a="&e;"><t>&e;</t><u>1</u></r>"#;
        let problem = "blockwatch reads only the 5 entities that XML defines, not `&e;`";

        assert_eq!(
            values(&xml(source)?, source),
            vec![("/r".to_string(), None), value("/r/u", "1")]
        );
        assert_eq!(
            unresolved(&xml(source)?, source, "/r/@a")?,
            decode_error(4, 6, problem)
        );
        assert_eq!(
            unresolved(&xml(source)?, source, "/r/t")?,
            decode_error(4, 15, problem)
        );
        Ok(())
    }

    #[test]
    fn character_reference_to_a_character_xml_forbids_parse_symbols_makes_the_path_unreadable()
    -> anyhow::Result<()> {
        let forbidden = [
            "&#0;",
            "&#x8;",
            "&#x1F;",
            "&#xD800;",
            "&#xDFFF;",
            "&#xFFFE;",
            "&#xFFFF;",
            "&#x110000;",
            "&#99999999999;",
        ];
        for reference in forbidden {
            let source = format!("<t>{reference}</t>");
            let problem = format!("`{reference}` is not a character that XML allows");

            assert_eq!(
                unresolved(&xml(&source)?, &source, "/t")?,
                decode_error(1, 4, &problem)
            );
        }
        Ok(())
    }

    #[test]
    fn character_references_at_the_edges_of_what_xml_allows_parse_symbols_decodes_them()
    -> anyhow::Result<()> {
        let source = "<t>a&#x9;&#xA;&#xD;&#x20;&#xD7FF;&#xE000;&#xFFFD;&#x10000;&#x10FFFF;b</t>";

        assert_eq!(
            values(&xml(source)?, source),
            vec![value(
                "/t",
                "a\t\n\r \u{d7ff}\u{e000}\u{fffd}\u{10000}\u{10ffff}b"
            )]
        );
        Ok(())
    }
}
