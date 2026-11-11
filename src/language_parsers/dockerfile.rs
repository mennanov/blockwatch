use crate::language_parsers::{self, node_text};
use crate::language_parsers::{CommentsParser, LanguageParser, LanguageParserImpl};
use crate::symbols::ScalarValue;
use crate::symbols::code::{CodeLanguage, CodeSymbolsParser, Definition, Value};
use tree_sitter::Node;

/// Dockerfile (and Containerfile), whose files hold container build instructions such as `FROM`,
/// `ARG`, `ENV` and `LABEL`.
struct Dockerfile;

impl CodeLanguage for Dockerfile {
    fn definitions<'tree>(&self, root: Node<'tree>, source: &str) -> Vec<Definition<'tree>> {
        let mut definitions = Vec::new();
        let mut current_stage: Option<Node<'tree>> = None;
        let mut cursor = root.walk();

        for ins in root.named_children(&mut cursor) {
            match ins.kind() {
                "from_instruction" => {
                    let as_node = ins.child_by_field_name("as");
                    if let Some(as_node) = as_node {
                        let name = node_text(&as_node, source);
                        if !name.contains('$') {
                            let mut cursor = ins.walk();
                            let img_node = ins
                                .named_children(&mut cursor)
                                .find(|k| k.kind() == "image_spec");
                            definitions.push(Definition::new(
                                vec![],
                                as_node,
                                ins.byte_range(),
                                img_node,
                            ));
                            current_stage = Some(as_node);
                        } else {
                            current_stage = None;
                        }
                    } else {
                        current_stage = None;
                    }
                }
                "arg_instruction" => {
                    let parents = current_stage.map(|s| vec![s]).unwrap_or_default();
                    let mut cursor = ins.walk();
                    for pair in ins
                        .named_children(&mut cursor)
                        .filter(|k| k.kind() == "arg_pair")
                    {
                        let Some(name_node) = pair.child_by_field_name("name") else {
                            continue;
                        };
                        let value = match pair.child_by_field_name("default") {
                            Some(default_node) => Some(Value::Initializer(default_node)),
                            None if node_text(&pair, source).trim_end().ends_with('=') => {
                                Some(Value::Read(ScalarValue {
                                    text: String::new(),
                                    byte_range: pair.end_byte()..pair.end_byte(),
                                }))
                            }
                            None => continue,
                        };
                        definitions.push(Definition {
                            parents: parents.clone(),
                            names: vec![name_node],
                            range: pair.byte_range(),
                            value,
                        });
                    }
                }
                "env_instruction" => {
                    let parents = current_stage.map(|s| vec![s]).unwrap_or_default();
                    let mut cursor = ins.walk();
                    for pair in ins
                        .named_children(&mut cursor)
                        .filter(|k| k.kind() == "env_pair")
                    {
                        let Some(name_node) = pair.child_by_field_name("name") else {
                            continue;
                        };
                        let value = match pair.child_by_field_name("value") {
                            Some(val_node) => Some(Value::Initializer(val_node)),
                            None if node_text(&pair, source).trim_end().ends_with('=') => {
                                Some(Value::Read(ScalarValue {
                                    text: String::new(),
                                    byte_range: pair.end_byte()..pair.end_byte(),
                                }))
                            }
                            None => None,
                        };
                        definitions.push(Definition {
                            parents: parents.clone(),
                            names: vec![name_node],
                            range: pair.byte_range(),
                            value,
                        });
                    }
                }
                "label_instruction" => {
                    let parents = current_stage.map(|s| vec![s]).unwrap_or_default();
                    let mut cursor = ins.walk();
                    for pair in ins
                        .named_children(&mut cursor)
                        .filter(|k| k.kind() == "label_pair")
                    {
                        let Some(key_node) = pair.child_by_field_name("key") else {
                            continue;
                        };
                        let value = pair.child_by_field_name("value").map(Value::Initializer);
                        definitions.push(Definition {
                            parents: parents.clone(),
                            names: vec![key_node],
                            range: pair.byte_range(),
                            value,
                        });
                    }
                }
                _ => {}
            }
        }

        definitions
    }

    fn decode_string(&self, node: &Node, source: &str) -> anyhow::Result<Option<String>> {
        match node.kind() {
            "double_quoted_string" => {
                if has_expansion(*node) {
                    return Ok(None);
                }
                let text = node_text(node, source);
                let unquoted = crate::symbols::code::between_quotes(text, "\"")?;
                let escape = escape_directive(source);
                Ok(Some(unescape(unquoted, escape)))
            }
            "single_quoted_string" => {
                let text = node_text(node, source);
                let unquoted = crate::symbols::code::between_quotes(text, "'")?;
                Ok(Some(unquoted.to_string()))
            }
            "unquoted_string" => {
                if has_expansion(*node) {
                    return Ok(None);
                }
                let text = node_text(node, source);
                let escape = escape_directive(source);
                if text.contains(escape) {
                    Ok(Some(unescape(text, escape)))
                } else {
                    Ok(None)
                }
            }
            _ => Ok(None),
        }
    }

    const NAME_QUOTE: Option<char> = Some('"');
    const NAME_PUNCTUATION: &'static [char] = &['-', '.'];
}

