use crate::language_parsers::{
    CommentsParser, LanguageParser, LanguageParserImpl, TreeSitterCommentsParser,
    blank_line_marker, comment_visitor, node_text, walk_tree,
};
use crate::symbols::ScalarValue;
use crate::symbols::code::{self, CodeLanguage, CodeSymbolsParser, Definition, Value};
use anyhow::bail;
use std::ops::Range;
use tree_sitter::Node;

/// Returns a [`LanguageParser`] for Bash, with its symbols.
pub(super) fn parser() -> impl LanguageParser {
    LanguageParserImpl::new(comments_parser()).with_symbols(CodeSymbolsParser::new(
        &tree_sitter_bash::LANGUAGE.into(),
        Bash,
    ))
}

/// Returns a [`LanguageParser`] for zsh, with Bash's comments and no symbols. The Bash grammar
/// misreads much of zsh's syntax, so a path into a zsh file could resolve to the wrong value.
pub(super) fn zsh_parser() -> impl LanguageParser {
    LanguageParserImpl::new(comments_parser())
}

/// Bash, whose top-level names are variables and functions.
struct Bash;

impl CodeLanguage for Bash {
    /// The definitions under `root`, the root of a Bash file's syntax tree, outside functions,
    /// loops and subshells, in document order. `source` is the text of the file.
    fn definitions<'tree>(&self, root: Node<'tree>, source: &str) -> Vec<Definition<'tree>> {
        let mut definitions = Vec::new();
        walk_tree(root, |node| {
            // A statement that ends with `&` runs in the background, in a subshell.
            if node.next_sibling().is_some_and(|next| next.kind() == "&") {
                return false;
            }
            match node.kind() {
                // The shell runs these in the script itself, so a name they set stays set. A loop
                // sets a name each time it runs, and a subshell's names never reach the script.
                "program"
                | "if_statement"
                | "elif_clause"
                | "else_clause"
                | "case_statement"
                | "case_item"
                | "compound_statement"
                | "list"
                | "negated_command"
                | "redirected_statement"
                | "variable_assignments" => {
                    return true;
                }
                "variable_assignment" => definitions.extend(assignment(node, node.byte_range())),
                "declaration_command" => definitions.extend(declarations(node, source)),
                "function_definition" => {
                    definitions
                        .extend(node.child_by_field_name("name").map(|name| {
                            Definition::new(Vec::new(), name, node.byte_range(), None)
                        }));
                }
                _ => {}
            }
            false
        });
        definitions
    }

    /// Decodes a value that bash reads as plain text: a word, a quoted string, or several written
    /// together. Returns `None` for any other node, and for a value that bash expands, such as
    /// `$HOME`, `` `date` `` or `~/bin`.
    ///
    /// # Errors
    /// Returns an error for `$'...'` and `$"..."`, alone or written together with other parts.
    fn decode_string(&self, node: &Node, source: &str) -> anyhow::Result<Option<String>> {
        let text = node_text(node, source);
        match node.kind() {
            // The grammar gives a `$` at the end of a value no part of its own, so the parts
            // would leave it out.
            "concatenation" if node.children(&mut node.walk()).any(|part| !part.is_named()) => {
                Ok(None)
            }
            "concatenation" => code::decode_concatenated(node, source, &|part, source| {
                self.decode_string(part, source)
            }),
            // In an assignment, a `~` at the start or after a `:` stands for a home directory.
            "word" if text.starts_with('~') || text.contains(":~") => Ok(None),
            "word" | "number" => Ok(Some(unescape(text, |_| true))),
            "raw_string" => Ok(Some(code::between_quotes(text, "'")?.to_string())),
            "string" => {
                let mut cursor = node.walk();
                if node
                    .named_children(&mut cursor)
                    .any(|part| part.kind() != "string_content")
                {
                    return Ok(None);
                }
                // In double quotes, a backslash only escapes these characters.
                Ok(Some(unescape(code::between_quotes(text, "\"")?, |c| {
                    matches!(c, '$' | '`' | '"' | '\\' | '\n')
                })))
            }
            "ansi_c_string" => bail!("blockwatch doesn't decode the escapes of `$'...'`"),
            "translated_string" => bail!(r#"bash translates `$"..."` when the script runs"#),
            _ => Ok(None),
        }
    }

    // A function's name can hold these too, such as `my-tool.run` or `ns::helper`.
    const NAME_PUNCTUATION: &'static [char] = &['-', '.', ':'];
}

