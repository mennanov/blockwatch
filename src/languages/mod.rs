//! One comment parser per supported language, most of them backed by tree-sitter.

mod bash;
mod c;
mod c_sharp;
mod cmake;
mod code_symbols;
mod comments;
mod cpp;
mod css;
mod dart;
mod dockerfile;
mod elixir;
mod file_names;
mod go;
mod graphql;
mod groovy;
mod hcl;
mod html;
mod java;
mod javascript;
mod json;
mod kotlin;
mod lean;
mod lua;
mod makefile;
mod markdown;
mod mdx;
mod nix;
mod ocaml;
mod php;
mod proto;
mod python;
mod ruby;
/// Rust comment parsing. Serves as the stand-in language for the block parser's own unit tests,
/// which is why it alone is visible outside this module.
pub(crate) mod rust;
mod scala;
mod sql;
mod starlark;
mod swift;
mod syntax_tree;
pub(crate) mod toml;
mod tsx;
mod typescript;
mod xml;
mod yaml;

pub(crate) use comments::{Comment, CommentsParser};

use crate::blocks::Block;
use crate::blocks::BlocksFromCommentsParser;
use crate::symbols::{Symbols, SymbolsParser};
use comments::{
    DOXYGEN_MARKERS, SLASH_AND_HASH_MARKERS, SLASH_MARKERS, TreeSitterCommentsParser,
    blank_decorative_stars, blank_line_marker, blank_outside, blank_preserving_line_breaks,
    c_style_comment_text, c_style_comments_parser, c_style_multiline_comment_processor,
    comment_visitor, ecmascript_comments_parser, python_style_comments_parser,
    xml_style_comments_parser,
};
use file_names::{LanguageResolver, NAMES};
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::Path;
use std::sync::{LazyLock, Mutex};
use syntax_tree::{ensure_no_syntax_error, members, node_text, tree_sitter_parser, walk_tree};

/// A language that blockwatch parses, one variant per parser.
///
/// Two names that need different parsers are two variants, even when people call them one
/// language: `Bash` and `Zsh`, `TypeScript` and `Tsx`, `Go` and `GoModule`.
// <block name="languages" affects="src/blocks/mod.rs:supported-extensions, README.md:supported-grammar">
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Language {
    Bash,
    C,
    CMake,
    CSharp,
    Cpp,
    Css,
    Dart,
    Dockerfile,
    Elixir,
    Go,
    /// `go.mod`, `go.sum` and `go.work`.
    GoModule,
    GraphQl,
    Groovy,
    Hcl,
    Html,
    Java,
    JavaScript,
    Json,
    Kotlin,
    Lean,
    Lua,
    Makefile,
    Markdown,
    Mdx,
    Nix,
    OCaml,
    Php,
    Proto,
    Python,
    Ruby,
    Rust,
    Scala,
    Sql,
    Starlark,
    Swift,
    Toml,
    Tsx,
    TypeScript,
    Xml,
    Yaml,
    Zsh,
}
// </block>

impl Language {
    /// The language of `key`, an extension without its dot (`rs`) or a whole file name
    /// (`Dockerfile`). Returns `None` when no parser reads such files.
    pub(crate) fn from_key(key: &str) -> Option<Self> {
        // The lookup runs several times for every file in the walk.
        static BY_NAME: LazyLock<HashMap<&str, Language>> =
            LazyLock::new(|| NAMES.iter().copied().collect());
        BY_NAME.get(key).copied()
    }
}

/// Parses the source files of one language.
pub(crate) trait LanguageParser: Send + Sync {
    /// Returns an iterator over the [`Block`]s found in the given `contents` string.
    ///
    /// The blocks are required to be yielded sorted by the `starts_at` field in ascending order.
    ///
    /// The iteration stops at the first error: whatever follows a malformed or unbalanced tag
    /// cannot be trusted to belong to the block the source intended.
    ///
    /// Returned boxed rather than as an `impl Iterator` because the trait is used as
    /// `dyn LanguageParser`, which a return-position `impl Trait` would rule out.
    fn parse_blocks<'a>(
        &'a mut self,
        contents: &'a str,
    ) -> Box<dyn Iterator<Item = anyhow::Result<Block>> + 'a>;

    /// Returns the addressable symbols defined in `contents`, and the names a reference must not
    /// resolve in it.
    ///
    /// # Errors
    /// Returns an error if the language has no symbols, or if they cannot be derived from
    /// `contents`.
    fn parse_symbols(&mut self, contents: &str) -> anyhow::Result<Symbols>;
}

