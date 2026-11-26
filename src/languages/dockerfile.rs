use crate::languages::code_symbols::{self, Definition, Value};
use crate::languages::{
    self, CommentsParser, LanguageParser, LanguageParserImpl, node_text, tree_sitter_parser,
    walk_tree,
};
use crate::symbols::{ScalarValue, Symbol, SymbolPath, Symbols, SymbolsParser};
use anyhow::Context;
use tree_sitter::Node;

/// Finds the symbols of a Dockerfile (or Containerfile): stages, `ARG`s with defaults, `ENV`s
/// and `LABEL`s.
struct DockerfileSymbolsParser {
    parser: tree_sitter::Parser,
}

impl DockerfileSymbolsParser {
    /// A parser for Dockerfiles and Containerfiles.
    fn new() -> Self {
        Self {
            parser: tree_sitter_parser(&tree_sitter_containerfile::LANGUAGE.into()),
        }
    }
}

impl SymbolsParser for DockerfileSymbolsParser {
    fn parse(&mut self, source: &str) -> anyhow::Result<Symbols> {
        let tree = self
            .parser
            .parse(source, None)
            .context("failed to parse syntax tree")?;
        let (definitions, stages) = collect_instructions(tree.root_node(), source);
        let mut parsed = code_symbols::symbols(
            &tree,
            source,
            &definitions,
            &decode_string,
            Some('"'),
            &['-', '.'],
        );
        stages.merge_into(&mut parsed);
        Ok(parsed)
    }
}

/// Positional stage symbols (`/0`, `/1`, ...) collected while walking a Dockerfile's top-level
/// instructions.
#[derive(Default)]
struct StageIndices {
    next_index: usize,
    broken_reason: Option<String>,
    symbols: Vec<Symbol>,
    unreadable: Vec<(SymbolPath, String)>,
}

impl StageIndices {
    /// Records a parsed `FROM` instruction as the next 0-based stage symbol, or marks its index
    /// unreadable when it or an earlier skipped `FROM` has a syntax error.
    fn record_stage(&mut self, from: Node<'_>, source: &str) {
        let path = SymbolPath::from_segments(vec![self.next_index.to_string()]);
        self.next_index += 1;
        if let Some(reason) = self.stage_error(from, source) {
            self.unreadable.push((path, reason));
        } else {
            self.symbols.push(Symbol {
                path,
                def_byte_ranges: vec![from.byte_range()],
                value: stage_image(from).map(|image| ScalarValue {
                    text: node_text(&image, source).to_string(),
                    byte_range: image.byte_range(),
                }),
            });
        }
    }

    /// Records any skipped `FROM` lines in a top-level `ERROR` node, marking their indices and
    /// all later stage indices unreadable. When `skip_first_line` is true, the error started
    /// mid-line after the previous instruction, so only its later lines can start a `FROM`.
    fn record_error(&mut self, error: Node<'_>, skip_first_line: bool, source: &str) {
        for line in node_text(&error, source)
            .lines()
            .skip(usize::from(skip_first_line))
        {
            if !is_from_line(line.trim_start_matches([' ', '\t'])) {
                continue;
            }
            let reason = self
                .broken_reason
                .get_or_insert_with(|| {
                    code_symbols::syntax_error_reason(
                        code_symbols::position(&error, source),
                        "FROM",
                    )
                })
                .clone();
            let path = SymbolPath::from_segments(vec![self.next_index.to_string()]);
            self.next_index += 1;
            self.unreadable.push((path, reason));
        }
    }

    /// Merges the collected stage symbols and unreadable indices into `parsed`, keeping document
    /// order with each `/<index>` ahead of its named alias.
    fn merge_into(mut self, parsed: &mut Symbols) {
        if !self.symbols.is_empty() {
            self.symbols.extend(std::mem::take(&mut parsed.symbols));
            self.symbols
                .sort_by_key(|symbol| symbol.def_byte_ranges[0].start);
            parsed.symbols = self.symbols;
        }
        for (path, reason) in self.unreadable {
            parsed.unreadable.entry(path).or_insert(reason);
        }
    }

    /// Why `from` cannot be read reliably, if an earlier `FROM` was skipped or `from` itself
    /// holds or ends at a syntax error.
    fn stage_error(&self, from: Node<'_>, source: &str) -> Option<String> {
        if let Some(reason) = &self.broken_reason {
            return Some(reason.clone());
        }
        if from.has_error() {
            return Some(code_symbols::syntax_error_reason(
                code_symbols::position(&from, source),
                "FROM",
            ));
        }
        error_after(from).map(|error| {
            code_symbols::undecodable_reason(
                code_symbols::position(&error, source),
                "the instruction has a syntax error here, so its value could be cut short",
            )
        })
    }
}

/// Walks the top-level nodes under `root`, collecting `code_symbols` definitions and positional
/// stage symbols in document order.
fn collect_instructions<'tree>(
    root: Node<'tree>,
    source: &str,
) -> (Vec<Definition<'tree>>, StageIndices) {
    let mut definitions = Vec::new();
    let mut stages = StageIndices::default();
    let mut stage = None;
    let mut last_instruction_end_row = None;
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        if child.is_error() {
            // A trailing error on the previous instruction's line can't start a `FROM` on its
            // first line, but a multi-line error can still swallow a `FROM` on later lines.
            let skip_first_line = last_instruction_end_row == Some(child.start_position().row);
            stages.record_error(child, skip_first_line, source);
            continue;
        }
        if !child.is_named() || child.is_extra() {
            continue;
        }
        let mut found = if child.kind() == "from_instruction" {
            stages.record_stage(child, source);
            stage = stage_name(child, source);
            stage
                .map(|name| stage_definition(child, name))
                .into_iter()
                .collect()
        } else {
            instruction_definitions(child, stage, source)
        };
        if child.has_error() {
            // Error recovery inside an instruction can swallow a `FROM` on a later line.
            stages.record_error(child, true, source);
        }
        apply_trailing_error(child, &mut found, source);
        definitions.extend(found);
        last_instruction_end_row = Some(child.end_position().row);
    }
    (definitions, stages)
}

