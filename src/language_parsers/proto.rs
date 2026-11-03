use crate::language_parsers;
use crate::language_parsers::{CommentsParser, LanguageParser, LanguageParserImpl};
use crate::symbols::ScalarValue;
use crate::symbols::code::{self, CodeLanguage, CodeSymbolsParser, Definition, Value, child};
use tree_sitter::Node;

/// Returns a [`LanguageParser`] for Protocol Buffers, with its symbols.
pub(super) fn parser() -> anyhow::Result<impl LanguageParser> {
    Ok(
        LanguageParserImpl::new(comments_parser()?).with_symbols(CodeSymbolsParser::new(
            &tree_sitter_proto::LANGUAGE.into(),
            Proto,
        )?),
    )
}

/// Protocol Buffers, whose files hold messages, enums and services, and whose messages hold their
/// fields and nested types.
struct Proto;

impl CodeLanguage for Proto {
    /// The definitions under `root`, the root of a Protocol Buffers file's syntax tree, and the
    /// members of its messages, enums and services, in document order. A definition comes before
    /// its members. The package is not part of any path.
    fn definitions<'tree>(&self, root: Node<'tree>, source: &str) -> Vec<Definition<'tree>> {
        code::nested_definitions(root, |body, parents, definitions, bodies| {
            push_members(body, parents, source, definitions, bodies);
        })
    }

    /// Returns `None`, as no symbol has a string value: a value is a field or enum number, and
    /// options are not symbols.
    fn decode_string(&self, _node: &Node, _source: &str) -> anyhow::Result<Option<String>> {
        Ok(None)
    }
}

/// Adds the definitions of the members of `body` to `definitions`. `body` is the root of a file,
/// the body of a message, enum or `extend`, a oneof, or a service. `parents` are the names of the
/// definitions it belongs to, and `source` is the text of the file. Adds each body in `body` to
/// `bodies`, with its own parents.
fn push_members<'tree>(
    body: Node<'tree>,
    parents: Vec<Node<'tree>>,
    source: &str,
    definitions: &mut Vec<Definition<'tree>>,
    bodies: &mut Vec<(Node<'tree>, Vec<Node<'tree>>)>,
) {
    let mut cursor = body.walk();
    for member in body.named_children(&mut cursor) {
        let (name, value, member_body) = match member.kind() {
            "message" | "group" => (
                child(member, "message_name"),
                None,
                child(member, "message_body"),
            ),
            "enum" => (child(member, "enum_name"), None, child(member, "enum_body")),
            // A service holds its rpcs without a body node around them.
            "service" => (child(member, "service_name"), None, Some(member)),
            "rpc" => (child(member, "rpc_name"), None, None),
            "field" | "map_field" | "oneof_field" => (
                child(member, "identifier"),
                child(member, "field_number"),
                None,
            ),
            "enum_field" => {
                definitions.extend(enum_value(member, &parents, source));
                continue;
            }
            // protoc puts the fields of a oneof in its message, so they share the oneof's
            // parents.
            "oneof" => {
                definitions.extend(
                    child(member, "identifier").map(|name| {
                        Definition::new(parents.clone(), name, member.byte_range(), None)
                    }),
                );
                bodies.push((member, parents.clone()));
                continue;
            }
            // protoc puts an extension field where its `extend` is written, not in the message
            // it extends.
            "extend" => {
                bodies.extend(child(member, "message_body").map(|body| (body, parents.clone())));
                continue;
            }
            // Options, `reserved` and `extensions` ranges, and the lines at the top of a file,
            // such as `package`, define no name that code refers to.
            _ => continue,
        };
        let Some(name) = name else {
            continue;
        };
        definitions.push(Definition::new(
            parents.clone(),
            name,
            member.byte_range(),
            value,
        ));
        if let Some(member_body) = member_body {
            bodies.push((member_body, [parents.as_slice(), &[name]].concat()));
        }
    }
}