/// A [`LanguageParser`] that finds blocks in the comments `C` extracts, and symbols if the language
/// has a [`SymbolsParser`].
struct LanguageParserImpl<C: CommentsParser> {
    blocks_parser: BlocksFromCommentsParser<C>,
    symbols_parser: Option<Box<dyn SymbolsParser>>,
}

impl<C: CommentsParser> LanguageParserImpl<C> {
    /// Creates a parser for a language whose comments `comments_parser` extracts, and which has
    /// no symbols.
    fn new(comments_parser: C) -> Self {
        Self {
            blocks_parser: BlocksFromCommentsParser::new(comments_parser),
            symbols_parser: None,
        }
    }

    /// Gives the language the symbols that `symbols_parser` derives.
    fn with_symbols(self, symbols_parser: impl SymbolsParser + 'static) -> Self {
        Self {
            symbols_parser: Some(Box::new(symbols_parser)),
            ..self
        }
    }
}

impl<C: CommentsParser> LanguageParser for LanguageParserImpl<C> {
    fn parse_blocks<'a>(
        &'a mut self,
        contents: &'a str,
    ) -> Box<dyn Iterator<Item = anyhow::Result<Block>> + 'a> {
        Box::new(self.blocks_parser.parse_blocks(contents))
    }

    fn parse_symbols(&mut self, contents: &str) -> anyhow::Result<Symbols> {
        match &mut self.symbols_parser {
            Some(symbols_parser) => symbols_parser.parse(contents),
            None => anyhow::bail!("symbols are not supported for this language"),
        }
    }
}

/// A parser for one language, shared between files and threads.
///
/// Each parser owns a mutable tree-sitter `Parser` and is therefore behind a `Mutex`.
pub(crate) type SharedLanguageParser = Mutex<Box<dyn LanguageParser>>;

/// The language parsers of a run, one for each [`Language`], and how a file name selects one.
pub(crate) struct LanguageParsers {
    resolver: LanguageResolver,
    bash: SharedLanguageParser,
    c: SharedLanguageParser,
    cmake: SharedLanguageParser,
    c_sharp: SharedLanguageParser,
    cpp: SharedLanguageParser,
    css: SharedLanguageParser,
    dart: SharedLanguageParser,
    dockerfile: SharedLanguageParser,
    elixir: SharedLanguageParser,
    go: SharedLanguageParser,
    go_module: SharedLanguageParser,
    graphql: SharedLanguageParser,
    groovy: SharedLanguageParser,
    hcl: SharedLanguageParser,
    html: SharedLanguageParser,
    java: SharedLanguageParser,
    javascript: SharedLanguageParser,
    json: SharedLanguageParser,
    kotlin: SharedLanguageParser,
    lean: SharedLanguageParser,
    lua: SharedLanguageParser,
    makefile: SharedLanguageParser,
    markdown: SharedLanguageParser,
    mdx: SharedLanguageParser,
    nix: SharedLanguageParser,
    ocaml: SharedLanguageParser,
    php: SharedLanguageParser,
    proto: SharedLanguageParser,
    python: SharedLanguageParser,
    ruby: SharedLanguageParser,
    rust: SharedLanguageParser,
    scala: SharedLanguageParser,
    sql: SharedLanguageParser,
    starlark: SharedLanguageParser,
    swift: SharedLanguageParser,
    toml: SharedLanguageParser,
    tsx: SharedLanguageParser,
    typescript: SharedLanguageParser,
    xml: SharedLanguageParser,
    yaml: SharedLanguageParser,
    zsh: SharedLanguageParser,
}