/// Marks definitions ending at `instruction` undecodable when a syntax error follows on the same
/// line.
fn apply_trailing_error(instruction: Node<'_>, found: &mut [Definition<'_>], source: &str) {
    let Some(error) = error_after(instruction) else {
        return;
    };
    // The grammar stops at the error, so the last value can miss its rest.
    for definition in found
        .iter_mut()
        .filter(|definition| definition.range.end == instruction.end_byte())
    {
        definition.value = Some(Value::Undecodable(
            code_symbols::position(&error, source),
            anyhow::anyhow!(
                "the instruction has a syntax error here, so its value could be cut short"
            ),
        ));
    }
}

/// Decodes a Dockerfile string literal, or returns `None` for any other node or a string with `$`.
fn decode_string(node: &Node, source: &str) -> anyhow::Result<Option<String>> {
    match node.kind() {
        "double_quoted_string" => {
            if has_expansion(*node) {
                return Ok(None);
            }
            let text = joined_lines(node, source);
            let unquoted = code_symbols::between_quotes(&text, "\"")?;
            let escape = escape_directive(source)?;
            // Inside double quotes, the escape character before any other character is
            // kept, as in a shell: `"a\nb"` is `a\nb`.
            Ok(Some(unescape(unquoted, escape, |next| {
                next == '"' || next == '$' || next == escape
            })))
        }
        "single_quoted_string" => {
            let text = joined_lines(node, source);
            Ok(Some(code_symbols::between_quotes(&text, "'")?.to_string()))
        }
        "unquoted_string" => {
            if has_expansion(*node) {
                return Ok(None);
            }
            let escape = escape_directive(source)?;
            if node_text(node, source).contains(escape) {
                Ok(Some(unescape(&joined_lines(node, source), escape, |_| {
                    true
                })))
            } else {
                Ok(None)
            }
        }
        _ => Ok(None),
    }
}

/// Whether `line`, with leading spaces and tabs removed, starts with a `FROM` keyword.
fn is_from_line(line: &str) -> bool {
    let Some(head) = line.get(..4) else {
        return false;
    };
    head.eq_ignore_ascii_case("from")
        && matches!(
            line.as_bytes().get(4),
            None | Some(b' ' | b'\t' | b'\r' | b'\\' | b'`')
        )
}

/// The `image_spec` child of `from`, a `FROM` instruction, if any.
fn stage_image(from: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = from.walk();
    from.named_children(&mut cursor)
        .find(|child| child.kind() == "image_spec")
}

/// The name of the stage that `from`, a `FROM` instruction, starts. `None` when the stage has no
/// name, or when its name holds a `$`, because only a build can expand it.
fn stage_name<'tree>(from: Node<'tree>, source: &str) -> Option<Node<'tree>> {
    from.child_by_field_name("as")
        .filter(|name| !node_text(name, source).contains('$'))
}

/// The syntax error right after `instruction`, on the line where it ends. `None` when there is
/// none.
fn error_after(instruction: Node<'_>) -> Option<Node<'_>> {
    // The grammar ends an instruction where it can't read on, such as at an empty line inside a
    // continued value, and puts the rest of the instruction in an error node next to it. An
    // error that starts on a later line is a broken instruction of its own, such as `FOO bar`.
    instruction.next_sibling().filter(|next| {
        next.is_error() && next.start_position().row == instruction.end_position().row
    })
}

/// The definition of the stage `name`, which `from` starts. Its value is the image.
fn stage_definition<'tree>(from: Node<'tree>, name: Node<'tree>) -> Definition<'tree> {
    Definition::new(vec![], name, from.byte_range(), stage_image(from))
}

/// The definitions of the names that `instruction` sets, in document order. They are under
/// `stage`, or at the top level when it is `None`. Empty for an instruction that sets no names,
/// such as `RUN`.
fn instruction_definitions<'tree>(
    instruction: Node<'tree>,
    stage: Option<Node<'tree>>,
    source: &str,
) -> Vec<Definition<'tree>> {
    match instruction.kind() {
        // An ARG without a default only makes a name visible in the stage. It gives the name no
        // value of its own.
        "arg_instruction" => {
            pair_definitions(instruction, "arg_pair", "name", "default", stage, source)
                .into_iter()
                .filter(|definition| definition.value.is_some())
                .collect()
        }
        "env_instruction" => {
            pair_definitions(instruction, "env_pair", "name", "value", stage, source)
        }
        "label_instruction" => {
            pair_definitions(instruction, "label_pair", "key", "value", stage, source)
        }
        _ => Vec::new(),
    }
}

/// The definitions of the `pair_kind` children of `instruction`, in document order. Each pair's
/// name is its `name_field`, and its value is its `value_field`, or `None` when it has none. The
/// definitions are under `stage`, or at the top level when it is `None`.
fn pair_definitions<'tree>(
    instruction: Node<'tree>,
    pair_kind: &str,
    name_field: &str,
    value_field: &str,
    stage: Option<Node<'tree>>,
    source: &str,
) -> Vec<Definition<'tree>> {
    let mut cursor = instruction.walk();
    instruction
        .named_children(&mut cursor)
        .filter(|pair| pair.kind() == pair_kind)
        .filter_map(|pair| {
            let name = pair.child_by_field_name(name_field)?;
            let mut cursor = pair.walk();
            let has_equals = pair.children(&mut cursor).any(|child| child.kind() == "=");
            let value = match pair.child_by_field_name(value_field) {
                // Without an `=`, as in `ENV NAME value`, a value starts on the line of its
                // name, even after a line continuation, which is a part of the value. The
                // grammar reads `ENV NAME` without a value, which Docker rejects, as a name
                // whose value is the next line.
                Some(value)
                    if !has_equals && value.start_position().row > name.end_position().row =>
                {
                    Some(Value::Undecodable(
                        code_symbols::position(&value, source),
                        anyhow::anyhow!(
                            "the name has no value on its line, so the grammar read the next line as its value"
                        ),
                    ))
                }
                _ => pair_value(pair, value_field, source),
            };
            Some(Definition {
                parents: stage.into_iter().collect(),
                names: vec![name],
                range: pair.byte_range(),
                value,
            })
        })
        .collect()
}