/// Whether `node` or any descendant is an expansion such as `$VAR` or `${VAR}`.
fn has_expansion(node: Node<'_>) -> bool {
    if node.kind() == "expansion" {
        return true;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if has_expansion(child) {
            return true;
        }
    }
    false
}

/// The escape character for `source`, either `\` or `` ` ``.
fn escape_directive(source: &str) -> char {
    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('#') {
            let rest = rest.trim();
            if let Some(directive) = rest.strip_prefix("escape") {
                let directive = directive.trim_start();
                if let Some(val) = directive.strip_prefix('=') {
                    let val = val.trim();
                    if val.starts_with('`') {
                        return '`';
                    }
                    if val.starts_with('\\') {
                        return '\\';
                    }
                }
            }
            // Docker allows other parser directives such as `# syntax=` before `# escape=`.
            if !rest.starts_with("syntax") && !rest.starts_with("check") {
                break;
            }
        } else {
            break;
        }
    }
    '\\'
}

/// Unescapes `text` using `escape_char`. An escape character before a newline joins the line.
fn unescape(text: &str, escape_char: char) -> String {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == escape_char {
            match chars.next() {
                Some('\r') => {
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                }
                Some('\n') => {}
                Some(next_ch) => result.push(next_ch),
                None => result.push(ch),
            }
        } else {
            result.push(ch);
        }
    }
    result
}

/// Returns a [`LanguageParser`] for Dockerfile (Containerfile).
pub(super) fn parser() -> anyhow::Result<impl LanguageParser> {
    Ok(
        LanguageParserImpl::new(comments_parser()?).with_symbols(CodeSymbolsParser::new(
            &tree_sitter_containerfile::LANGUAGE.into(),
            Dockerfile,
        )?),
    )
}