impl LanguageParsers {
    /// Builds every language parser, with no extension mappings.
    ///
    /// # Panics
    /// Panics if the tree-sitter crate doesn't support a grammar's ABI version, or if the PHP
    /// grammar lacks the nodes its comments query reads. Either is a build bug.
    pub(crate) fn new() -> Self {
        fn parser<P: LanguageParser + 'static>(p: P) -> SharedLanguageParser {
            Mutex::new(Box::new(p))
        }

        Self {
            resolver: LanguageResolver::new(HashMap::new()),
            bash: parser(bash::parser()),
            c: parser(c::parser()),
            cmake: parser(cmake::parser()),
            c_sharp: parser(c_sharp::parser()),
            cpp: parser(cpp::parser()),
            css: parser(css::parser()),
            dart: parser(dart::parser()),
            dockerfile: parser(dockerfile::parser()),
            elixir: parser(elixir::parser()),
            go: parser(go::parser()),
            go_module: parser(go::module_files_parser()),
            graphql: parser(graphql::parser()),
            groovy: parser(groovy::parser()),
            hcl: parser(hcl::parser()),
            html: parser(html::parser()),
            java: parser(java::parser()),
            javascript: parser(javascript::parser()),
            json: parser(json::parser()),
            kotlin: parser(kotlin::parser()),
            lean: parser(lean::parser()),
            lua: parser(lua::parser()),
            makefile: parser(makefile::parser()),
            markdown: parser(markdown::parser()),
            mdx: parser(mdx::parser()),
            nix: parser(nix::parser()),
            ocaml: parser(ocaml::parser()),
            php: parser(php::parser()),
            proto: parser(proto::parser()),
            python: parser(python::parser()),
            ruby: parser(ruby::parser()),
            rust: parser(rust::parser()),
            scala: parser(scala::parser()),
            sql: parser(sql::parser()),
            starlark: parser(starlark::parser()),
            swift: parser(swift::parser()),
            toml: parser(toml::parser()),
            tsx: parser(tsx::parser()),
            typescript: parser(typescript::parser()),
            xml: parser(xml::parser()),
            yaml: parser(yaml::parser()),
            zsh: parser(bash::zsh_parser()),
        }
    }

    /// Uses `extension_mappings` to select a parser for the extensions and file names they map.
    pub(crate) fn with_extension_mappings(
        self,
        extension_mappings: HashMap<OsString, Language>,
    ) -> Self {
        Self {
            resolver: LanguageResolver::new(extension_mappings),
            ..self
        }
    }

    /// The parser for `file_path`, or `None` if its name has no language.
    pub(crate) fn parser_for(&self, file_path: &Path) -> Option<&SharedLanguageParser> {
        self.resolver
            .resolve(file_path)
            .map(|language| self.parser(language))
    }

    /// The parser for `language`.
    pub(crate) fn parser(&self, language: Language) -> &SharedLanguageParser {
        match language {
            Language::Bash => &self.bash,
            Language::C => &self.c,
            Language::CMake => &self.cmake,
            Language::CSharp => &self.c_sharp,
            Language::Cpp => &self.cpp,
            Language::Css => &self.css,
            Language::Dart => &self.dart,
            Language::Dockerfile => &self.dockerfile,
            Language::Elixir => &self.elixir,
            Language::Go => &self.go,
            Language::GoModule => &self.go_module,
            Language::GraphQl => &self.graphql,
            Language::Groovy => &self.groovy,
            Language::Hcl => &self.hcl,
            Language::Html => &self.html,
            Language::Java => &self.java,
            Language::JavaScript => &self.javascript,
            Language::Json => &self.json,
            Language::Kotlin => &self.kotlin,
            Language::Lean => &self.lean,
            Language::Lua => &self.lua,
            Language::Makefile => &self.makefile,
            Language::Markdown => &self.markdown,
            Language::Mdx => &self.mdx,
            Language::Nix => &self.nix,
            Language::OCaml => &self.ocaml,
            Language::Php => &self.php,
            Language::Proto => &self.proto,
            Language::Python => &self.python,
            Language::Ruby => &self.ruby,
            Language::Rust => &self.rust,
            Language::Scala => &self.scala,
            Language::Sql => &self.sql,
            Language::Starlark => &self.starlark,
            Language::Swift => &self.swift,
            Language::Toml => &self.toml,
            Language::Tsx => &self.tsx,
            Language::TypeScript => &self.typescript,
            Language::Xml => &self.xml,
            Language::Yaml => &self.yaml,
            Language::Zsh => &self.zsh,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::symbol_path::SymbolPath;

    #[test]
    fn languages_with_symbols_match_the_documented_list() {
        let parsers = LanguageParsers::new();
        let mut with_symbols = Vec::new();
        for &(_, language) in NAMES {
            if with_symbols.contains(&language) {
                continue;
            }
            // A parser without symbols refuses every source with the same error, so an empty
            // source is enough to tell. A parser with symbols may accept it or report it as broken.
            let has_symbols = match parsers
                .parser(language)
                .lock()
                .expect("a thread panicked while it held the parser")
                .parse_symbols("")
            {
                Ok(_) => true,
                Err(err) => err.to_string() != "symbols are not supported for this language",
            };
            if has_symbols {
                with_symbols.push(language);
            }
        }

        // Each language with the name the docs give it. The docs list languages, not file names,
        // so a new file name of a language with symbols needs no change here.
        /* <block name="languages-with-symbols"
        same-as="docs/symbols.md:languages-with-symbols, src/skill.md:languages-with-symbols"
        same-as-pattern='"(?P<value>[^"]+)"'> */
        let documented = [
            (Language::Json, "JSON"),
            (Language::Toml, "TOML"),
            (Language::Yaml, "YAML"),
            (Language::Markdown, "Markdown"),
            (Language::Mdx, "MDX"),
            (Language::Xml, "XML"),
            (Language::Css, "CSS"),
            (Language::Go, "Go"),
            (Language::C, "C"),
            (Language::Cpp, "C++"),
            (Language::Python, "Python"),
            (Language::Starlark, "Starlark"),
            (Language::Rust, "Rust"),
            (Language::TypeScript, "TypeScript"),
            (Language::Tsx, "TypeScript"),
            (Language::JavaScript, "JavaScript"),
            (Language::Java, "Java"),
            (Language::CSharp, "C#"),
            (Language::Kotlin, "Kotlin"),
            (Language::Swift, "Swift"),
            (Language::Proto, "Protocol Buffers"),
            (Language::Hcl, "HCL"),
            (Language::Php, "PHP"),
            (Language::Bash, "Bash"),
            (Language::Makefile, "Makefile"),
            (Language::Dart, "Dart"),
            (Language::Dockerfile, "Dockerfile"),
        ];
        // </block>
        let mut documented: Vec<Language> =
            documented.iter().map(|&(language, _)| language).collect();
        with_symbols.sort_by_key(|language| format!("{language:?}"));
        documented.sort_by_key(|language| format!("{language:?}"));
        assert_eq!(with_symbols, documented);
    }

    #[test]
    fn language_without_symbols_parse_symbols_returns_error() -> anyhow::Result<()> {
        let mut parser = ruby::parser();

        let err = parser.parse_symbols("def main; end").unwrap_err();

        assert!(
            err.to_string()
                .contains("symbols are not supported for this language"),
            "unexpected error message: {err}"
        );
        Ok(())
    }

    #[test]
    fn typescript_file_with_type_assertion_resolves_its_symbols() -> anyhow::Result<()> {
        let parsers = LanguageParsers::new();
        let path = SymbolPath::parse("/a")?;
        let resolves = |parser: &SharedLanguageParser| -> anyhow::Result<bool> {
            let symbols = parser
                .lock()
                .expect("a thread panicked while it held the parser")
                .parse_symbols("let a = <string>b;\n")?;
            Ok(symbols.resolve(&path).is_ok())
        };

        let typescript = parsers
            .parser_for(Path::new("a.ts"))
            .expect("`.ts` files have a parser");
        assert!(resolves(typescript)?);
        // TSX reads `<string>` as the start of an element, so the source tells the two apart.
        assert!(!resolves(parsers.parser(Language::Tsx))?);
        Ok(())
    }
}