/// The value of `pair`, written in its `field` children, or `None` when it has none.
fn pair_value<'tree>(pair: Node<'tree>, field: &str, source: &str) -> Option<Value<'tree>> {
    let mut cursor = pair.walk();
    let parts: Vec<Node<'tree>> = pair.children_by_field_name(field, &mut cursor).collect();
    match parts.as_slice() {
        // The grammar gives `A=` no value node.
        [] if node_text(&pair, source).trim_end().ends_with('=') => {
            Some(Value::Read(ScalarValue {
                text: String::new(),
                byte_range: pair.end_byte()..pair.end_byte(),
            }))
        }
        [] => None,
        [part] => Some(Value::Initializer(*part)),
        // Parts written together, such as `"a"b`, make one value, as in a shell.
        [first, .., last] => {
            let byte_range = first.start_byte()..last.end_byte();
            if parts.iter().any(|part| has_expansion(*part)) {
                return Some(Value::Read(ScalarValue {
                    text: parts.iter().map(|part| node_text(part, source)).collect(),
                    byte_range,
                }));
            }
            let mut text = String::new();
            for part in &parts {
                match code_symbols::node_value(*part, source, &decode_string) {
                    Ok(value) => text.push_str(&value.text),
                    Err((node, error)) => {
                        return Some(Value::Undecodable(
                            code_symbols::position(&node, source),
                            error,
                        ));
                    }
                }
            }
            Some(Value::Read(ScalarValue { text, byte_range }))
        }
    }
}

/// Whether `node` or any descendant is an expansion such as `$VAR` or `${VAR}`.
fn has_expansion(node: Node<'_>) -> bool {
    let mut found = false;
    walk_tree(node, |node| {
        found |= node.kind() == "expansion";
        !found
    });
    found
}

/// The escape character for `source`, either `\` or `` ` ``.
///
/// # Errors
/// Returns an error when Docker would read another escape character, because a `# escape=`
/// line has a space before its `#`.
fn escape_directive(source: &str) -> anyhow::Result<char> {
    let escape = escape_in_directives(source.lines());
    // The grammar finds the line continuations with `escape`. Decoding with Docker's escape
    // character instead would mix two sets of rules.
    if escape_in_directives(
        source
            .lines()
            .map(|line| line.trim_start_matches([' ', '\t'])),
    ) != escape
    {
        anyhow::bail!(
            "Docker reads the `# escape=` line at the top even with a space before its `#`, but blockwatch can't. Remove the space"
        );
    }
    Ok(escape)
}

/// The escape character that the directives at the start of `lines` set, or `\` without one.
fn escape_in_directives<'a>(lines: impl Iterator<Item = &'a str>) -> char {
    // Directives sit only in the comment lines at the top of the file. Any other line, or a
    // comment that is not a directive, ends them. These rules are the grammar's. Docker's
    // rules are the same, except that Docker also allows spaces before the `#`.
    let mut escape = None;
    for line in lines {
        let Some(text) = line.strip_prefix('#') else {
            break;
        };
        match directive(text) {
            (key, value @ ("\\" | "`")) if key == "escape" && escape.is_none() => {
                escape = value.chars().next();
            }
            (key, value) if (key == "syntax" || key == "check") && !value.is_empty() => {}
            _ => break,
        }
    }
    escape.unwrap_or('\\')
}

/// The key, in lowercase, and the value of `text`, a comment after its `#`, read as a directive
/// such as `` escape=` ``. The value is the first word after the `=`, or empty without one.
fn directive(text: &str) -> (String, &str) {
    const SPACES: [char; 2] = [' ', '\t'];
    let text = text.trim_start_matches(SPACES);
    let (key, rest) = text.split_at(
        text.find(|c: char| !c.is_ascii_alphabetic())
            .unwrap_or(text.len()),
    );
    let value = rest
        .trim_start_matches(SPACES)
        .strip_prefix('=')
        .and_then(|value| value.trim_start_matches(SPACES).split(SPACES).next())
        .unwrap_or("");
    (key.to_ascii_lowercase(), value)
}

/// The text of `node`, a string, with its lines joined: without each line continuation, and
/// without each comment line inside it.
fn joined_lines(node: &Node, source: &str) -> String {
    // Docker joins the lines of an instruction before it reads the quotes, so this happens
    // inside quotes too. The grammar finds the continuations and the comment lines with the
    // same rules as Docker, such as the spaces it allows after the escape character.
    let mut text = String::new();
    let mut start = node.start_byte();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let end = match child.kind() {
            "line_continuation" => child.start_byte(),
            // The grammar leaves the spaces before the `#` out of the comment, but Docker
            // drops the whole line.
            "comment" => {
                #[expect(
                    clippy::string_slice,
                    reason = "tree-sitter gives character boundaries"
                )]
                let before = source[start..child.start_byte()].trim_end_matches([' ', '\t']);
                start + before.len()
            }
            _ => continue,
        };
        #[expect(
            clippy::string_slice,
            reason = "tree-sitter gives character boundaries"
        )]
        text.push_str(&source[start..end]);
        start = child.end_byte();
    }
    #[expect(
        clippy::string_slice,
        reason = "tree-sitter gives character boundaries"
    )]
    text.push_str(&source[start..node.end_byte()]);
    text
}