/// `text` without each backslash before a character that `is_escaped` accepts. A backslash before
/// a line end goes away with the line end, as the line goes on in the next one.
fn unescape(text: &str, is_escaped: impl Fn(char) -> bool) -> String {
    let mut unescaped = String::with_capacity(text.len());
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        match characters.peek() {
            Some(&next) if character == '\\' && is_escaped(next) => {
                characters.next();
                if next != '\n' {
                    unescaped.push(next);
                }
            }
            _ => unescaped.push(character),
        }
    }
    unescaped
}

/// The definition that `assignment`, such as `NAME=value`, makes, covering `range`. Returns
/// `None` when it sets no whole name, as `NAME+=value` and `A[0]=x` do.
fn assignment(assignment: Node<'_>, range: Range<usize>) -> Option<Definition<'_>> {
    let name = assignment
        .child_by_field_name("name")
        .filter(|name| name.kind() == "variable_name")?;
    let mut cursor = assignment.walk();
    // `+=` adds to a value set somewhere else.
    if assignment
        .children(&mut cursor)
        .any(|child| child.kind() == "+=")
    {
        return None;
    }
    let value = match assignment.child_by_field_name("value") {
        Some(value) => Value::Initializer(value),
        // `NAME=` sets the name to an empty string.
        None => Value::Read(ScalarValue {
            text: String::new(),
            byte_range: assignment.end_byte()..assignment.end_byte(),
        }),
    };
    Some(Definition {
        parents: Vec::new(),
        names: vec![name],
        range,
        value: Some(value),
    })
}

/// The definitions that `declaration`, such as `export A=1 B=2`, makes, in document order. A
/// definition covers the whole statement when it is the statement's only name. `local` makes
/// none, as it only works inside a function.
fn declarations<'tree>(declaration: Node<'tree>, source: &str) -> Vec<Definition<'tree>> {
    if declaration
        .child(0)
        .is_some_and(|keyword| keyword.kind() == "local")
    {
        return Vec::new();
    }
    let mut cursor = declaration.walk();
    // Flags such as `-r` turn an attribute on, and `+r` turns it off.
    let names: Vec<Node> = declaration
        .named_children(&mut cursor)
        .filter(|child| !node_text(child, source).starts_with(['-', '+']))
        .collect();
    code::part_ranges(declaration.byte_range(), names)
        .filter(|(name, _)| name.kind() == "variable_assignment")
        .filter_map(|(name, range)| assignment(name, range))
        .collect()
}

fn comments_parser() -> impl CommentsParser {
    let bash_language = tree_sitter_bash::LANGUAGE.into();
    TreeSitterCommentsParser::new(
        &bash_language,
        comment_visitor(|node, source| {
            if node.kind() != "comment" {
                return None;
            }
            let comment = node_text(node, source);
            // A shebang works only as the first line of a file. Any other `#!` line is a comment.
            if node.start_byte() == 0 && comment.starts_with("#!") {
                None
            } else {
                blank_line_marker(comment, &["#"])
            }
        }),
    )
}

#[cfg(test)]
mod symbols_tests {
    use super::*;
    use crate::symbols::code::test_utils::{
        decode_error, symbol, symbols, syntax_error, unresolved, value, values,
    };
    use crate::symbols::{Symbols, SymbolsParser};

    /// The symbols of `source`, a Bash file.
    fn bash(source: &str) -> anyhow::Result<Symbols> {
        CodeSymbolsParser::new(&tree_sitter_bash::LANGUAGE.into(), Bash).parse(source)
    }

