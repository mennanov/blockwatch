use crate::languages;
use crate::languages::code_symbols::{
    self, CodeLanguage, CodeSymbolsParser, Definition, child, value_after,
};
use crate::languages::{CommentsParser, LanguageParser, LanguageParserImpl, node_text};
use anyhow::{anyhow, bail};
use std::iter::Peekable;
use std::str::Chars;
use tree_sitter::Node;

/// Returns a [`LanguageParser`] for Prisma schema files, with their symbols.
pub(super) fn parser() -> impl LanguageParser {
    LanguageParserImpl::new(comments_parser()).with_symbols(CodeSymbolsParser::new(
        &tree_sitter_prisma::LANGUAGE.into(),
        Prisma,
    ))
}

/// Prisma schemas, whose files hold `datasource`, `generator`, `model`, `view`, `type` and `enum`
/// blocks, with their properties, fields and enum values under them.
struct Prisma;

impl CodeLanguage for Prisma {
    /// The top-level blocks under `root` and their members, in document order.
    fn definitions<'tree>(&self, root: Node<'tree>, _source: &str) -> Vec<Definition<'tree>> {
        let mut definitions = Vec::new();
        let mut cursor = root.walk();
        for declaration in root.named_children(&mut cursor) {
            push_declaration(declaration, &mut definitions);
        }
        definitions
    }

    /// Decodes a double-quoted Prisma string literal following JSON string escape rules. Returns
    /// `None` for any other node, such as a call or an array.
    ///
    /// # Errors
    /// Returns an error if the string uses single quotes, holds a control character such as a tab
    /// or a line break, uses an escape that Prisma rejects, or holds half of a character.
    fn decode_string(&self, node: &Node, source: &str) -> anyhow::Result<Option<String>> {
        if node.kind() != "string" {
            return Ok(None);
        }
        let raw = node_text(node, source);
        if !raw.starts_with('"') {
            bail!("Prisma strings use double quotes");
        }
        let content = code_symbols::between_quotes(raw, "\"")?;
        // The grammar allows these, even after a backslash. Prisma rejects them, but allows U+007F.
        if content.contains(|character| character < ' ') {
            bail!(
                "Prisma does not allow a control character, such as a tab or a line break, in a string"
            );
        }
        code_symbols::decode_utf16(content, push_escape).map(Some)
    }
}

/// Reads one escape from `characters`, which start right after its backslash, and adds the UTF-16
/// code units it stands for to `units`.
///
/// # Errors
/// Returns an error if the escape is not one of Prisma's, or if no character follows the backslash.
fn push_escape(characters: &mut Peekable<Chars>, units: &mut Vec<u16>) -> anyhow::Result<()> {
    let escaped = characters
        .next()
        .ok_or_else(|| anyhow!("the string ends with a `\\` that escapes nothing"))?;
    let character = match escaped {
        '"' | '\\' | '/' => escaped,
        'b' => '\u{8}',
        'f' => '\u{c}',
        'n' => '\n',
        'r' => '\r',
        't' => '\t',
        'u' => {
            let digits: String = characters.by_ref().take(4).collect();
            let unit = code_symbols::utf16_unit(&digits)
                .ok_or_else(|| anyhow!("`\\u{digits}` is not an escape in a Prisma string"))?;
            units.push(unit);
            return Ok(());
        }
        _ => bail!("`\\{escaped}` is not an escape in a Prisma string"),
    };
    code_symbols::push_utf16(character, units);
    Ok(())
}

/// Adds `declaration`, a top-level Prisma block, and its members to `definitions`, in document
/// order. Adds nothing for comments, non-block `type` aliases, or other nodes.
fn push_declaration<'tree>(declaration: Node<'tree>, definitions: &mut Vec<Definition<'tree>>) {
    let (body_kind, member_kind, with_value) = match declaration.kind() {
        "datasource_declaration" | "generator_declaration" => {
            ("statement_block", "assignment_expression", true)
        }
        "model_declaration" | "view_declaration" | "type_declaration" => {
            ("statement_block", "column_declaration", false)
        }
        "enum_declaration" => ("enum_block", "enumeral", false),
        _ => return,
    };
    let Some(name) = child(declaration, "identifier") else {
        return;
    };
    // Require the body block so legacy non-block `type` aliases in the grammar are skipped.
    let Some(body) = child(declaration, body_kind) else {
        return;
    };
    definitions.push(Definition::new(
        Vec::new(),
        name,
        declaration.byte_range(),
        None,
    ));
    let mut cursor = body.walk();
    for member in body.named_children(&mut cursor) {
        if member.kind() != member_kind {
            continue;
        }
        let (member_name, value) = if with_value {
            (child(member, "variable"), value_after(member, "="))
        } else {
            (child(member, "identifier"), None)
        };
        if let Some(member_name) = member_name {
            definitions.push(Definition::new(
                vec![name],
                member_name,
                member.byte_range(),
                value,
            ));
        }
    }
}