/// The definition of `field`, a value of the enum at `parents`, in `source`. Its value is its
/// number, with the sign. Returns `None` when the grammar found no name or number.
fn enum_value<'tree>(
    field: Node<'tree>,
    parents: &[Node<'tree>],
    source: &str,
) -> Option<Definition<'tree>> {
    let name = child(field, "identifier")?;
    let number = child(field, "int_lit")?;
    // The number's node doesn't hold its sign, which is a token before it. protoc allows a space
    // between them.
    let mut cursor = field.walk();
    let start = field
        .children(&mut cursor)
        .find(|token| token.kind() == "-")
        .map_or(number.start_byte(), |sign| sign.start_byte());
    let byte_range = start..number.end_byte();
    #[expect(
        clippy::string_slice,
        reason = "the range starts and ends where tree-sitter nodes do, on whole characters"
    )]
    let text = source[byte_range.clone()].to_string();
    Some(Definition {
        parents: parents.to_vec(),
        names: vec![name],
        range: field.byte_range(),
        value: Some(Value::Read(ScalarValue { text, byte_range })),
    })
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let proto_language = tree_sitter_proto::LANGUAGE.into();
    let parser = language_parsers::c_style_comments_parser(&proto_language, "comment")?;
    Ok(parser)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::symbols::code::CodeSymbolsParser;
    use crate::symbols::code::test_utils::{symbol, symbols, syntax_error, unresolved, values};
    use crate::symbols::{Symbols, SymbolsParser};
    use crate::{Position, language_parsers::Comment};
    use anyhow::Context;

    /// The symbols of `source`, a Protocol Buffers file.
    fn proto(source: &str) -> anyhow::Result<Symbols> {
        CodeSymbolsParser::new(&tree_sitter_proto::LANGUAGE.into(), Proto)?.parse(source)
    }

    #[test]
    fn file_with_every_shape_parse_symbols_returns_every_symbol_in_order() -> anyhow::Result<()> {
        let source = r#"syntax = "proto2";

package acme.v1;

option go_package = "acme/v1";

// The order.
message Order {
  option deprecated = true;
  optional int32 id = 0x3 [default = 5];
  map<string, int32> counts = 5;
  enum State {
    option deprecated = true;
    OPEN = 1;
    CLOSED = -2 [deprecated = true];
  }
  oneof kind {
    string sku = 4;
    group Pick = 9 {
      optional int32 size = 10;
    }
  }
  optional group Result = 6 {
    optional int32 code = 7;
  }
  extend Order {
    optional int32 ext = 100;
  }
  extensions 100 to max;
  reserved 8, 11;
  message Inner {
    repeated string tags = 1;
  }
}

enum Top {
  TOP_UNKNOWN = 0;
}

service Orders {
  rpc Get(Order) returns (Order);
  rpc List(Order) returns (stream Order) {
    option deprecated = true;
  }
}
"#;
        // The text from the first `start` up to the first `end` after it, `end` included.
        let text_of = |start: &str, end: &str| -> anyhow::Result<&str> {
            let start = source.find(start).context("no start")?;
            let end = start + source[start..].find(end).context("no end")? + end.len();
            Ok(&source[start..end])
        };

        // <block affects="docs/symbols.md:proto-paths, src/skill.md#/blockwatch/Symbols/Protocol Buffers">
        assert_eq!(
            symbols(&proto(source)?, source),
            vec![
                symbol("/Order", text_of("message Order", "\n}")?, None),
                symbol(
                    "/Order/id",
                    "optional int32 id = 0x3 [default = 5];",
                    Some("0x3")
                ),
                symbol("/Order/counts", "map<string, int32> counts = 5;", Some("5")),
                symbol("/Order/State", text_of("enum State", "\n  }")?, None),
                symbol("/Order/State/OPEN", "OPEN = 1;", Some("1")),
                symbol(
                    "/Order/State/CLOSED",
                    "CLOSED = -2 [deprecated = true];",
                    Some("-2")
                ),
                symbol("/Order/kind", text_of("oneof kind", "\n  }")?, None),
                symbol("/Order/sku", "string sku = 4;", Some("4")),
                symbol("/Order/Pick", text_of("group Pick", "\n    }")?, None),
                symbol("/Order/Pick/size", "optional int32 size = 10;", Some("10")),
                symbol(
                    "/Order/Result",
                    text_of("optional group Result", "\n  }")?,
                    None
                ),
                symbol("/Order/Result/code", "optional int32 code = 7;", Some("7")),
                symbol("/Order/ext", "optional int32 ext = 100;", Some("100")),
                symbol("/Order/Inner", text_of("message Inner", "\n  }")?, None),
                symbol("/Order/Inner/tags", "repeated string tags = 1;", Some("1")),
                symbol("/Top", text_of("enum Top", "\n}")?, None),
                symbol("/Top/TOP_UNKNOWN", "TOP_UNKNOWN = 0;", Some("0")),
                symbol("/Orders", text_of("service Orders", "\n}")?, None),
                symbol("/Orders/Get", "rpc Get(Order) returns (Order);", None),
                symbol("/Orders/List", text_of("rpc List", "\n  }")?, None),
            ]
        );
        // </block>
        Ok(())
    }

    #[test]
    fn extend_at_the_top_level_parse_symbols_returns_its_fields_at_the_top_level()
    -> anyhow::Result<()> {
        let source = r#"syntax = "proto2";

message Other {
  extensions 100 to max;
}

extend Other {
  optional int32 ext = 100;
}
"#;

        assert_eq!(
            values(&proto(source)?, source),
            vec![
                ("/Other".to_string(), None),
                ("/ext".to_string(), Some("100".to_string())),
            ]
        );
        Ok(())
    }

    #[test]
    fn negative_enum_value_with_a_space_parse_symbols_returns_it_as_written() -> anyhow::Result<()>
    {
        let source = r#"enum Sign {
  ZERO = 0;
  NEGATIVE = - 2;
}
"#;

        assert_eq!(
            symbols(&proto(source)?, source),
            vec![
                symbol("/Sign", source.trim_end(), None),
                symbol("/Sign/ZERO", "ZERO = 0;", Some("0")),
                symbol("/Sign/NEGATIVE", "NEGATIVE = - 2;", Some("- 2")),
            ]
        );
        Ok(())
    }

    #[test]
    fn deeply_nested_messages_parse_symbols_does_not_overflow_stack() -> anyhow::Result<()> {
        // Each message has a path one segment longer than the message around it, so the paths
        // grow with the square of the depth. This depth is enough to overflow a recursive walk.
        let depth = 3_000;
        let source = format!("{}{}", "message M {\n".repeat(depth), "}\n".repeat(depth));

        let symbols = proto(&source)?;

        assert_eq!(symbols.symbols.len(), depth);
        Ok(())
    }

    #[test]
    fn syntax_error_parse_symbols_makes_the_definitions_that_hold_it_unreadable()
    -> anyhow::Result<()> {
        // The field has no number, so the parser assumes one is missing before the `;`.
        let source = r#"message A {
  int32 x = 1;
  int32 broken = ;
  int32 y = 2;
}

