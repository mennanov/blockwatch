//! Finds the comments and the symbols in Makefiles with `makefile-lossless`.
//!
//! A tree-sitter grammar reads many valid Makefiles wrongly. `tree-sitter-make` 1.1.1 has a
//! syntax error on a `(` in a value or a comment, on `:::=` and on `export X != cmd`. It reads
//! `export define` as a list of names, and every line that starts with a tab as a recipe line.

use crate::LineIndex;
use crate::language_parsers::{Comment, CommentsParser, LanguageParser, LanguageParserImpl};
use crate::symbol_path::SymbolPath;
use crate::symbols::{ScalarValue, Symbol, Symbols, SymbolsParser};
use anyhow::bail;
use makefile_lossless::{
    AssignmentOperator, Makefile, MakefileVariant, ParseErrorKind, Rule, TextRange,
    VariableDefinition,
};
use std::ops::Range;

/// Returns a [`LanguageParser`] for Makefile.
pub(super) fn parser() -> anyhow::Result<impl LanguageParser> {
    Ok(LanguageParserImpl::new(MakefileCommentsParser).with_symbols(MakefileSymbolsParser))
}

/// Finds the comments in a Makefile, where makefile-lossless finds them.
struct MakefileCommentsParser;

impl CommentsParser for MakefileCommentsParser {
    fn parse<'source>(
        &'source mut self,
        source: &'source str,
    ) -> impl Iterator<Item = Comment> + 'source {
        let lines = LineIndex::new(source);
        let tree = Makefile::parse_with_variant(source, MakefileVariant::GNUMake).tree();
        // A comment that ends in `\` before a blank line takes in the line end, so its range
        // would end on the next line.
        let ranges: Vec<Range<usize>> = tree
            .comment_ranges()
            .map(|range| trim_end(source, range))
            .collect();
        ranges.into_iter().map(move |range| {
            #[expect(clippy::string_slice, reason = "makefile-lossless cuts at tokens")]
            let text = &source[range.clone()];
            Comment {
                position_range: lines.range(&range),
                comment_text: text.replacen('#', " ", 1),
                source_range: range,
            }
        })
    }
}

/// Finds the symbols of a Makefile: its variables and targets, at the top or in any branch of a
/// conditional.
struct MakefileSymbolsParser;

impl SymbolsParser for MakefileSymbolsParser {
    /// Returns the symbols in document order. The targets of a rule come in the order the rule
    /// lists them.
    ///
    /// # Errors
    /// Returns an error when make would stop reading the file, such as at a `define` without
    /// `endef`, or when makefile-lossless skips a part of it, such as conditionals nested too
    /// deeply. The error shows the line.
    fn parse(&mut self, source: &str) -> anyhow::Result<Symbols> {
        let parsed = Makefile::parse_with_variant(source, MakefileVariant::GNUMake);
        if let Some(error) = parsed.errors().iter().find(|error| stops_run(error.kind())) {
            bail!("line {}: {}", error.line, error.message);
        }
        let tree = parsed.tree();
        let mut symbols: Vec<Symbol> = tree
            .variable_definitions()
            .filter_map(|variable| variable_symbol(source, &variable))
            .collect();
        symbols.extend(tree.rules().flat_map(|rule| rule_symbols(source, &rule)));
        // The sort is stable, so the targets of a rule keep their order.
        symbols.sort_by_key(|symbol| symbol.def_byte_ranges.first().map(|range| range.start));
        Ok(Symbols::from(symbols))
    }
}