fn comments_parser() -> impl CommentsParser {
    let prisma_language = tree_sitter_prisma::LANGUAGE.into();
    languages::c_style_comments_parser(
        &prisma_language,
        &["comment", "developer_comment", "block_comment"],
        languages::SLASH_MARKERS,
    )
}

#[cfg(test)]
mod symbols_tests {
    use super::*;
    use crate::languages::code_symbols::test_utils::{
        decode_error, symbol, symbols, syntax_error, text_of, unresolved, value, values,
    };
    use crate::symbols::{Symbols, SymbolsParser};

    /// The symbols of `source`, a Prisma schema file.
    fn prisma(source: &str) -> anyhow::Result<Symbols> {
        CodeSymbolsParser::new(&tree_sitter_prisma::LANGUAGE.into(), Prisma).parse(source)
    }

    #[test]
    fn file_with_every_shape_parse_symbols_returns_every_symbol_in_order() -> anyhow::Result<()> {
        let source = r#"// Database settings.
datasource db {
  provider = "postgresql"
  url      = env("DATABASE_URL")
  schemas  = ["public", "auth"]
}

/// Client generator.
generator client {
  provider        = "prisma-client-js"
  output          = "../src/generated/prisma"
  previewFeatures = ["views", "multiSchema"]
  binaryTargets   = ["native"]
  strict          = true
  port            = 5432
}

/// Display colors.
enum Color {
  RED
  /// The default color.
  GREEN  @map("green")
  BLUE
  YELLOW

  @@map("color")
  @@schema("public")
}

/// A stored item.
model Item {
  id      String   @id @default(uuid())
  /// Shown in lists.
  name    String   @unique
  note    String?  @db.VarChar(255)
  tags    String[]
  raw     Unsupported("citext")?
  address Address?

  @@index([name])
  @@map("items")
  @@ignore
}

type Address {
  street String
  city   String
}

view ActiveItem {
  id   String @unique
  name String
}
"#;

        // <block affects="docs/symbols.md:prisma-paths">
        assert_eq!(
            symbols(&prisma(source)?, source),
            vec![
                symbol("/db", text_of(source, "datasource db", "\n}")?, None),
                symbol(
                    "/db/provider",
                    r#"provider = "postgresql""#,
                    Some("postgresql")
                ),
                symbol(
                    "/db/url",
                    r#"url      = env("DATABASE_URL")"#,
                    Some(r#"env("DATABASE_URL")"#)
                ),
                symbol(
                    "/db/schemas",
                    r#"schemas  = ["public", "auth"]"#,
                    Some(r#"["public", "auth"]"#)
                ),
                symbol("/client", text_of(source, "generator client", "\n}")?, None),
                symbol(
                    "/client/provider",
                    r#"provider        = "prisma-client-js""#,
                    Some("prisma-client-js")
                ),
                symbol(
                    "/client/output",
                    r#"output          = "../src/generated/prisma""#,
                    Some("../src/generated/prisma")
                ),
                symbol(
                    "/client/previewFeatures",
                    r#"previewFeatures = ["views", "multiSchema"]"#,
                    Some(r#"["views", "multiSchema"]"#)
                ),
                symbol(
                    "/client/binaryTargets",
                    r#"binaryTargets   = ["native"]"#,
                    Some(r#"["native"]"#)
                ),
                symbol("/client/strict", "strict          = true", Some("true")),
                symbol("/client/port", "port            = 5432", Some("5432")),
                symbol("/Color", text_of(source, "enum Color", "\n}")?, None),
                symbol("/Color/RED", "RED", None),
                symbol("/Color/GREEN", r#"GREEN  @map("green")"#, None),
                symbol("/Color/BLUE", "BLUE", None),
                symbol("/Color/YELLOW", "YELLOW", None),
                symbol("/Item", text_of(source, "model Item", "\n}")?, None),
                symbol("/Item/id", "id      String   @id @default(uuid())", None),
                symbol("/Item/name", "name    String   @unique", None),
                symbol("/Item/note", "note    String?  @db.VarChar(255)", None),
                symbol("/Item/tags", "tags    String[]", None),
                symbol("/Item/raw", r#"raw     Unsupported("citext")?"#, None),
                symbol("/Item/address", "address Address?", None),
                symbol("/Address", text_of(source, "type Address", "\n}")?, None),
                symbol("/Address/street", "street String", None),
                symbol("/Address/city", "city   String", None),
                symbol(
                    "/ActiveItem",
                    text_of(source, "view ActiveItem", "\n}")?,
                    None
                ),
                symbol("/ActiveItem/id", "id   String @unique", None),
                symbol("/ActiveItem/name", "name String", None),
            ]
        );
        // </block>
        Ok(())
    }

    #[test]
    fn strings_with_escapes_parse_symbols_decodes_json_escapes() -> anyhow::Result<()> {
        let source = concat!(
            r#"generator client {
  plain = "hello"
  escapes = "a\bb\fc\nd\re\tf\"g\\h\/i"
  unicode = "caf\u00e9 \uD83D\uDE00"
"#,
            // Prisma rejects every character below U+0020 in a string, but not U+007F.
            "  raw_delete = \"a\x7fb\"\n",
            "}\n",
        );

        assert_eq!(
            values(&prisma(source)?, source),
            vec![
                ("/client".to_string(), None),
                value("/client/plain", "hello"),
                value("/client/escapes", "a\u{8}b\u{c}c\nd\re\tf\"g\\h/i"),
                value("/client/unicode", "café 😀"),
                value("/client/raw_delete", "a\x7fb"),
            ]
        );
        Ok(())
    }

    #[test]
    fn invalid_strings_parse_symbols_marks_only_those_paths_unreadable() -> anyhow::Result<()> {
        const CONTROL_CHARACTER: &str =
            "Prisma does not allow a control character, such as a tab or a line break, in a string";
        let source = concat!(
            r#"generator client {
  ok = "good"
  single = 'bad'
  escape = "\q"
  short_unicode = "\u12"
  surrogate = "\uD800"
  multiline = "a\
b"
"#,
            "  raw_tab = \"a\tb\"\n",
            "  raw_unit_separator = \"a\x1fb\"\n",
            "}\n",
        );
        let parsed = prisma(source)?;

        assert_eq!(
            values(&parsed, source),
            vec![("/client".to_string(), None), value("/client/ok", "good")]
        );
        assert_eq!(
            unresolved(&parsed, source, "/client/single")?,
            decode_error(3, 12, "Prisma strings use double quotes")
        );
        assert_eq!(
            unresolved(&parsed, source, "/client/escape")?,
            decode_error(4, 12, "`\\q` is not an escape in a Prisma string")
        );
        assert_eq!(
            unresolved(&parsed, source, "/client/short_unicode")?,
            decode_error(5, 19, "`\\u12` is not an escape in a Prisma string")
        );
        assert_eq!(
            unresolved(&parsed, source, "/client/surrogate")?,
            decode_error(
                6,
                15,
                "the string holds U+D800, which is only half of a character"
            )
        );
        assert_eq!(
            unresolved(&parsed, source, "/client/multiline")?,
            decode_error(7, 15, CONTROL_CHARACTER)
        );
        assert_eq!(
            unresolved(&parsed, source, "/client/raw_tab")?,
            decode_error(9, 13, CONTROL_CHARACTER)
        );
        assert_eq!(
            unresolved(&parsed, source, "/client/raw_unit_separator")?,
            decode_error(10, 24, CONTROL_CHARACTER)
        );
        Ok(())
    }

    #[test]
    fn backslash_at_string_end_push_escape_returns_error() {
        let error = push_escape(&mut "".chars().peekable(), &mut Vec::new())
            .expect_err("a backslash with nothing after it is not an escape");

        assert_eq!(
            error.to_string(),
            "the string ends with a `\\` that escapes nothing"
        );
    }

    #[test]
    fn legacy_non_block_type_aliases_parse_symbols_returns_no_symbols() -> anyhow::Result<()> {
        let source = r"type LegacyString String
type LegacyAssign = String
";

        assert_eq!(symbols(&prisma(source)?, source), vec![]);
        Ok(())
    }

    #[test]
    fn duplicate_block_names_parse_symbols_reports_top_level_ambiguous_and_resolves_unique_members()
    -> anyhow::Result<()> {
        use crate::symbols::SymbolPath;

        let source = r#"datasource db {
  provider = "postgresql"
}

model db {
  id Int @id
}
"#;
        let parsed = prisma(source)?;

        let err = unresolved(&parsed, source, "/db")?;
        assert!(
            err.contains("ambiguous symbol, defined at 1:1, 5:1"),
            "unexpected error: {err}"
        );
        assert_eq!(
            parsed
                .resolve(&SymbolPath::parse("/db/provider")?)
                .expect("unique member should resolve")
                .value
                .as_ref()
                .map(|value| value.text.as_str()),
            Some("postgresql")
        );
        assert_eq!(
            parsed
                .resolve(&SymbolPath::parse("/db/id")?)
                .expect("unique member should resolve")
                .def_byte_ranges
                .iter()
                .map(|range| &source[range.clone()])
                .collect::<Vec<_>>(),
            vec!["id Int @id"]
        );
        Ok(())
    }

    #[test]
    fn field_named_like_keyword_and_relation_attribute_parse_symbols_returns_field()
    -> anyhow::Result<()> {
        let source = r"model Post {
  id       Int    @id @default(autoincrement())
  type     String
  parent   Item   @relation(fields: [parentId], references: [id], onDelete: Cascade)
  parentId Int
}
";

        assert_eq!(
            symbols(&prisma(source)?, source),
            vec![
                symbol("/Post", source.trim_end(), None),
                symbol(
                    "/Post/id",
                    "id       Int    @id @default(autoincrement())",
                    None
                ),
                symbol("/Post/type", "type     String", None),
                symbol(
                    "/Post/parent",
                    "parent   Item   @relation(fields: [parentId], references: [id], onDelete: Cascade)",
                    None
                ),
                symbol("/Post/parentId", "parentId Int", None),
            ]
        );
        Ok(())
    }

    #[test]
    fn syntax_error_parse_symbols_makes_the_definitions_that_hold_it_unreadable()
    -> anyhow::Result<()> {
        let source = r"model Broken {
  id     String @id
  broken @id
  name   String
}

enum BadEnum {
  OK
  =
}

enum After {
  READY
}
";
        let parsed = prisma(source)?;

        assert_eq!(
            symbols(&parsed, source),
            vec![
                symbol("/Broken/id", "id     String @id", None),
                symbol("/Broken/name", "name   String", None),
                symbol("/BadEnum/OK", "OK", None),
                symbol("/After", "enum After {\n  READY\n}", None),
                symbol("/After/READY", "READY", None),
            ]
        );
        assert_eq!(
            unresolved(&parsed, source, "/Broken")?,
            syntax_error(3, 9, "Broken")
        );
        assert_eq!(
            unresolved(&parsed, source, "/Broken/broken")?,
            syntax_error(3, 9, "broken")
        );
        assert_eq!(
            unresolved(&parsed, source, "/BadEnum")?,
            syntax_error(9, 3, "BadEnum")
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{languages::Comment, position::Position};

    #[test]
    fn parses_comments_correctly() {
        let mut comments_parser = comments_parser();

        let comments: Vec<Comment> = comments_parser
            .parse(
                r#"
// Line comment
//
/// Doc comment
//// Banner comment
//! Bang comment
datasource db {
  provider = "postgresql"
  url      = "postgresql://user:pass@localhost/db" // inline comment
}

/* Block comment
 * spanning lines
 */
"#,
            )
            .collect();

        assert_eq!(
            comments,
            vec![
                Comment {
                    position_range: Position::new(2, 1)..Position::new(2, 16),
                    source_range: 1..16,
                    comment_text: "   Line comment".to_string(),
                },
                Comment {
                    position_range: Position::new(3, 1)..Position::new(3, 3),
                    source_range: 17..19,
                    comment_text: "  ".to_string(),
                },
                Comment {
                    position_range: Position::new(4, 1)..Position::new(4, 16),
                    source_range: 20..35,
                    comment_text: "    Doc comment".to_string(),
                },
                Comment {
                    position_range: Position::new(5, 1)..Position::new(5, 20),
                    source_range: 36..55,
                    comment_text: "     Banner comment".to_string(),
                },
                Comment {
                    position_range: Position::new(6, 1)..Position::new(6, 17),
                    source_range: 56..72,
                    comment_text: "    Bang comment".to_string(),
                },
                Comment {
                    position_range: Position::new(9, 52)..Position::new(9, 69),
                    source_range: 166..183,
                    comment_text: "   inline comment".to_string(),
                },
                Comment {
                    position_range: Position::new(12, 1)..Position::new(14, 4),
                    source_range: 187..225,
                    comment_text: "   Block comment\n   spanning lines\n   ".to_string(),
                },
            ]
        );
    }
}