    #[test]
    fn file_with_every_shape_parse_symbols_returns_every_symbol_in_order() -> anyhow::Result<()> {
        let source = r#"#!/usr/bin/env bash
set -euo pipefail

# Doc.
GO_VERSION=1.22.5
KIND_VERSION="v0.23.0"
export REGISTRY='ghcr.io/acme'
readonly IMAGE="${REGISTRY}/app:${GO_VERSION}"
declare -r MODE=release
typeset +x COUNT=3
export LEFT=1 RIGHT=2
export NAME_ONLY TAIL=3
PORT="${PORT:-8080}"
TARGETS=(linux darwin)
EMPTY=
FIRST=1 SECOND=2
TARGETS+=(windows)
TARGETS[0]=freebsd
export GO_VERSION
FOO=1 make build
local SCOPED=1
: "${DEFAULTED:=1}"

if FOUND=$(command -v go); then
  SED=gsed
elif [[ -n "${CI:-}" ]]; then
  ELIF_NAME=1
else
  SED=sed
fi

case "$OS" in
  linux) CASE_NAME=1 ;;
esac
{ GROUPED=1; }
true && AND_NAME=1 || OR_NAME=2
{ REDIRECTED=1; } >/dev/null
if true; then REDIRECTED_IF=1; fi 2>/dev/null
! NEGATED=1
BACKGROUND=1 & FOREGROUND=2
{ BACKGROUND_GROUP=1; } &
true && BACKGROUND_LIST=1 &

log() { echo "$@" >&2; }

function setup {
  INSIDE=1
}

function teardown() (
  rm -rf "$TMP"
)

(SUBSHELL=1)
echo x | PIPED=1
while [[ $# -gt 0 ]]; do
  case "$1" in
    --kind) KIND_VERSION="$2"; shift 2 ;;
  esac
done
for target in "${TARGETS[@]}"; do LOOPED=1; done
until true; do UNTIL_NAME=1; done
for ((i = 0; i < 3; i++)); do C_LOOPED=1; done
"#;

        // <block affects="docs/symbols.md:shell-paths">
        assert_eq!(
            symbols(&bash(source)?, source),
            vec![
                symbol("/GO_VERSION", "GO_VERSION=1.22.5", Some("1.22.5")),
                symbol(
                    "/KIND_VERSION",
                    r#"KIND_VERSION="v0.23.0""#,
                    Some("v0.23.0")
                ),
                symbol(
                    "/REGISTRY",
                    "export REGISTRY='ghcr.io/acme'",
                    Some("ghcr.io/acme")
                ),
                symbol(
                    "/IMAGE",
                    r#"readonly IMAGE="${REGISTRY}/app:${GO_VERSION}""#,
                    Some(r#""${REGISTRY}/app:${GO_VERSION}""#)
                ),
                symbol("/MODE", "declare -r MODE=release", Some("release")),
                symbol("/COUNT", "typeset +x COUNT=3", Some("3")),
                // A statement with several names covers only the one it defines.
                symbol("/LEFT", "LEFT=1", Some("1")),
                symbol("/RIGHT", "RIGHT=2", Some("2")),
                symbol("/TAIL", "TAIL=3", Some("3")),
                symbol(
                    "/PORT",
                    r#"PORT="${PORT:-8080}""#,
                    Some(r#""${PORT:-8080}""#)
                ),
                symbol("/TARGETS", "TARGETS=(linux darwin)", Some("(linux darwin)")),
                symbol("/EMPTY", "EMPTY=", Some("")),
                symbol("/FIRST", "FIRST=1", Some("1")),
                symbol("/SECOND", "SECOND=2", Some("2")),
                // A condition runs in the script, as the bodies of `if` do.
                symbol("/FOUND", "FOUND=$(command -v go)", Some("$(command -v go)")),
                symbol("/SED", "SED=gsed", Some("gsed")),
                symbol("/ELIF_NAME", "ELIF_NAME=1", Some("1")),
                symbol("/SED", "SED=sed", Some("sed")),
                symbol("/CASE_NAME", "CASE_NAME=1", Some("1")),
                symbol("/GROUPED", "GROUPED=1", Some("1")),
                symbol("/AND_NAME", "AND_NAME=1", Some("1")),
                symbol("/OR_NAME", "OR_NAME=2", Some("2")),
                symbol("/REDIRECTED", "REDIRECTED=1", Some("1")),
                symbol("/REDIRECTED_IF", "REDIRECTED_IF=1", Some("1")),
                symbol("/NEGATED", "NEGATED=1", Some("1")),
                // A statement that ends with `&` runs in a subshell.
                symbol("/FOREGROUND", "FOREGROUND=2", Some("2")),
                symbol("/log", r#"log() { echo "$@" >&2; }"#, None),
                symbol("/setup", "function setup {\n  INSIDE=1\n}", None),
                symbol(
                    "/teardown",
                    "function teardown() (\n  rm -rf \"$TMP\"\n)",
                    None
                ),
            ]
        );
        // </block>
        Ok(())
    }

    #[test]
    fn value_kinds_parse_symbols_returns_decoded_or_written_values() -> anyhow::Result<()> {
        let source = r#"WORD=v1.2
ESCAPED_WORD=a\ b\q\"
SINGLE='a b\n$x'
DOUBLE="a\$b\q\"c\\"
DOUBLE_BACKTICK="a\`b"
SPLIT="a\
b"
JOINED="a"'b'c
QUOTE_IN_SINGLE='a'\''b'
EMPTY_DOUBLE=""
EMPTY_SINGLE=''
NUMBER=-5
NUMBER_JOINED="v"1
GLOB=*.txt
TILDE_INSIDE=a~b
TILDE_AFTER_EQUALS=a=~/b
QUOTED_TILDE="~/bin"
DOLLAR_IN_STRING="a$"
HOME_DIR=~/bin
PATH_LIST=a:~/bin"c"
TILDE_AFTER_QUOTE="a"~/b
DEFAULTED="${PORT:-8080}"
VARIABLE=$HOME
BACKTICKS=`date`
COMMAND_IN_STRING="a$(date)"
LONE_DOLLAR="a $ b"
JOINED_VARIABLE="a"$HOME
TRAILING_DOLLAR=5$
JOINED_DOLLAR="a"$
ARRAY=(a b)
"#;

        assert_eq!(
            values(&bash(source)?, source),
            vec![
                value("/WORD", "v1.2"),
                value("/ESCAPED_WORD", r#"a bq""#),
                value("/SINGLE", r"a b\n$x"),
                value("/DOUBLE", r#"a$b\q"c\"#),
                value("/DOUBLE_BACKTICK", "a`b"),
                value("/SPLIT", "ab"),
                value("/JOINED", "abc"),
                value("/QUOTE_IN_SINGLE", "a'b"),
                value("/EMPTY_DOUBLE", ""),
                value("/EMPTY_SINGLE", ""),
                value("/NUMBER", "-5"),
                value("/NUMBER_JOINED", "v1"),
                // An assignment expands no glob.
                value("/GLOB", "*.txt"),
                value("/TILDE_INSIDE", "a~b"),
                value("/TILDE_AFTER_EQUALS", "a=~/b"),
                value("/QUOTED_TILDE", "~/bin"),
                value("/DOLLAR_IN_STRING", "a$"),
                // The rest are kept as written. A `~` at the start or after a `:` is a home
                // directory.
                value("/HOME_DIR", "~/bin"),
                value("/PATH_LIST", r#"a:~/bin"c""#),
                // Bash doesn't expand this `~`, but any word that starts with `~` is kept.
                value("/TILDE_AFTER_QUOTE", r#""a"~/b"#),
                value("/DEFAULTED", r#""${PORT:-8080}""#),
                value("/VARIABLE", "$HOME"),
                value("/BACKTICKS", "`date`"),
                value("/COMMAND_IN_STRING", r#""a$(date)""#),
                // Bash reads `a $ b`, but the grammar reads `$ b` as a variable.
                value("/LONE_DOLLAR", r#""a $ b""#),
                value("/JOINED_VARIABLE", r#""a"$HOME"#),
                // The grammar leaves a `$` at the end out of the parts it gives.
                value("/TRAILING_DOLLAR", "5$"),
                value("/JOINED_DOLLAR", r#""a"$"#),
                value("/ARRAY", "(a b)"),
            ]
        );
        Ok(())
    }

    #[test]
    fn string_bash_decodes_at_run_time_parse_symbols_makes_the_name_unreadable()
    -> anyhow::Result<()> {
        let source = r#"ANSI_C=$'a\n'
TRANSLATED=$"hello"
JOINED="a"$'\t'
FINE=ok
"#;

        assert_eq!(
            symbols(&bash(source)?, source),
            vec![symbol("/FINE", "FINE=ok", Some("ok"))]
        );
        assert_eq!(
            unresolved(&bash(source)?, source, "/ANSI_C")?,
            decode_error(1, 8, "blockwatch doesn't decode the escapes of `$'...'`")
        );
        assert_eq!(
            unresolved(&bash(source)?, source, "/TRANSLATED")?,
            decode_error(2, 12, r#"bash translates `$"..."` when the script runs"#)
        );
        assert_eq!(
            unresolved(&bash(source)?, source, "/JOINED")?,
            decode_error(3, 8, "blockwatch doesn't decode the escapes of `$'...'`")
        );
        Ok(())
    }

    #[test]
    fn span_that_swallows_definitions_parse_symbols_makes_their_names_unreadable()
    -> anyhow::Result<()> {
        // The parser skips everything from `$((` up to the body of `helper`. A function's name can
        // hold `-`, `.` and `:`, so the whole word counts as a name.
        let source = r#"BEFORE=1
echo $(( 1 + )) ns::helper my-tool.run
helper() { :; }
AFTER=2
"#;

        assert_eq!(
            symbols(&bash(source)?, source),
            vec![
                symbol("/BEFORE", "BEFORE=1", Some("1")),
                symbol("/AFTER", "AFTER=2", Some("2")),
            ]
        );
        for name in ["helper", "ns::helper", "my-tool.run"] {
            assert_eq!(
                unresolved(&bash(source)?, source, &format!("/{name}"))?,
                syntax_error(2, 6, name)
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Position;
    use crate::language_parsers::Comment;

    #[test]
    fn parses_comments_correctly() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser();

        let blocks: Vec<Comment> = comments_parser
            .parse(
                r#"#!/bin/bash
# This is a single line comment
echo "Hello"  # This is an inline comment

# This is a multi-line
# comment that spans
# several lines

VALUE=42  # Comment after code
## A section heading repeats the marker.
#! Only the first line of a file holds a shebang.
"#,
            )
            .collect();

        assert_eq!(
            blocks,
            vec![
                Comment {
                    position_range: Position::new(2, 1)..Position::new(2, 32),
                    source_range: 12..43,
                    comment_text: "  This is a single line comment".to_string()
                },
                Comment {
                    position_range: Position::new(3, 15)..Position::new(3, 42),
                    source_range: 58..85,
                    comment_text: "  This is an inline comment".to_string()
                },
                Comment {
                    position_range: Position::new(5, 1)..Position::new(5, 23),
                    source_range: 87..109,
                    comment_text: "  This is a multi-line".to_string()
                },
                Comment {
                    position_range: Position::new(6, 1)..Position::new(6, 21),
                    source_range: 110..130,
                    comment_text: "  comment that spans".to_string()
                },
                Comment {
                    position_range: Position::new(7, 1)..Position::new(7, 16),
                    source_range: 131..146,
                    comment_text: "  several lines".to_string()
                },
                Comment {
                    position_range: Position::new(9, 11)..Position::new(9, 31),
                    source_range: 158..178,
                    comment_text: "  Comment after code".to_string()
                },
                Comment {
                    position_range: Position::new(10, 1)..Position::new(10, 41),
                    source_range: 179..219,
                    comment_text: "   A section heading repeats the marker.".to_string()
                },
                Comment {
                    position_range: Position::new(11, 1)..Position::new(11, 50),
                    source_range: 220..269,
                    comment_text: " ! Only the first line of a file holds a shebang.".to_string()
                },
            ]
        );

        Ok(())
    }

    #[test]
    fn comment_at_the_start_that_is_not_a_shebang_parse_returns_it() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser();

        // Only a `#!` at the first byte of the file is a shebang.
        for (source, expected) in [
            ("# Not a shebang.\n", "  Not a shebang."),
            ("\n#! Not a shebang either.\n", " ! Not a shebang either."),
        ] {
            let texts: Vec<String> = comments_parser
                .parse(source)
                .map(|comment| comment.comment_text)
                .collect();

            assert_eq!(texts, [expected], "{source:?}");
        }
        Ok(())
    }
}