/// Whether a parse error of `kind` stops the run: make stops reading the file there, or
/// makefile-lossless skips a part of it. Makefile-lossless also reports errors on lines that make
/// reads, or never reads, such as a tab line in a branch make skips. Those don't stop the run.
fn stops_run(kind: ParseErrorKind) -> bool {
    match kind {
        ParseErrorKind::MissingEndef
        | ParseErrorKind::MissingEndif
        | ParseErrorKind::ExtraneousEndif
        | ParseErrorKind::ElseWithoutIf
        | ParseErrorKind::DuplicateElse
        | ParseErrorKind::InvalidConditional => true,
        // Make reads any depth, but makefile-lossless skips a conditional nested deeper than it
        // reads, so a name set inside would be missing without a word. A `$(...)` nested that
        // deep loses nothing, but it gives the same kind, so it stops the run too.
        ParseErrorKind::TooDeeplyNested => true,
        // The enum is non-exhaustive, and a kind added later can't be one of the seven above.
        _ => false,
    }
}

/// The symbol of `variable`, a variable definition in `source`, or `None` when it sets no
/// variable for the whole file, as with `+=`, `undefine`, `export NAME`, `build: NAME = x` or a
/// computed name such as `$(PKG)_DIR`.
fn variable_symbol(source: &str, variable: &VariableDefinition) -> Option<Symbol> {
    if variable.assignment_operator_kind() == Some(AssignmentOperator::Append)
        || variable.is_target_specific()
    {
        return None;
    }
    let name = variable.name().filter(|name| is_name(name))?;
    // `undefine NAME` and `export NAME` have no value.
    let value = variable.value_for(MakefileVariant::GNUMake)?;
    Some(Symbol {
        path: SymbolPath::from_segments(vec![name]),
        def_byte_ranges: vec![trim_end(source, variable.text_range())],
        value: Some(ScalarValue {
            // Make keeps the white space at the end, but no one could see it fail a check.
            text: value.trim_end().to_string(),
            byte_range: trim_end(source, variable.value_range()?),
        }),
    })
}

/// A symbol for each target of `rule`, a rule in `source`, that a path can refer to, in the
/// order the rule lists them. Each one covers the whole rule.
fn rule_symbols(source: &str, rule: &Rule) -> Vec<Symbol> {
    // A target-specific variable, such as `build: NAME = x`, adds no recipe to `build`.
    if rule.scoped_assignment().is_some() {
        return Vec::new();
    }
    let has_prerequisites =
        rule.prerequisites().next().is_some() || rule.order_only_prerequisites().next().is_some();
    let range = trim_end(source, rule.text_range());
    rule.targets()
        .filter(|target| is_name(target) && has_name(target, has_prerequisites))
        .map(|target| Symbol {
            path: SymbolPath::from_segments(vec![target]),
            def_byte_ranges: vec![range.clone()],
            value: None,
        })
        .collect()
}

/// Whether `name` is a name that a path can refer to. A name with white space is an error for
/// make, and a name with `$` is computed.
fn is_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains(|character: char| character.is_whitespace() || character == '$')
}

/// Whether `target`, a target as the file writes it, is a name a path can refer to. A pattern,
/// such as `%.o`, a suffix rule, such as `.c.o`, and a special target, such as `.PHONY`, are
/// not. A suffix rule has no prerequisites, so with `has_prerequisites`, `.c.o` is a file.
fn has_name(target: &str, has_prerequisites: bool) -> bool {
    // GNU make writes each special target this way, so one that a later version adds is covered.
    // `.` alone is the directory.
    let is_special = target.strip_prefix('.').is_some_and(|rest| {
        !rest.is_empty()
            && rest
                .chars()
                .all(|character| character.is_ascii_uppercase() || character == '_')
    });
    // A suffix rule is one suffix, or two written together. A Makefile can add suffixes with
    // `.SUFFIXES`, but most use only the ones make knows.
    let is_suffix_rule = !has_prerequisites
        && SUFFIXES.iter().any(|first| {
            target
                .strip_prefix(first)
                .is_some_and(|rest| rest.is_empty() || SUFFIXES.contains(&rest))
        });
    !is_special && !is_suffix_rule && !target.contains('%')
}