fn comments_parser() -> anyhow::Result<impl CommentsParser> {
    let containerfile_language = tree_sitter_containerfile::LANGUAGE.into();
    let parser =
        language_parsers::python_style_comments_parser(&containerfile_language, "comment", &["#"])?
            .with_break_at_node_kinds(&["double_quoted_string", "single_quoted_string"])
            // Because of the bug in the Dockerfile tree-sitter grammar, the strings like
            // "# not a comment" are lexed as a comment. This is a workaround to ignore them.
            .with_break_when(|node, source_code| {
                node.kind() == "comment"
                    && node.start_byte() > 0
                    && source_code.as_bytes()[node.start_byte() - 1] == b'"'
            });
    Ok(parser)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Position, language_parsers::Comment};

    #[test]
    fn parses_comments_correctly() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser()?;

        // The `#` in the `RUN` instruction belongs to the shell command and must not be
        // extracted as a comment. Note that the grammar includes the trailing newline in
        // each comment node.
        let comments: Vec<Comment> = comments_parser
            .parse(
                r#"
# syntax=docker/dockerfile:1
FROM alpine:3.20

# This is a comment
RUN echo hi # not a comment

  # Indented comment
COPY . /app
## A section heading repeats the marker.
"#,
            )
            .collect();

        assert_eq!(
            comments,
            vec![
                Comment {
                    position_range: Position::new(2, 1)..Position::new(3, 1),
                    source_range: 1..30,
                    comment_text: "  syntax=docker/dockerfile:1\n".to_string()
                },
                Comment {
                    position_range: Position::new(5, 1)..Position::new(6, 1),
                    source_range: 48..68,
                    comment_text: "  This is a comment\n".to_string()
                },
                Comment {
                    position_range: Position::new(8, 3)..Position::new(9, 1),
                    source_range: 99..118,
                    comment_text: "  Indented comment\n".to_string()
                },
                Comment {
                    position_range: Position::new(10, 1)..Position::new(11, 1),
                    source_range: 130..171,
                    comment_text: "   A section heading repeats the marker.\n".to_string()
                },
            ]
        );

        Ok(())
    }

    #[test]
    fn hash_inside_a_double_quoted_argument_is_not_a_comment() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser()?;

        let comments: Vec<String> = comments_parser
            .parse(
                r##"FROM alpine:3.20
LABEL description="# 1 of a kind"

# The only comment here
COPY . /app
"##,
            )
            .map(|comment| comment.comment_text)
            .collect();

        assert_eq!(comments, vec!["  The only comment here\n".to_string()]);

        Ok(())
    }

    #[test]
    fn hash_inside_a_single_quoted_argument_is_not_a_comment() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser()?;

        let comments: Vec<String> = comments_parser
            .parse(
                r##"FROM alpine:3.20
LABEL summary='# also not a comment'

# The only comment here
COPY . /app
"##,
            )
            .map(|comment| comment.comment_text)
            .collect();

        assert_eq!(comments, vec!["  The only comment here\n".to_string()]);

        Ok(())
    }

    #[test]
    fn hash_opening_a_json_array_argument_is_not_a_comment() -> anyhow::Result<()> {
        let mut comments_parser = comments_parser()?;

        let comments: Vec<String> = comments_parser
            .parse(
                r##"FROM alpine:3.20
ENTRYPOINT ["#/bin/sh", "-c", "# neither of these is a comment"]

# The only comment here
COPY . /app
"##,
            )
            .map(|comment| comment.comment_text)
            .collect();

        assert_eq!(comments, vec!["  The only comment here\n".to_string()]);

        Ok(())
    }

    #[test]
    fn real_block_is_parsed_while_json_array_marker_is_ignored() -> anyhow::Result<()> {
        let contents = r##"FROM alpine:3.20
CMD ["sh", "-c", "# <block name='fake'> x # </block>"]
# <block name="real">
COPY . /app
# </block>
"##;
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
}

#[cfg(test)]
mod symbols_tests {
    use super::*;
    use crate::symbols::code::CodeSymbolsParser;
    use crate::symbols::code::test_utils::{symbol, symbols, unresolved};
    use crate::symbols::{Symbols, SymbolsParser};

    /// The symbols of `source`, a Dockerfile.
    fn dockerfile(source: &str) -> anyhow::Result<Symbols> {
        CodeSymbolsParser::new(&tree_sitter_containerfile::LANGUAGE.into(), Dockerfile)?
            .parse(source)
    }

