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
mod parsers;
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

pub(crate) use comments::Comment;
pub(crate) use parsers::{LanguageParsers, SharedLanguageParser};

use crate::symbols::{Symbols, SymbolsParser};
use comments::{
    CommentsParser, DOXYGEN_MARKERS, SLASH_AND_HASH_MARKERS, SLASH_MARKERS,
    TreeSitterCommentsParser, blank_decorative_stars, blank_line_marker, blank_outside,
    blank_preserving_line_breaks, c_style_comment_text, c_style_comments_parser,
    c_style_multiline_comment_processor, comment_visitor, ecmascript_comments_parser,
    python_style_comments_parser, xml_style_comments_parser,
};
use strum_macros::EnumIter;
use syntax_tree::{ensure_no_syntax_error, members, node_text, tree_sitter_parser, walk_tree};

/// A language that blockwatch parses, one variant per parser.
///
/// Two names that need different parsers are two variants, even when people call them one
/// language: `Bash` and `Zsh`, `TypeScript` and `Tsx`, `Go` and `GoModule`.
// <block name="languages" affects="src/languages/parsers.rs:supported-extensions, README.md:supported-grammar">
#[derive(Clone, Copy, Debug, PartialEq, Eq, EnumIter)]
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

/// Parses the source files of one language.
pub(crate) trait LanguageParser: Send + Sync {
    /// Returns the comments in `contents`, in source order.
    ///
    /// Returned boxed rather than as an `impl Iterator` because the trait is used as
    /// `dyn LanguageParser`, which a return-position `impl Trait` would rule out.
    ///
    /// # Errors
    /// Returns an error if the comments can't be found, such as in an MDX file that is not valid
    /// MDX.
    fn parse_comments<'a>(
        &'a mut self,
        contents: &'a str,
    ) -> anyhow::Result<Box<dyn Iterator<Item = Comment> + 'a>>;

    /// Returns the addressable symbols defined in `contents`, and the names a reference must not
    /// resolve in it.
    ///
    /// # Errors
    /// Returns an error if the language has no symbols, or if they cannot be derived from
    /// `contents`.
    fn parse_symbols(&mut self, contents: &str) -> anyhow::Result<Symbols>;
}

/// A [`LanguageParser`] whose comments `C` extracts, with symbols if the language has a
/// [`SymbolsParser`].
struct LanguageParserImpl<C: CommentsParser> {
    comments_parser: C,
    symbols_parser: Option<Box<dyn SymbolsParser>>,
}

impl<C: CommentsParser> LanguageParserImpl<C> {
    /// Creates a parser for a language whose comments `comments_parser` extracts, and which has
    /// no symbols.
    fn new(comments_parser: C) -> Self {
        Self {
            comments_parser,
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
    fn parse_comments<'a>(
        &'a mut self,
        contents: &'a str,
    ) -> anyhow::Result<Box<dyn Iterator<Item = Comment> + 'a>> {
        Ok(Box::new(self.comments_parser.parse(contents)))
    }

    fn parse_symbols(&mut self, contents: &str) -> anyhow::Result<Symbols> {
        match &mut self.symbols_parser {
            Some(symbols_parser) => symbols_parser.parse(contents),
            None => anyhow::bail!("symbols are not supported for this language"),
        }
    }
}

#[cfg(test)]
pub(crate) mod test_utils {
    use super::LanguageParser;

    /// The source text of each comment that `parser` finds in `contents`, in the order of the
    /// comments in `contents`.
    ///
    /// # Errors
    /// Returns the error of [`LanguageParser::parse_comments`].
    pub(crate) fn comment_sources<'a>(
        parser: &mut impl LanguageParser,
        contents: &'a str,
    ) -> anyhow::Result<Vec<&'a str>> {
        Ok(parser
            .parse_comments(contents)?
            .map(|comment| &contents[comment.source_range])
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_without_symbols_parse_symbols_returns_error() {
        let mut parser = LanguageParserImpl::new(python_style_comments_parser(
            &tree_sitter_ruby::LANGUAGE.into(),
            "comment",
            &["#"],
        ));

        let err = parser.parse_symbols("def main; end").unwrap_err();

        assert!(
            err.to_string()
                .contains("symbols are not supported for this language"),
            "unexpected error message: {err}"
        );
    }
}