/// `text` without each `escape_char` before a character that `is_escaped` accepts. `text` must
/// have its lines joined already, as [`joined_lines`] does.
fn unescape(text: &str, escape_char: char, is_escaped: impl Fn(char) -> bool) -> String {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch == escape_char {
            match chars.next() {
                Some(next_ch) if is_escaped(next_ch) => result.push(next_ch),
                Some(next_ch) => {
                    result.push(ch);
                    result.push(next_ch);
                }
                None => result.push(ch),
            }
        } else {
            result.push(ch);
        }
    }
    result
}

/// Returns a [`LanguageParser`] for Dockerfile (Containerfile).
pub(super) fn parser() -> impl LanguageParser {
    LanguageParserImpl::new(comments_parser()).with_symbols(DockerfileSymbolsParser::new())
}

fn comments_parser() -> impl CommentsParser {
    let containerfile_language = tree_sitter_containerfile::LANGUAGE.into();
    languages::python_style_comments_parser(&containerfile_language, "comment", &["#"])
        .with_break_at_node_kinds(&["double_quoted_string", "single_quoted_string"])
        // Because of the bug in the Dockerfile tree-sitter grammar, the strings like
        // "# not a comment" are lexed as a comment. This is a workaround to ignore them.
        .with_break_when(|node, source_code| {
            node.kind() == "comment"
                && node.start_byte() > 0
                && source_code.as_bytes()[node.start_byte() - 1] == b'"'
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{languages::Comment, position::Position};

    #[test]
    fn parses_comments_correctly() {
        let mut comments_parser = comments_parser();

        // The `#` in the `RUN` instruction belongs to the shell command and must not be
        // extracted as a comment. Note that the grammar includes the trailing newline in
        // each comment node.
        let comments: Vec<Comment> = comments_parser
            .parse(
                r"
# syntax=docker/dockerfile:1
FROM alpine:3.20

# This is a comment
RUN echo hi # not a comment

  # Indented comment
COPY . /app
## A section heading repeats the marker.
",
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
    }

    #[test]
    fn hash_inside_a_double_quoted_argument_is_not_a_comment() {
        let mut comments_parser = comments_parser();

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
    }

    #[test]
    fn hash_inside_a_single_quoted_argument_is_not_a_comment() {
        let mut comments_parser = comments_parser();

        let comments: Vec<String> = comments_parser
            .parse(
                r"FROM alpine:3.20
LABEL summary='# also not a comment'

# The only comment here
COPY . /app
",
            )
            .map(|comment| comment.comment_text)
            .collect();

        assert_eq!(comments, vec!["  The only comment here\n".to_string()]);
    }

    #[test]
    fn hash_opening_a_json_array_argument_is_not_a_comment() {
        let mut comments_parser = comments_parser();

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
    }
}

#[cfg(test)]
mod symbols_tests {
    use super::*;
    use crate::languages::code_symbols::test_utils::{
        decode_error, symbol, symbols, syntax_error, unresolved,
    };
    use crate::symbols::{Symbols, SymbolsParser};

    /// The symbols of `source`, a Dockerfile.
    fn dockerfile(source: &str) -> anyhow::Result<Symbols> {
        DockerfileSymbolsParser::new().parse(source)
    }

    #[test]
    fn single_unnamed_stage_parse_symbols_exposes_stage_zero() -> anyhow::Result<()> {
        for (source, expected_def, expected_image) in [
            (
                "FROM python:3.12-slim\nWORKDIR /app\n",
                "FROM python:3.12-slim",
                "python:3.12-slim",
            ),
            (
                "from python:3.12-slim\n",
                "from python:3.12-slim",
                "python:3.12-slim",
            ),
            (
                "FROM --platform=linux/amd64 python:3.12-slim@sha256:abcdef\n",
                "FROM --platform=linux/amd64 python:3.12-slim@sha256:abcdef",
                "python:3.12-slim@sha256:abcdef",
            ),
        ] {
            assert_eq!(
                symbols(&dockerfile(source)?, source),
                vec![symbol("/0", expected_def, Some(expected_image))],
                "{source:?}"
            );
        }

        Ok(())
    }

    #[test]
    fn mixed_named_and_unnamed_stages_parse_symbols_indexes_every_stage_from_zero()
    -> anyhow::Result<()> {
        let source = r"FROM golang:1.25 AS builder
FROM python:3.12-slim
FROM scratch AS final
";

        assert_eq!(
            symbols(&dockerfile(source)?, source),
            vec![
                symbol("/0", "FROM golang:1.25 AS builder", Some("golang:1.25")),
                symbol(
                    "/builder",
                    "FROM golang:1.25 AS builder",
                    Some("golang:1.25")
                ),
                symbol("/1", "FROM python:3.12-slim", Some("python:3.12-slim")),
                symbol("/2", "FROM scratch AS final", Some("scratch")),
                symbol("/final", "FROM scratch AS final", Some("scratch")),
            ]
        );

        Ok(())
    }

    #[test]
    fn label_named_zero_with_stage_zero_parse_symbols_marks_zero_ambiguous() -> anyhow::Result<()> {
        let source = "FROM alpine:3.20\nLABEL 0=\"zero\"\n";

        assert_eq!(
            unresolved(&dockerfile(source)?, source, "/0")?,
            "ambiguous symbol, defined at 1:1, 2:7"
        );

        Ok(())
    }

    #[test]
    fn stage_named_like_another_stage_index_parse_symbols_marks_it_ambiguous() -> anyhow::Result<()>
    {
        let source = "FROM alpine:3.20 AS 1\nFROM ubuntu:24.04\n";
        assert_eq!(
            unresolved(&dockerfile(source)?, source, "/1")?,
            "ambiguous symbol, defined at 1:1, 2:1"
        );

        Ok(())
    }

    #[test]
    fn duplicate_stage_names_and_multiline_from_parse_symbols_index_each_stage()
    -> anyhow::Result<()> {
        let duplicate = "FROM alpine:3.20 AS builder\nFROM ubuntu:24.04 AS builder\n";
        let parsed = dockerfile(duplicate)?;
        assert_eq!(
            parsed
                .resolve(&SymbolPath::parse("/0")?)
                .map(|found| found.value_or_definition(duplicate)),
            Ok(std::borrow::Cow::Borrowed("alpine:3.20"))
        );
        assert_eq!(
            parsed
                .resolve(&SymbolPath::parse("/1")?)
                .map(|found| found.value_or_definition(duplicate)),
            Ok(std::borrow::Cow::Borrowed("ubuntu:24.04"))
        );
        assert_eq!(
            unresolved(&parsed, duplicate, "/builder")?,
            "ambiguous symbol, defined at 1:1, 2:1"
        );

        let multiline = "FROM \\\n  alpine:3.20 \\\n  AS builder\n";
        assert_eq!(
            symbols(&dockerfile(multiline)?, multiline),
            vec![
                symbol(
                    "/0",
                    "FROM \\\n  alpine:3.20 \\\n  AS builder",
                    Some("alpine:3.20")
                ),
                symbol(
                    "/builder",
                    "FROM \\\n  alpine:3.20 \\\n  AS builder",
                    Some("alpine:3.20")
                ),
            ]
        );

        let onbuild = "ONBUILD FROM alpine:3.20\nFROM ubuntu:24.04\n";
        assert_eq!(
            symbols(&dockerfile(onbuild)?, onbuild),
            vec![symbol("/0", "FROM ubuntu:24.04", Some("ubuntu:24.04"))]
        );

        Ok(())
    }

    #[test]
    fn broken_from_parse_symbols_marks_affected_stage_indices_unreadable() -> anyhow::Result<()> {
        for (source, column) in [
            ("FROM\nFROM ubuntu:24.04 AS app\n", 1),
            ("from\nFROM ubuntu:24.04 AS app\n", 1),
            ("  FROM\nFROM ubuntu:24.04 AS app\n", 3),
        ] {
            let parsed = dockerfile(source)?;
            let expected = syntax_error(1, column, "FROM");
            for path in ["/0", "/1"] {
                assert_eq!(unresolved(&parsed, source, path)?, expected, "{source:?}");
            }
            assert_eq!(
                parsed
                    .resolve(&SymbolPath::parse("/app")?)
                    .map(|found| found.value_or_definition(source)),
                Ok(std::borrow::Cow::Borrowed("ubuntu:24.04")),
                "{source:?}"
            );
        }

        let consecutive = "FROM\nFROM\nFROM ubuntu:24.04\n";
        let parsed = dockerfile(consecutive)?;
        assert_eq!(
            unresolved(&parsed, consecutive, "/0")?,
            syntax_error(1, 1, "FROM")
        );
        for path in ["/1", "/2"] {
            assert_eq!(
                unresolved(&parsed, consecutive, path)?,
                syntax_error(2, 1, "FROM"),
                "{path}"
            );
        }
        assert_eq!(unresolved(&parsed, consecutive, "/3")?, "symbol not found");

        let error_before_broken_from =
            "FROM alpine:3.20\nUNKNOWN echo from here\nFROM ubuntu:24.04\nFROM\nFROM debian:12\n";
        let parsed = dockerfile(error_before_broken_from)?;
        assert_eq!(
            parsed
                .resolve(&SymbolPath::parse("/0")?)
                .map(|found| found.value_or_definition(error_before_broken_from)),
            Ok(std::borrow::Cow::Borrowed("alpine:3.20"))
        );
        assert_eq!(
            parsed
                .resolve(&SymbolPath::parse("/1")?)
                .map(|found| found.value_or_definition(error_before_broken_from)),
            Ok(std::borrow::Cow::Borrowed("ubuntu:24.04"))
        );
        for path in ["/2", "/3"] {
            assert_eq!(
                unresolved(&parsed, error_before_broken_from, path)?,
                syntax_error(4, 1, "FROM"),
                "{path}"
            );
        }

        let trailing_broken = "FROM alpine:3.20\nFROM\n";
        let parsed = dockerfile(trailing_broken)?;
        assert_eq!(
            parsed
                .resolve(&SymbolPath::parse("/0")?)
                .map(|found| found.value_or_definition(trailing_broken)),
            Ok(std::borrow::Cow::Borrowed("alpine:3.20"))
        );
        assert_eq!(
            unresolved(&parsed, trailing_broken, "/1")?,
            syntax_error(2, 1, "FROM")
        );

        let trailing_junk = "FROM alpine:3.20 AS app from bar\nFROM debian:12\n";
        let parsed = dockerfile(trailing_junk)?;
        for path in ["/0", "/app"] {
            assert_eq!(
                unresolved(&parsed, trailing_junk, path)?,
                decode_error(
                    1,
                    24,
                    "the instruction has a syntax error here, so its value could be cut short"
                ),
                "{path}"
            );
        }
        assert_eq!(
            parsed
                .resolve(&SymbolPath::parse("/1")?)
                .map(|found| found.value_or_definition(trailing_junk)),
            Ok(std::borrow::Cow::Borrowed("debian:12"))
        );

        let after_comment = "# comment\nFROM\nFROM ubuntu:24.04\n";
        let parsed = dockerfile(after_comment)?;
        for path in ["/0", "/1"] {
            assert_eq!(
                unresolved(&parsed, after_comment, path)?,
                syntax_error(2, 1, "FROM"),
                "{path}"
            );
        }

        Ok(())
    }

    #[test]
    fn multiline_error_swallowing_from_parse_symbols_marks_later_stages_unreadable()
    -> anyhow::Result<()> {
        for (source, line, column) in [
            (
                "FROM alpine\nCMD echo \\\n\"\nFROM ubuntu\nFROM debian\n",
                2,
                1,
            ),
            (
                "FROM alpine\nHEALTHCHECK CMD x \\\n\"\nFROM ubuntu\nFROM debian\n",
                2,
                1,
            ),
            ("FROM alpine\nCMD echo \\\n\"\nFROM\nFROM ubuntu\n", 2, 1),
            (
                "FROM alpine\nENV CUT=\"a\\\n\nFROM ubuntu\nb\"\nFROM debian\n",
                2,
                9,
            ),
        ] {
            let parsed = dockerfile(source)?;
            assert_eq!(
                parsed
                    .resolve(&SymbolPath::parse("/0")?)
                    .map(|found| found.value_or_definition(source)),
                Ok(std::borrow::Cow::Borrowed("alpine")),
                "{source:?}"
            );
            for path in ["/1", "/2"] {
                assert_eq!(
                    unresolved(&parsed, source, path)?,
                    syntax_error(line, column, "FROM"),
                    "{path} in {source:?}"
                );
            }
        }

        Ok(())
    }

    #[test]
    fn unrelated_error_mentioning_from_parse_symbols_keeps_later_stages() -> anyhow::Result<()> {
        for source in [
            "FROM alpine:3.20\nCOPY --from=0 /only_one_arg\nFROM ubuntu:24.04\n",
            "FROM alpine:3.20\nENV MSG=\"hello from\\\n\nworld\"\nFROM ubuntu:24.04\n",
            "FROM alpine:3.20\nENV CUT=\"a\\\n\n# FROM fake\nb\"\nFROM ubuntu:24.04\n",
            "FROM alpine:3.20\nFROM_IMAGE=1\nFROM ubuntu:24.04\n",
            "FROM alpine:3.20\nUNKNOWN FROM_IMAGE\nFROM ubuntu:24.04\n",
        ] {
            let parsed = dockerfile(source)?;
            assert_eq!(
                parsed
                    .resolve(&SymbolPath::parse("/0")?)
                    .map(|found| found.value_or_definition(source)),
                Ok(std::borrow::Cow::Borrowed("alpine:3.20")),
                "{source:?}"
            );
            assert_eq!(
                parsed
                    .resolve(&SymbolPath::parse("/1")?)
                    .map(|found| found.value_or_definition(source)),
                Ok(std::borrow::Cow::Borrowed("ubuntu:24.04")),
                "{source:?}"
            );
        }

        Ok(())
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

        // <block affects="docs/symbols.md:dockerfile-paths">
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
                    "/0",
                    "FROM golang:${GO_VERSION}-alpine AS builder",
                    Some("golang:${GO_VERSION}-alpine")
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
                    "/1",
                    "FROM ghcr.io/astral-sh/uv:0.13.5 AS uv",
                    Some("ghcr.io/astral-sh/uv:0.13.5")
                ),
                symbol(
                    "/uv",
                    "FROM ghcr.io/astral-sh/uv:0.13.5 AS uv",
                    Some("ghcr.io/astral-sh/uv:0.13.5")
                ),
                symbol("/2", "FROM ${BASE}", Some("${BASE}")),
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
        let source = r"FROM alpine:3.20
ARG PORT=8080
ENV HOST=localhost
LABEL maintainer=team
";

        assert_eq!(
            symbols(&dockerfile(source)?, source),
            vec![
                symbol("/0", "FROM alpine:3.20", Some("alpine:3.20")),
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
        let source = r"FROM alpine:3.20
ENV PORT=8080

FROM alpine:3.20
ENV PORT=9090
";

        assert_eq!(
            unresolved(&dockerfile(source)?, source, "/PORT")?,
            "ambiguous symbol, defined at 2:5, 5:5"
        );

        Ok(())
    }

    #[test]
    fn name_before_first_from_and_in_unnamed_stage_parse_symbols_marks_it_unresolved()
    -> anyhow::Result<()> {
        let source = r"ARG PORT=8080

FROM alpine:3.20
ARG PORT=9090
";

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
        let source = r"FROM alpine:3.20 AS app
ARG VERSION=1.0
ENV VERSION=1.0
";

        assert_eq!(
            unresolved(&dockerfile(source)?, source, "/app/VERSION")?,
            "ambiguous symbol, defined at 2:5, 3:5"
        );

        Ok(())
    }

    #[test]
    fn both_env_forms_parse_symbols_returns_both() -> anyhow::Result<()> {
        let source = r"FROM alpine:3.20
ENV KEY_EQUALS=val1
ENV KEY_SPACE val2
";

        assert_eq!(
            symbols(&dockerfile(source)?, source),
            vec![
                symbol("/0", "FROM alpine:3.20", Some("alpine:3.20")),
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
            vec![
                symbol("/0", "FROM alpine:3.20", Some("alpine:3.20")),
                symbol(
                    "/com.example.vendor",
                    r#""com.example.vendor"="ACME""#,
                    Some("ACME")
                ),
            ]
        );

        Ok(())
    }

    #[test]
    fn from_with_platform_tag_and_digest_parse_symbols_returns_image_spec_without_platform()
    -> anyhow::Result<()> {
        let source = "FROM --platform=linux/amd64 golang:1.23@sha256:abcdef AS build\n";

        assert_eq!(
            symbols(&dockerfile(source)?, source),
            vec![
                symbol(
                    "/0",
                    "FROM --platform=linux/amd64 golang:1.23@sha256:abcdef AS build",
                    Some("golang:1.23@sha256:abcdef")
                ),
                symbol(
                    "/build",
                    "FROM --platform=linux/amd64 golang:1.23@sha256:abcdef AS build",
                    Some("golang:1.23@sha256:abcdef")
                ),
            ]
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
            vec![
                symbol("/0", "FROM alpine:3.20", Some("alpine:3.20")),
                symbol("/PORT", "PORT=8080", Some("8080")),
            ]
        );

        Ok(())
    }

    #[test]
    fn value_cut_short_by_syntax_error_parse_symbols_marks_it_unresolved() -> anyhow::Result<()> {
        let source = r#"ENV BEFORE=1 CUT="a\

b"
ARG ESCAPED="a\\
b"
"#;
        let parsed = dockerfile(source)?;

        assert_eq!(
            unresolved(&parsed, source, "/CUT")?,
            "blockwatch can't decode the string at line 1, column 18: the instruction has a \
             syntax error here, so its value could be cut short"
        );
        assert_eq!(
            unresolved(&parsed, source, "/ESCAPED")?,
            "blockwatch can't decode the string at line 4, column 13: the instruction has a \
             syntax error here, so its value could be cut short"
        );
        assert_eq!(
            symbols(&parsed, source)
                .into_iter()
                .filter(|(path, _, _)| path == "/BEFORE")
                .collect::<Vec<_>>(),
            vec![symbol("/BEFORE", "BEFORE=1", Some("1"))]
        );

        Ok(())
    }

    #[test]
    fn name_without_value_on_its_line_parse_symbols_marks_it_unresolved() -> anyhow::Result<()> {
        for (source, line) in [
            ("ENV NAME\n", 2),
            ("ENV NAME\nENV OTHER=1\n", 2),
            ("ENV NAME\n\nENV OTHER=1\n", 3),
            ("LABEL NAME\nENV OTHER=1\n", 2),
        ] {
            assert_eq!(
                unresolved(&dockerfile(source)?, source, "/NAME")?,
                format!(
                    "blockwatch can't decode the string at line {line}, column 1: the name has no \
                     value on its line, so the grammar read the next line as its value"
                ),
                "{source:?}"
            );
        }

        Ok(())
    }

    #[test]
    fn value_after_line_continuation_parse_symbols_returns_the_value() -> anyhow::Result<()> {
        let source = "ENV SPACE \\\nvalue\nENV EQUALS=\\\nvalue\n";

        assert_eq!(
            symbols(&dockerfile(source)?, source),
            vec![
                symbol("/SPACE", "SPACE \\\nvalue", Some("value")),
                symbol("/EQUALS", "EQUALS=\\\nvalue", Some("value")),
            ]
        );

        Ok(())
    }

    #[test]
    fn syntax_error_on_the_next_line_parse_symbols_keeps_the_value() -> anyhow::Result<()> {
        let source = r"ENV NEXT=1
UNKNOWN instruction
";

        assert_eq!(
            symbols(&dockerfile(source)?, source),
            vec![symbol("/NEXT", "NEXT=1", Some("1"))]
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
    fn double_quoted_string_parse_symbols_drops_escape_only_before_quote_dollar_and_itself()
    -> anyhow::Result<()> {
        let source = r#"ENV ESCAPED="hello\"world"
ENV BACKSLASH="hello\\world"
ENV DOLLAR="cost\$5"
ENV KEPT="a\nb"
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
                    Some(r"hello\world")
                ),
                symbol("/DOLLAR", r#"DOLLAR="cost\$5""#, Some("cost$5")),
                symbol("/KEPT", r#"KEPT="a\nb""#, Some(r"a\nb")),
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
    fn single_quoted_string_parse_symbols_reads_no_escapes() -> anyhow::Result<()> {
        let source = r"ENV SINGLE='hello\world'
ENV CONTINUATION='line1\
line2'
";

        assert_eq!(
            symbols(&dockerfile(source)?, source),
            vec![
                symbol("/SINGLE", r"SINGLE='hello\world'", Some(r"hello\world")),
                symbol(
                    "/CONTINUATION",
                    "CONTINUATION='line1\\\nline2'",
                    Some("line1line2")
                ),
            ]
        );

        Ok(())
    }

    #[test]
    fn file_with_escape_directive_parse_symbols_decodes_with_backtick() -> anyhow::Result<()> {
        let source = r#"# escape=`
ENV WIN_PATH=C:\Windows\Temp
ENV ESCAPED="hello`"world"
ENV BACKTICK="a``b"
ENV KEPT="a`nb\c"
ENV CONTINUATION="line1`
line2"
"#;

        assert_eq!(
            symbols(&dockerfile(source)?, source),
            vec![
                symbol(
                    "/WIN_PATH",
                    r"WIN_PATH=C:\Windows\Temp",
                    Some(r"C:\Windows\Temp")
                ),
                symbol(
                    "/ESCAPED",
                    r#"ESCAPED="hello`"world""#,
                    Some(r#"hello"world"#)
                ),
                symbol("/BACKTICK", r#"BACKTICK="a``b""#, Some("a`b")),
                symbol("/KEPT", r#"KEPT="a`nb\c""#, Some(r"a`nb\c")),
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
    fn continuation_with_spaces_after_the_escape_parse_symbols_joins_the_lines()
    -> anyhow::Result<()> {
        // The spaces and tabs after each escape character are written as escapes, so that an
        // editor that trims them can't change the test.
        let source = "ENV DOUBLE=\"a\\  \nb\" TAB=\"a\\\t\nb\"\n\
                      ENV UNQUOTED=a\\ \nb SINGLE='a\\ \nb' CRLF=\"a\\ \r\nb\"\n";

        assert_eq!(
            symbols(&dockerfile(source)?, source),
            vec![
                symbol("/DOUBLE", "DOUBLE=\"a\\  \nb\"", Some("ab")),
                symbol("/TAB", "TAB=\"a\\\t\nb\"", Some("ab")),
                symbol("/UNQUOTED", "UNQUOTED=a\\ \nb", Some("ab")),
                symbol("/SINGLE", "SINGLE='a\\ \nb'", Some("ab")),
                symbol("/CRLF", "CRLF=\"a\\ \r\nb\"", Some("ab")),
            ]
        );

        Ok(())
    }

    #[test]
    fn comment_line_inside_a_value_parse_symbols_leaves_the_line_out() -> anyhow::Result<()> {
        let source = r#"ENV DOUBLE="a\
# comment
b"
ENV INDENTED="a\
    # comment
	# comment
b"
ENV UNQUOTED=a\
# comment
b
ENV SPACES="a\
  b"
"#;

        assert_eq!(
            symbols(&dockerfile(source)?, source),
            vec![
                symbol("/DOUBLE", "DOUBLE=\"a\\\n# comment\nb\"", Some("ab")),
                symbol(
                    "/INDENTED",
                    "INDENTED=\"a\\\n    # comment\n\t# comment\nb\"",
                    Some("ab")
                ),
                symbol("/UNQUOTED", "UNQUOTED=a\\\n# comment\nb", Some("ab")),
                symbol("/SPACES", "SPACES=\"a\\\n  b\"", Some("a  b")),
            ]
        );

        Ok(())
    }

    #[test]
    fn value_of_parts_written_together_parse_symbols_joins_the_decoded_parts() -> anyhow::Result<()>
    {
        let source = r#"ARG FIRST="a"b
ENV SECOND=a"b"c THIRD='a'"b"
LABEL fourth="a"'b'
"#;

        assert_eq!(
            symbols(&dockerfile(source)?, source),
            vec![
                symbol("/FIRST", r#"FIRST="a"b"#, Some("ab")),
                symbol("/SECOND", r#"SECOND=a"b"c"#, Some("abc")),
                symbol("/THIRD", r#"THIRD='a'"b""#, Some("ab")),
                symbol("/fourth", r#"fourth="a"'b'"#, Some("ab")),
            ]
        );

        Ok(())
    }

    #[test]
    fn value_with_expansion_in_one_part_parse_symbols_keeps_the_value_as_written()
    -> anyhow::Result<()> {
        let source = r#"ENV PATH_VALUE="$HOME""/bin"
"#;

        assert_eq!(
            symbols(&dockerfile(source)?, source),
            vec![symbol(
                "/PATH_VALUE",
                r#"PATH_VALUE="$HOME""/bin""#,
                Some(r#""$HOME""/bin""#)
            )]
        );

        Ok(())
    }

    #[test]
    fn string_with_expansion_parse_symbols_keeps_value_as_written() -> anyhow::Result<()> {
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
    fn single_quoted_string_with_dollar_parse_symbols_keeps_dollar() -> anyhow::Result<()> {
        let source = r"ENV SINGLE='$FOO'
ENV SINGLE_BRACED='${FOO}'
";

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
    fn stage_with_dollar_in_name_parse_symbols_puts_its_names_at_top_level() -> anyhow::Result<()> {
        let source = "FROM alpine:3.20 AS $stage\nARG PORT=8080\n";

        assert_eq!(
            symbols(&dockerfile(source)?, source),
            vec![
                symbol("/0", "FROM alpine:3.20 AS $stage", Some("alpine:3.20")),
                symbol("/PORT", "PORT=8080", Some("8080")),
            ]
        );

        Ok(())
    }

    #[test]
    fn unquoted_string_with_escape_parse_symbols_drops_escape_character() -> anyhow::Result<()> {
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

    /// The value of `ENV V=x\y` after `directives`, the comment lines at the top of the file:
    /// `xy` when the escape character is `\`, and `x\y` when it is a backtick.
    fn value_after(directives: &str) -> anyhow::Result<Option<String>> {
        let source = format!("{directives}ENV V=x\\y\n");
        let found = symbols(&dockerfile(&source)?, &source);
        Ok(found
            .into_iter()
            .find(|(path, _, _)| path == "/V")
            .and_then(|(_, _, value)| value))
    }

    #[test]
    fn escape_directive_at_top_parse_symbols_reads_backtick_as_escape() -> anyhow::Result<()> {
        for directives in [
            "# escape=`\n",
            "#escape=`\n",
            "#  escape =  ` \n",
            "# Escape=`\n",
            "# syntax=docker/dockerfile:1\n# escape=`\n",
            "# check=skip=all\n# escape=`\n",
            "# syntax=docker/dockerfile:1\r\n# escape=`\r\n",
            // Docker rejects a second escape directive. The grammar keeps the first one.
            "# escape=`\n# escape=\\\n",
        ] {
            assert_eq!(
                value_after(directives)?.as_deref(),
                Some("x\\y"),
                "{directives:?}"
            );
        }

        Ok(())
    }

    #[test]
    fn escape_directive_after_end_of_directives_parse_symbols_keeps_backslash() -> anyhow::Result<()>
    {
        for directives in [
            "\n# escape=`\n",
            "# syntax=docker/dockerfile:1\n\n# escape=`\n",
            "# comment\n# escape=`\n",
            "# syntax is great\n# escape=`\n",
            "# unknown=value\n# escape=`\n",
            "# syntax=\n# escape=`\n",
            "# escape2=`\n",
            " # escape=\\\n",
            "# comment\n # escape=`\n",
        ] {
            assert_eq!(
                value_after(directives)?.as_deref(),
                Some("xy"),
                "{directives:?}"
            );
        }

        Ok(())
    }

    #[test]
    fn indented_escape_directive_parse_symbols_marks_values_unresolved() -> anyhow::Result<()> {
        for (directives, value) in [
            (" # escape=`\n", "x\\y"),
            ("\t# escape=`\n", "x\\y"),
            ("# syntax=docker/dockerfile:1\n # escape=`\n", "x\\y"),
            (" # escape=`\n", "\"x\""),
        ] {
            let source = format!("{directives}ENV V={value}\n");
            assert_eq!(
                unresolved(&dockerfile(&source)?, &source, "/V")?,
                format!(
                    "blockwatch can't decode the string at line {}, column 7: Docker reads the \
                     `# escape=` line at the top even with a space before its `#`, but \
                     blockwatch can't. Remove the space",
                    directives.lines().count() + 1
                ),
                "{directives:?}"
            );
        }

        Ok(())
    }
}