/// The suffixes that GNU make 4.4.1 knows before a Makefile adds any, as `make -p` prints them.
const SUFFIXES: &[&str] = &[
    ".out", ".a", ".ln", ".o", ".c", ".cc", ".C", ".cpp", ".p", ".f", ".F", ".m", ".r", ".y", ".l",
    ".ym", ".yl", ".s", ".S", ".mod", ".sym", ".def", ".h", ".info", ".dvi", ".tex", ".texinfo",
    ".texi", ".txinfo", ".w", ".ch", ".web", ".sh", ".elc", ".el",
];

/// `range`, a range that makefile-lossless gives, of `source` without the white space at its end,
/// such as the line end that a node of a statement takes in.
fn trim_end(source: &str, range: TextRange) -> Range<usize> {
    let range = usize::from(range.start())..usize::from(range.end());
    #[expect(clippy::string_slice, reason = "makefile-lossless cuts at tokens")]
    let text = &source[range.clone()];
    range.start..range.start + text.trim_end().len()
}

#[cfg(test)]
mod symbols_tests {
    use super::*;
    use crate::symbols::code::test_utils::{symbol, symbols, text_of};

    /// The symbols of `source`, a Makefile.
    fn makefile(source: &str) -> anyhow::Result<Symbols> {
        MakefileSymbolsParser.parse(source)
    }

    #[test]
    fn file_with_every_shape_parse_symbols_returns_every_symbol_in_order() -> anyhow::Result<()> {
        let source = "# Doc.
GO_VERSION := 1.22.5
VERSION ?= $(shell git describe --tags)
export IMAGE = ghcr.io/acme/app:$(VERSION)
override OPT = -O2
private SECRET = x
LINT := v2.10.1   # keep in sync with CI
TARGETS = linux \\
          darwin

STRICT ::= 1
DATE != date +%F
VPATH = src:include
EMPTY =
CFLAGS += -O2
export NAME_ONLY
unexport UNSET
$(PKG)_VERSION = 1
build: TARGET_ONLY = x

ifeq ($(shell uname),Darwin)
SED = gsed
else ifdef BSD
SED = bsdsed
else
SED = sed
endif

ifdef OUTER
ifndef INNER
NESTED = 1
endif
endif

define BANNER
Build $(VERSION)
endef

define WITH_OPERATOR :=
x
endef

override define OVERRIDDEN
y
endef

define APPENDED +=
z
endef

build: deps
\tgo build ./...

bin/app: $(SRC)
\tgo build -o $@ .
a b: c
\techo $@
%.o: %.c
\t$(CC) -c $<
$(BIN): x
.PHONY: build
.DELETE_ON_ERROR:
.venv: requirements.txt
\tpython3 -m venv .venv
.c.o:
\t$(CC) -c $<
.c:
\t$(CC) -o $@ $<
.sh: install.txt
\techo building .sh
.info: | docs
\tmakeinfo manual.texi
.coverage: tests
\tcoverage run -m pytest
inline: ; echo hi
.RECIPEPREFIX = >
";

        // <block affects="docs/symbols.md:makefile-paths, src/skill.md#/blockwatch/Symbols/Makefile">
        assert_eq!(
            symbols(&makefile(source)?, source),
            vec![
                symbol("/GO_VERSION", "GO_VERSION := 1.22.5", Some("1.22.5")),
                symbol(
                    "/VERSION",
                    "VERSION ?= $(shell git describe --tags)",
                    Some("$(shell git describe --tags)")
                ),
                symbol(
                    "/IMAGE",
                    "export IMAGE = ghcr.io/acme/app:$(VERSION)",
                    Some("ghcr.io/acme/app:$(VERSION)")
                ),
                symbol("/OPT", "override OPT = -O2", Some("-O2")),
                symbol("/SECRET", "private SECRET = x", Some("x")),
                symbol(
                    "/LINT",
                    "LINT := v2.10.1   # keep in sync with CI",
                    Some("v2.10.1")
                ),
                // A range leaves out the blank line after the statement.
                symbol(
                    "/TARGETS",
                    "TARGETS = linux \\\n          darwin",
                    Some("linux darwin")
                ),
                symbol("/STRICT", "STRICT ::= 1", Some("1")),
                symbol("/DATE", "DATE != date +%F", Some("date +%F")),
                symbol("/VPATH", "VPATH = src:include", Some("src:include")),
                symbol("/EMPTY", "EMPTY =", Some("")),
                symbol("/SED", "SED = gsed", Some("gsed")),
                symbol("/SED", "SED = bsdsed", Some("bsdsed")),
                symbol("/SED", "SED = sed", Some("sed")),
                symbol("/NESTED", "NESTED = 1", Some("1")),
                symbol(
                    "/BANNER",
                    "define BANNER\nBuild $(VERSION)\nendef",
                    Some("Build $(VERSION)")
                ),
                symbol(
                    "/WITH_OPERATOR",
                    "define WITH_OPERATOR :=\nx\nendef",
                    Some("x")
                ),
                symbol(
                    "/OVERRIDDEN",
                    "override define OVERRIDDEN\ny\nendef",
                    Some("y")
                ),
                symbol("/build", "build: deps\n\tgo build ./...", None),
                symbol("/bin~1app", "bin/app: $(SRC)\n\tgo build -o $@ .", None),
                // Each target of a rule covers the whole rule.
                symbol("/a", "a b: c\n\techo $@", None),
                symbol("/b", "a b: c\n\techo $@", None),
                symbol(
                    "/.venv",
                    ".venv: requirements.txt\n\tpython3 -m venv .venv",
                    None
                ),
                // `.c.o` and `.c` are suffix rules, the old form of `%.o: %.c` and `%: %.c`.
                // A suffix rule has no prerequisites, so make reads `.sh` here as a file.
                symbol("/.sh", ".sh: install.txt\n\techo building .sh", None),
                symbol("/.info", ".info: | docs\n\tmakeinfo manual.texi", None),
                // `.coverage` starts with `.c`, but `overage` is not a suffix that make knows.
                symbol(
                    "/.coverage",
                    ".coverage: tests\n\tcoverage run -m pytest",
                    None
                ),
                symbol("/inline", "inline: ; echo hi", None),
                symbol("/.RECIPEPREFIX", ".RECIPEPREFIX = >", Some(">")),
            ]
        );
        // </block>
        Ok(())
    }