    #[test]
    fn file_with_every_shape_parse_symbols_returns_every_symbol_in_order() -> anyhow::Result<()> {
        let source = r#"# syntax=docker/dockerfile:1
ARG GO_VERSION=1.25
ARG BASE="debian:bookworm-slim"

FROM golang:${GO_VERSION}-alpine AS builder
ARG GO_VERSION
ARG TARGETOS TARGETARCH=amd64
ENV CGO_ENABLED=0 GOFLAGS="-mod=vendor"
ENV LEGACY some value
LABEL org.opencontainers.image.version="2.1.0"
RUN go build -o /app ./cmd/app

FROM ghcr.io/astral-sh/uv:0.13.5 AS uv

FROM ${BASE}
COPY --from=builder /app /app
ENV APP_HOME="/srv/app" PATH="${APP_HOME}/bin:${PATH}"
ENTRYPOINT ["/app"]
"#;

        // <block affects="docs/symbols.md:dockerfile-paths, src/skill.md#/blockwatch/Symbols/Dockerfile">
        assert_eq!(
            symbols(&dockerfile(source)?, source),
            vec![
                symbol("/GO_VERSION", "GO_VERSION=1.25", Some("1.25")),
                symbol(
                    "/BASE",
                    r#"BASE="debian:bookworm-slim""#,
                    Some("debian:bookworm-slim")
                ),
                symbol(
                    "/builder",
                    "FROM golang:${GO_VERSION}-alpine AS builder",
                    Some("golang:${GO_VERSION}-alpine")
                ),
                symbol("/builder/TARGETARCH", "TARGETARCH=amd64", Some("amd64")),
                symbol("/builder/CGO_ENABLED", "CGO_ENABLED=0", Some("0")),
                symbol(
                    "/builder/GOFLAGS",
                    r#"GOFLAGS="-mod=vendor""#,
                    Some("-mod=vendor")
                ),
                symbol("/builder/LEGACY", "LEGACY some value", Some("some value")),
                symbol(
                    "/builder/org.opencontainers.image.version",
                    r#"org.opencontainers.image.version="2.1.0""#,
                    Some("2.1.0")
                ),
                symbol(
                    "/uv",
                    "FROM ghcr.io/astral-sh/uv:0.13.5 AS uv",
                    Some("ghcr.io/astral-sh/uv:0.13.5")
                ),
                symbol("/APP_HOME", r#"APP_HOME="/srv/app""#, Some("/srv/app")),
                symbol(
                    "/PATH",
                    r#"PATH="${APP_HOME}/bin:${PATH}""#,
                    Some(r#""${APP_HOME}/bin:${PATH}""#)
                ),
            ]
        );
        // </block>

        Ok(())
    }

    #[test]
    fn file_with_one_unnamed_stage_parse_symbols_puts_symbols_at_top_level() -> anyhow::Result<()> {
        let source = r#"FROM alpine:3.20
ARG PORT=8080
ENV HOST=localhost
LABEL maintainer=team
"#;

        assert_eq!(
            symbols(&dockerfile(source)?, source),
            vec![
                symbol("/PORT", "PORT=8080", Some("8080")),
                symbol("/HOST", "HOST=localhost", Some("localhost")),
                symbol("/maintainer", "maintainer=team", Some("team")),
            ]
        );

        Ok(())
    }

    #[test]
    fn two_unnamed_stages_setting_one_name_parse_symbols_marks_it_unresolved() -> anyhow::Result<()>
    {
        let source = r#"FROM alpine:3.20
ENV PORT=8080

FROM alpine:3.20
ENV PORT=9090
"#;

        assert_eq!(
            unresolved(&dockerfile(source)?, source, "/PORT")?,
            "ambiguous symbol, defined at 2:5, 5:5"
        );

        Ok(())
    }

    #[test]
    fn name_before_first_from_and_in_unnamed_stage_parse_symbols_marks_it_unresolved()
    -> anyhow::Result<()> {
        let source = r#"ARG PORT=8080

FROM alpine:3.20
ARG PORT=9090
"#;

        assert_eq!(
            unresolved(&dockerfile(source)?, source, "/PORT")?,
            "ambiguous symbol, defined at 1:5, 4:5"
        );

        Ok(())
    }

    #[test]
    fn arg_with_several_names_parse_symbols_returns_each_name_with_a_default() -> anyhow::Result<()>
    {
        let source = "ARG A=1 B=2 C\n";

        assert_eq!(
            symbols(&dockerfile(source)?, source),
            vec![
                symbol("/A", "A=1", Some("1")),
                symbol("/B", "B=2", Some("2")),
            ]
        );

        Ok(())
    }

    #[test]
    fn arg_without_default_next_to_arg_with_default_parse_symbols_skips_the_one_without_default()
    -> anyhow::Result<()> {
        let source = "ARG NO_DEFAULT\nARG HAS_DEFAULT=yes\n";

        assert_eq!(
            symbols(&dockerfile(source)?, source),
            vec![symbol("/HAS_DEFAULT", "HAS_DEFAULT=yes", Some("yes"))]
        );

        Ok(())
    }

    #[test]
    fn arg_and_env_with_one_name_parse_symbols_marks_it_unresolved() -> anyhow::Result<()> {
        let source = r#"FROM alpine:3.20 AS app
ARG VERSION=1.0
ENV VERSION=1.0
"#;

        assert_eq!(
            unresolved(&dockerfile(source)?, source, "/app/VERSION")?,
            "ambiguous symbol, defined at 2:5, 3:5"
        );

        Ok(())
    }

    #[test]
    fn both_env_forms_parse_symbols_returns_both() -> anyhow::Result<()> {
        let source = r#"FROM alpine:3.20
ENV KEY_EQUALS=val1
ENV KEY_SPACE val2
"#;

        assert_eq!(
            symbols(&dockerfile(source)?, source),
            vec![
                symbol("/KEY_EQUALS", "KEY_EQUALS=val1", Some("val1")),
                symbol("/KEY_SPACE", "KEY_SPACE val2", Some("val2")),
            ]
        );

        Ok(())
    }

    #[test]
    fn label_with_quoted_key_parse_symbols_returns_unquoted_key() -> anyhow::Result<()> {
        let source = r#"FROM alpine:3.20
LABEL "com.example.vendor"="ACME"
"#;

        assert_eq!(
            symbols(&dockerfile(source)?, source),
            vec![symbol(
                "/com.example.vendor",
                r#""com.example.vendor"="ACME""#,
                Some("ACME")
            )]
        );

        Ok(())
    }

    #[test]
    fn from_with_platform_tag_and_digest_parse_symbols_returns_image_spec_without_platform()
    -> anyhow::Result<()> {
        let source = "FROM --platform=linux/amd64 golang:1.23@sha256:abcdef AS build\n";

        assert_eq!(
            symbols(&dockerfile(source)?, source),
            vec![symbol(
                "/build",
                "FROM --platform=linux/amd64 golang:1.23@sha256:abcdef AS build",
                Some("golang:1.23@sha256:abcdef")
            )]
        );

        Ok(())
    }

    #[test]
    fn run_with_bracket_syntax_error_above_arg_parse_symbols_still_returns_arg()
    -> anyhow::Result<()> {
        let source = r#"FROM alpine:3.20
RUN [ -s "/tmp/file" ]; \
    echo ok
ARG PORT=8080
"#;

        assert_eq!(
            symbols(&dockerfile(source)?, source),
            vec![symbol("/PORT", "PORT=8080", Some("8080"))]
        );

        Ok(())
    }

    #[test]
    fn arg_with_empty_default_parse_symbols_returns_empty_string() -> anyhow::Result<()> {
        let source = "ARG EMPTY=\n";

        assert_eq!(
            symbols(&dockerfile(source)?, source),
            vec![symbol("/EMPTY", "EMPTY=", Some(""))]
        );

        Ok(())
    }

    #[test]
    fn env_with_empty_value_parse_symbols_returns_empty_string() -> anyhow::Result<()> {
        let source = "ENV EMPTY=\n";

        assert_eq!(
            symbols(&dockerfile(source)?, source),
            vec![symbol("/EMPTY", "EMPTY=", Some(""))]
        );

        Ok(())
    }

    #[test]
    fn instruction_after_onbuild_parse_symbols_is_not_a_symbol() -> anyhow::Result<()> {
        let source = "ONBUILD ARG FOO=bar\nONBUILD ENV BAZ=qux\n";

        assert_eq!(symbols(&dockerfile(source)?, source), vec![]);

        Ok(())
    }

    #[test]
    fn hash_in_string_parse_symbols_keeps_hash() -> anyhow::Result<()> {
        let source = r#"ENV FOO="hello#world"
LABEL description="v1#2"
"#;

        assert_eq!(
            symbols(&dockerfile(source)?, source),
            vec![
                symbol("/FOO", r#"FOO="hello#world""#, Some("hello#world")),
                symbol("/description", r#"description="v1#2""#, Some("v1#2")),
            ]
        );

        Ok(())
    }

    #[test]
    fn double_quoted_string_with_escapes_and_continuation_decodes_correctly() -> anyhow::Result<()>
    {
        let source = r#"ENV ESCAPED="hello\"world"
ENV BACKSLASH="hello\\world"
ENV CONTINUATION="line1\
line2"
"#;

        assert_eq!(
            symbols(&dockerfile(source)?, source),
            vec![
                symbol(
                    "/ESCAPED",
                    r#"ESCAPED="hello\"world""#,
                    Some(r#"hello"world"#)
                ),
                symbol(
                    "/BACKSLASH",
                    r#"BACKSLASH="hello\\world""#,
                    Some(r#"hello\world"#)
                ),
                symbol(
                    "/CONTINUATION",
                    "CONTINUATION=\"line1\\\nline2\"",
                    Some("line1line2")
                ),
            ]
        );

        Ok(())
    }

    #[test]
    fn single_quoted_string_decodes_without_reading_escapes() -> anyhow::Result<()> {
        let source = r#"ENV SINGLE='hello\world'
ENV CONTINUATION='line1\
line2'
"#;

        assert_eq!(
            symbols(&dockerfile(source)?, source),
            vec![
                symbol("/SINGLE", r#"SINGLE='hello\world'"#, Some(r#"hello\world"#)),
                symbol(
                    "/CONTINUATION",
                    "CONTINUATION='line1\\\nline2'",
                    Some("line1\\\nline2")
                ),
            ]
        );

        Ok(())
    }

    #[test]
    fn file_with_escape_directive_uses_backtick() -> anyhow::Result<()> {
        let source = r#"# escape=`
ENV WIN_PATH=C:\Windows\Temp
ENV ESCAPED="hello`"world"
ENV CONTINUATION="line1`
line2"
"#;

        assert_eq!(
            symbols(&dockerfile(source)?, source),
            vec![
                symbol(
                    "/WIN_PATH",
                    r#"WIN_PATH=C:\Windows\Temp"#,
                    Some(r#"C:\Windows\Temp"#)
                ),
                symbol(
                    "/ESCAPED",
                    r#"ESCAPED="hello`"world""#,
                    Some(r#"hello"world"#)
                ),
                symbol(
                    "/CONTINUATION",
                    "CONTINUATION=\"line1`\nline2\"",
                    Some("line1line2")
                ),
            ]
        );

        Ok(())
    }

    #[test]
    fn double_quoted_and_unquoted_string_with_expansion_keeps_quotes_and_as_written()
    -> anyhow::Result<()> {
        let source = r#"ENV DOUBLE="$FOO"
ENV BRACED="${FOO:-default}"
ENV UNQUOTED=$FOO
ENV UNQUOTED_BRACED=${FOO:-default}
"#;

        assert_eq!(
            symbols(&dockerfile(source)?, source),
            vec![
                symbol("/DOUBLE", r#"DOUBLE="$FOO""#, Some(r#""$FOO""#)),
                symbol(
                    "/BRACED",
                    r#"BRACED="${FOO:-default}""#,
                    Some(r#""${FOO:-default}""#)
                ),
                symbol("/UNQUOTED", "UNQUOTED=$FOO", Some("$FOO")),
                symbol(
                    "/UNQUOTED_BRACED",
                    "UNQUOTED_BRACED=${FOO:-default}",
                    Some("${FOO:-default}")
                ),
            ]
        );

        Ok(())
    }

    #[test]
    fn single_quoted_string_with_dollar_keeps_literal_dollar() -> anyhow::Result<()> {
        let source = r#"ENV SINGLE='$FOO'
ENV SINGLE_BRACED='${FOO}'
"#;

        assert_eq!(
            symbols(&dockerfile(source)?, source),
            vec![
                symbol("/SINGLE", "SINGLE='$FOO'", Some("$FOO")),
                symbol("/SINGLE_BRACED", "SINGLE_BRACED='${FOO}'", Some("${FOO}")),
            ]
        );

        Ok(())
    }

    #[test]
    fn stage_with_dollar_in_name_is_not_a_symbol_and_names_sit_at_top_level() -> anyhow::Result<()>
    {
        let source = "FROM alpine:3.20 AS $stage\nARG PORT=8080\n";

        assert_eq!(
            symbols(&dockerfile(source)?, source),
            vec![symbol("/PORT", "PORT=8080", Some("8080"))]
        );

        Ok(())
    }

    #[test]
    fn unquoted_string_with_escape_and_line_continuation_decodes_correctly() -> anyhow::Result<()> {
        let source = "ENV ESCAPED=hello\\ world\nENV CONTINUATION=line1\\\nline2\n";

        assert_eq!(
            symbols(&dockerfile(source)?, source),
            vec![
                symbol("/ESCAPED", "ESCAPED=hello\\ world", Some("hello world")),
                symbol(
                    "/CONTINUATION",
                    "CONTINUATION=line1\\\nline2",
                    Some("line1line2")
                ),
            ]
        );

        Ok(())
    }

    #[test]
    fn label_with_empty_value_parse_symbols_returns_empty_string() -> anyhow::Result<()> {
        let source = "LABEL EMPTY=\"\"\n";

        assert_eq!(
            symbols(&dockerfile(source)?, source),
            vec![symbol("/EMPTY", "EMPTY=\"\"", Some(""))]
        );

        Ok(())
    }

    #[test]
    fn escape_directive_with_leading_empty_line_uses_backtick() -> anyhow::Result<()> {
        let source = "\n# escape=`\nENV WIN_PATH=C:\\Windows\\Temp\n";

        assert_eq!(
            symbols(&dockerfile(source)?, source),
            vec![symbol(
                "/WIN_PATH",
                "WIN_PATH=C:\\Windows\\Temp",
                Some("C:\\Windows\\Temp")
            )]
        );

        Ok(())
    }
}