message After {}
"#;

        assert_eq!(
            symbols(&proto(source)?, source),
            vec![
                symbol("/A/x", "int32 x = 1;", Some("1")),
                symbol("/A/y", "int32 y = 2;", Some("2")),
                symbol("/After", "message After {}", None),
            ]
        );
        assert_eq!(
            unresolved(&proto(source)?, source, "/A")?,
            syntax_error(3, 17, "A")
        );
        assert_eq!(
            unresolved(&proto(source)?, source, "/A/broken")?,
            syntax_error(3, 17, "broken")
        );
        Ok(())
    }

    #[test]
    fn parses_comments_correctly() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser()?;

        let comments: Vec<Comment> = comments_parser
            .parse(
                r#"
// This is a line comment
syntax = "proto3";

message User {
  int32 id = 1; // inline comment
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
                    position_range: Position::new(2, 1)..Position::new(2, 26),
                    source_range: 1..26,
                    comment_text: "   This is a line comment".to_string()
                },
                Comment {
                    position_range: Position::new(6, 17)..Position::new(6, 34),
                    source_range: 78..95,
                    comment_text: "   inline comment".to_string()
                },
                Comment {
                    position_range: Position::new(9, 1)..Position::new(11, 3),
                    source_range: 99..152,
                    comment_text: "   This is a block comment\nspanning multiple lines\n  "
                        .to_string()
                },
                Comment {
                    position_range: Position::new(12, 1)..Position::new(12, 26),
                    source_range: 153..178,
                    comment_text: "  / Triple slash comment.".to_string()
                },
            ]
        );

        Ok(())
    }
}