    #[test]
    fn error_make_stops_on_parse_symbols_fails_with_its_line() {
        // Make stops on each of these files, so no value in them is certain.
        for (source, error) in [
            (
                "A = 1\ndefine B\nb\n",
                "line 2: missing `endef` for `define`",
            ),
            // Make also reports a missing `endif` at the end of the file.
            (
                "ifdef A\nB = 1\n",
                "line 3: unterminated conditional (missing endif)",
            ),
            ("endif\n", "line 1: extraneous `endif`"),
            ("A = 1\nelse\n", "line 2: extraneous `else`"),
            (
                "ifdef A\nelse\nelse\nendif\n",
                "line 3: only one `else` per conditional",
            ),
            (
                "ifeq\nendif\n",
                "line 1: expected opening parenthesis or quote",
            ),
        ] {
            let result = makefile(source);
            assert_eq!(
                result.err().map(|error| format!("{error:#}")).as_deref(),
                Some(error),
                "{source:?}"
            );
        }
    }

    #[test]
    fn error_make_reads_through_parse_symbols_returns_the_symbols() -> anyhow::Result<()> {
        // Make skips the lines of a branch it doesn't read, so it reads this file. Makefile-lossless
        // reports an error on each line of the branch.
        let source = "ifdef NEVER
\t$(error unsupported)
$(error unsupported))
endif
A = 1
";

        assert_eq!(
            symbols(&makefile(source)?, source),
            vec![symbol("/A", "A = 1", Some("1"))]
        );
        Ok(())
    }

    // <block affects="docs/symbols.md:makefile-nesting">
    #[test]
    fn nesting_past_the_crate_limit_parse_symbols_fails_with_its_line() {
        // Make reads this file. Makefile-lossless skips the conditional that is too deep, so a
        // name set in it would be missing.
        let source = format!(
            "A = 1\n{}DEEP = 2\n{}",
            "ifndef X\n".repeat(129),
            "endif\n".repeat(129)
        );

        assert_eq!(
            makefile(&source)
                .err()
                .map(|error| format!("{error:#}"))
                .as_deref(),
            Some("line 130: conditionals and loops nested too deeply")
        );
    }

    #[test]
    fn nesting_at_the_crate_limit_parse_symbols_returns_the_symbols() -> anyhow::Result<()> {
        let source = format!(
            "{}DEEP = 2\n{}",
            "ifndef X\n".repeat(128),
            "endif\n".repeat(128)
        );

        assert_eq!(
            symbols(&makefile(&source)?, &source),
            vec![symbol("/DEEP", "DEEP = 2", Some("2"))]
        );
        Ok(())
    }
    // </block>

    #[test]
    fn value_kinds_parse_symbols_returns_each_value_as_make_reads_it() -> anyhow::Result<()> {
        // Each line ending in `\x20` ends in a space, which an editor could strip.
        let source = "LITERAL = v1.2
COMMENT := v2.10.1   # keep in sync
TRAILING = x  \x20
ONLY_COMMENT = # nothing
ESCAPED_HASH = y\\#z
CONTINUED = linux \\
          darwin
UNICODE = héllo # c
define BODY
one
two \x20
endef
define EMPTY
endef
";

        // Each value with the text of the source where it is written. The two differ only when
        // make reads the value differently from how it is written.
        let values: Vec<(String, String, &str)> = makefile(source)?
            .symbols
            .into_iter()
            .map(|symbol| {
                let value = symbol.value.expect("a variable has a value");
                (
                    symbol.path.to_string(),
                    value.text,
                    &source[value.byte_range],
                )
            })
            .collect();
        let value = |path: &str, text: &str, written: &'static str| {
            (path.to_string(), text.to_string(), written)
        };
        assert_eq!(
            values,
            vec![
                value("/LITERAL", "v1.2", "v1.2"),
                value("/COMMENT", "v2.10.1", "v2.10.1"),
                value("/TRAILING", "x", "x"),
                value("/ONLY_COMMENT", "", ""),
                value("/ESCAPED_HASH", "y#z", r"y\#z"),
                value("/CONTINUED", "linux darwin", "linux \\\n          darwin"),
                value("/UNICODE", "héllo", "héllo"),
                value("/BODY", "one\ntwo", "one\ntwo"),
                value("/EMPTY", "", ""),
            ]
        );
        Ok(())
    }

    #[test]
    fn name_defined_twice_parse_symbols_returns_each_definition() -> anyhow::Result<()> {
        let source = "TOP = 1
ifdef X
TOP = 2
endif
DEFAULT = a
DEFAULT ?= b
all: a
all: b
clean::
\trm a
clean::
\trm b
dist = out
dist:
\ttar
";

        assert_eq!(
            symbols(&makefile(source)?, source),
            vec![
                symbol("/TOP", "TOP = 1", Some("1")),
                symbol("/TOP", "TOP = 2", Some("2")),
                symbol("/DEFAULT", "DEFAULT = a", Some("a")),
                symbol("/DEFAULT", "DEFAULT ?= b", Some("b")),
                symbol("/all", "all: a", None),
                symbol("/all", "all: b", None),
                symbol("/clean", "clean::\n\trm a", None),
                symbol("/clean", "clean::\n\trm b", None),
                symbol("/dist", "dist = out", Some("out")),
                symbol("/dist", "dist:\n\ttar", None),
            ]
        );
        Ok(())
    }

    #[test]
    fn conditional_after_recipe_parse_symbols_ends_the_rule_before_its_definitions()
    -> anyhow::Result<()> {
        // Each of these conditionals follows a recipe. One that holds a statement ends the rule
        // before it.
        let source = "test:
\tgo test ./...
ifeq ($(OS),Darwin)
SED = gsed
lint: tools
\tgolangci-lint run
endif
\tgolangci-lint fmt
debug:
\techo start
ifndef QUIET
\techo verbose
ifeq ($(V),2)
\techo very
endif
# Only on CI.
else ifneq ($(CI),)
\techo ci
else ifdef LOCAL_ONLY
\techo local
else
\techo quiet
endif
deploy:
\techo deploy
ifdef CI
\techo strict
else
LOCAL = 1
endif
";

        assert_eq!(
            symbols(&makefile(source)?, source),
            vec![
                // Make adds the recipe line after `endif` to `lint` when it reads that branch,
                // so the line belongs to no rule here.
                symbol("/test", "test:\n\tgo test ./...", None),
                symbol("/SED", "SED = gsed", Some("gsed")),
                symbol("/lint", "lint: tools\n\tgolangci-lint run", None),
                // Make runs the recipe lines of a conditional as part of the recipe.
                symbol("/debug", text_of(source, "debug:", "quiet\nendif")?, None),
                // Make reads `echo strict` as a recipe line of `deploy` when it reads that branch.
                symbol(
                    "/deploy",
                    text_of(source, "deploy:", "LOCAL = 1\nendif")?,
                    None
                ),
                symbol("/LOCAL", "LOCAL = 1", Some("1")),
            ]
        );
        Ok(())
    }

    #[test]
    fn assignment_with_keywords_in_any_order_parse_symbols_returns_it() -> anyhow::Result<()> {
        let source = "export override A = 1
override export B = 2
private override C = 3
override D != echo d
private E != echo e
build: export F = 4
test: G := 5
";

        assert_eq!(
            symbols(&makefile(source)?, source),
            vec![
                symbol("/A", "export override A = 1", Some("1")),
                symbol("/B", "override export B = 2", Some("2")),
                symbol("/C", "private override C = 3", Some("3")),
                symbol("/D", "override D != echo d", Some("echo d")),
                symbol("/E", "private E != echo e", Some("echo e")),
            ]
        );
        Ok(())
    }

    #[test]
    fn define_with_keywords_parse_symbols_returns_its_body() -> anyhow::Result<()> {
        // Make reads the body of `export define` as the value, not as statements.
        let source = "SCRIPT = old
export define SCRIPT
X = 1
endef
private define P
p
endef
export override define EO :=
eo
endef
define Q ?=
q
endef
undefine SCRIPT
";

        assert_eq!(
            symbols(&makefile(source)?, source),
            vec![
                symbol("/SCRIPT", "SCRIPT = old", Some("old")),
                symbol(
                    "/SCRIPT",
                    "export define SCRIPT\nX = 1\nendef",
                    Some("X = 1")
                ),
                symbol("/P", "private define P\np\nendef", Some("p")),
                symbol("/EO", "export override define EO :=\neo\nendef", Some("eo")),
                symbol("/Q", "define Q ?=\nq\nendef", Some("q")),
            ]
        );
        Ok(())
    }

    #[test]
    fn rule_parse_symbols_covers_only_its_own_recipe_lines() -> anyhow::Result<()> {
        let source = "all: build
ifeq ($(OS),Darwin)
# Holds no recipe line.
endif
ifeq ($(OS),Darwin)
build: x
else
build: y
endif
\tgo build
lint:
\tgolangci-lint run \\
  --fix
.RECIPEPREFIX = >
test:
> go test
X = 1
";

        assert_eq!(
            symbols(&makefile(source)?, source),
            vec![
                symbol("/all", "all: build", None),
                // Each rule ends with its branch. Make adds the recipe line after `endif` to the
                // rule of the branch it read, which a path can't tell.
                symbol("/build", "build: x", None),
                symbol("/build", "build: y", None),
                symbol("/lint", "lint:\n\tgolangci-lint run \\\n  --fix", None),
                symbol("/.RECIPEPREFIX", ".RECIPEPREFIX = >", Some(">")),
                symbol("/test", "test:\n> go test", None),
                symbol("/X", "X = 1", Some("1")),
            ]
        );
        Ok(())
    }

    #[test]
    fn target_kinds_parse_symbols_returns_each_named_target() -> anyhow::Result<()> {
        let source = "a.o b.o: %.o: %.c
$(addprefix bin/,a b): x
.c.o::
\t$(CC) -c $<
.y.c: ; $(YACC) $<
$(OBJS): %.o: %.c
bin/$(APP): main.go
all \\
  check: deps
build/app.map: $(APP:.elf=.map)
.: build
.X: build
";

        assert_eq!(
            symbols(&makefile(source)?, source),
            vec![
                // A static pattern rule makes its targets from a pattern, but each target is a
                // file.
                symbol("/a.o", "a.o b.o: %.o: %.c", None),
                symbol("/b.o", "a.o b.o: %.o: %.c", None),
                symbol("/all", "all \\\n  check: deps", None),
                symbol("/check", "all \\\n  check: deps", None),
                symbol("/build~1app.map", "build/app.map: $(APP:.elf=.map)", None),
                // `.` is the directory, not a special target. `.X` is written like one.
                symbol("/.", ".: build", None),
            ]
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LineIndex, Position, language_parsers::Comment};

    #[test]
    fn parses_comments_correctly() {
        let mut comments_parser = MakefileCommentsParser;

        let blocks: Vec<Comment> = comments_parser
            .parse(
                r#"
# This is a comment
all:
	@echo "Hello" # Inline comment

# Another comment
# spanning multiple lines
"#,
            )
            .collect();

        assert_eq!(
            blocks,
            vec![
                Comment {
                    position_range: Position::new(2, 1)..Position::new(2, 20),
                    source_range: 1..20,
                    comment_text: "  This is a comment".to_string()
                },
                Comment {
                    position_range: Position::new(6, 1)..Position::new(6, 18),
                    source_range: 59..76,
                    comment_text: "  Another comment".to_string()
                },
                Comment {
                    position_range: Position::new(7, 1)..Position::new(7, 26),
                    source_range: 77..102,
                    comment_text: "  spanning multiple lines".to_string()
                },
            ]
        );
    }

    #[test]
    fn assignment_with_inline_comment_has_comment() {
        let comments = comments_in(
            "A = 1 # equals
B := 2 # colon equals
C ?= 3 # question equals
D != echo 4 # shell
export E = 5 # export
override F = 6 # override
private G = 7 # private
build: H = 8 # target-specific
I = 9\t# after a tab
J = 10
",
        );

        assert_eq!(
            comments,
            [
                "# equals",
                "# colon equals",
                "# question equals",
                "# shell",
                "# export",
                "# override",
                "# private",
                "# target-specific",
                "# after a tab",
            ]
        );
    }

    #[test]
    fn statement_of_any_kind_has_comment() {
        let comments = comments_in(
            "ifeq ($(A),1) # one
B = 1
else # two
B = 2
endif # three
include other.mk # four
export B # five
$(info text) # six
build: deps \\
  more # seven
\tgo build
",
        );

        assert_eq!(
            comments,
            [
                "# one", "# two", "# three", "# four", "# five", "# six", "# seven"
            ]
        );
    }

    #[test]
    fn parenthesis_in_inline_comment_has_comment() {
        let comments = comments_in(
            r#"A := 1.2.3 # pinned (see the docs)
B := 1.2.3 # same-as-pattern="(?P<value>.+)"
build:
	go build
"#,
        );

        assert_eq!(
            comments,
            [
                "# pinned (see the docs)",
                r#"# same-as-pattern="(?P<value>.+)""#
            ]
        );
    }

    #[test]
    fn parenthesis_in_value_has_comment() {
        let comments = comments_in(
            "A = red (blue) # one
B = $(@:.elf=.map) # two
",
        );

        assert_eq!(comments, ["# one", "# two"]);
    }

    #[test]
    fn hash_after_odd_backslashes_is_not_a_comment() {
        let comments = comments_in(
            r"A = a\#b # one
B = a\\# two
C = a\\\#b # three
",
        );

        assert_eq!(comments, ["# one", "# two", "# three"]);
    }

    #[test]
    fn hash_inside_reference_is_not_a_comment() {
        let comments = comments_in(
            "A = $(shell echo #) # one
B = $(subst $(A),#,y) # two
C = ${A} # three
E = a$$# four
",
        );

        assert_eq!(comments, ["# one", "# two", "# three", "# four"]);
    }

    #[test]
    fn semicolon_on_rule_line_starts_inline_recipe() {
        // Make passes the rest of the line after `;` to the shell.
        let comments = comments_in(
            "build: ; go build # shell
test:: ; go test # shell
check: $(foreach dir,a b,$(dir);) # one
",
        );

        assert_eq!(comments, ["# one"]);
    }

    #[test]
    fn comment_line_in_recipe_has_comment_and_keeps_the_recipe() {
        let comments = comments_in(
            "build:
\tgo build
# one
\tgo test # shell
",
        );

        assert_eq!(comments, ["# one"]);
    }

    #[test]
    fn recipe_line_starting_with_hash_has_comment() {
        // Make passes these lines to the shell, so it reads no comment in them. Makefile-lossless
        // finds one on a line that starts with `#`, and blockwatch follows it.
        let comments = comments_in(
            "build:
\t# one $(A) two
\techo # shell
",
        );

        assert_eq!(comments, ["# one $(A) two"]);
    }

    #[test]
    fn define_body_with_hash_has_comment() {
        // Make keeps a `#` in a `define` body as part of the value. Makefile-lossless finds a
        // comment there, and blockwatch follows it.
        let comments = comments_in(
            "define A # zero
# one
endef\t# two
export define B :=
X = 1 # three
endef
# four
",
        );

        assert_eq!(comments, ["# zero", "# one", "# two", "# three", "# four"]);
    }

    #[test]
    fn inline_comment_on_continued_line_starts_on_that_line() {
        let comments = comments_in(
            r"A = one \
    two # one
B = 1 # two \
    continued
",
        );

        assert_eq!(comments, ["# one", "# two \\\n    continued"]);
    }

    #[test]
    fn comment_ending_in_odd_backslashes_takes_in_the_next_line() {
        // Make joins the lines first, so the next line is part of the comment. Before a blank
        // line, the comment ends on its own line.
        let comments = comments_in(
            r"# one \
A = 1 # in the comment above
# two \\
B = 2 # three
# four \

C = 3
",
        );

        assert_eq!(
            comments,
            [
                "# one \\\nA = 1 # in the comment above",
                "# two \\\\",
                "# three",
                "# four \\"
            ]
        );
    }

    #[test]
    fn crlf_line_endings_have_comments_without_carriage_returns() {
        let comments =
            comments_in("A = 1 # one\r\n# two \\\r\nB = 2\r\n# three \\\r\n\r\nC = 3\r\n");

        assert_eq!(comments, ["# one", "# two \\\r\nB = 2", "# three \\"]);
    }

    /// The source text of each comment in `source`, in order. Checks that each comment's
    /// positions and text match the bytes it covers.
    fn comments_in(source: &str) -> Vec<String> {
        let lines = LineIndex::new(source);
        let mut comments_parser = MakefileCommentsParser;
        comments_parser
            .parse(source)
            .map(|comment| {
                let text = &source[comment.source_range.clone()];
                assert_eq!(
                    comment.position_range,
                    lines.range(&comment.source_range),
                    "the positions of {text:?}"
                );
                assert_eq!(comment.comment_text, text.replacen('#', " ", 1));
                text.to_string()
            })
            .collect()
    }
}
