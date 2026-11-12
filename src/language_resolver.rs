//! Which language a file is, decided from its name.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::sync::LazyLock;

/// A language that blockwatch parses, one variant per parser.
///
/// Two names that need different parsers are two variants, even when people call them one
/// language: `Bash` and `Zsh`, `TypeScript` and `Tsx`, `Go` and `GoModule`.
// <block name="languages" affects="src/blocks.rs:supported-extensions">
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

/// Each extension without its dot, such as `rs`, or whole file name, such as `Dockerfile`, with
/// the language of the files it fits.
pub(crate) const NAMES: &[(&str, Language)] = {
    use Language::*;
    &[
        /* <block name="language-names"
        affects="README.md:supported-grammar, src/language_resolver.rs:language-name-rows"
        keep-sorted keep-unique='\("(?P<value>[^"]+)"'> */
        ("BUCK", Starlark),
        ("BUILD", Starlark),
        ("Brewfile", Ruby),
        ("CMakeLists.txt", CMake),
        ("Containerfile", Dockerfile),
        ("Dockerfile", Dockerfile),
        ("Fastfile", Ruby),
        ("GNUmakefile", Makefile),
        ("Gemfile", Ruby),
        ("Jenkinsfile", Groovy),
        ("Makefile", Makefile),
        ("Pipfile", Toml),
        ("Podfile", Ruby),
        ("Rakefile", Ruby),
        ("Tiltfile", Starlark),
        ("Vagrantfile", Ruby),
        ("WORKSPACE", Starlark),
        ("bash", Bash),
        ("bash_profile", Bash),
        ("bashrc", Bash),
        ("bazel", Starlark),
        ("builder", Ruby),
        ("bzl", Starlark),
        ("bzlmod", Starlark),
        ("c", C),
        ("cc", Cpp),
        ("cff", Yaml),
        ("cjs", JavaScript),
        ("cmake", CMake),
        ("containerfile", Dockerfile),
        ("cpp", Cpp),
        ("cs", CSharp),
        ("csproj", Xml),
        ("css", Css),
        ("cts", TypeScript),
        ("cxx", Cpp),
        ("d.ts", TypeScript),
        ("dart", Dart),
        ("dockerfile", Dockerfile),
        ("envrc", Bash),
        ("ex", Elixir),
        ("exs", Elixir),
        ("fsproj", Xml),
        ("gemspec", Ruby),
        ("go", Go),
        ("go.mod", GoModule),
        ("go.sum", GoModule),
        ("go.work", GoModule),
        ("gql", GraphQl),
        ("gradle", Groovy),
        ("graphql", GraphQl),
        ("graphqls", GraphQl),
        ("groovy", Groovy),
        ("h", Cpp),
        ("hcl", Hcl),
        ("hh", Cpp),
        ("hpp", Cpp),
        ("htm", Html),
        ("html", Html),
        ("hxx", Cpp),
        ("java", Java),
        ("jbuilder", Ruby),
        ("jenkinsfile", Groovy),
        ("js", JavaScript),
        ("json", Json),
        ("jsonc", Json),
        ("jsx", JavaScript),
        ("kt", Kotlin),
        ("kts", Kotlin),
        ("lean", Lean),
        ("lua", Lua),
        ("mak", Makefile),
        ("makefile", Makefile),
        ("markdown", Markdown),
        ("md", Markdown),
        ("mdx", Mdx),
        ("mjs", JavaScript),
        ("mk", Makefile),
        ("ml", OCaml),
        ("mli", OCaml),
        ("mts", TypeScript),
        ("nix", Nix),
        ("nomad", Hcl),
        ("php", Php),
        ("phtml", Php),
        ("podspec", Ruby),
        ("props", Xml),
        ("proto", Proto),
        ("py", Python),
        ("pyi", Python),
        ("pyw", Python),
        ("rake", Ruby),
        ("rb", Ruby),
        ("rockspec", Lua),
        ("rs", Rust),
        ("ru", Ruby),
        ("sbt", Scala),
        ("scala", Scala),
        ("sh", Bash),
        ("sql", Sql),
        ("star", Starlark),
        ("swift", Swift),
        ("targets", Xml),
        ("tf", Hcl),
        ("tfvars", Hcl),
        ("tofu", Hcl),
        ("toml", Toml),
        ("ts", TypeScript),
        ("tsx", Tsx),
        ("vbproj", Xml),
        ("vcxproj", Xml),
        ("webmanifest", Json),
        ("xaml", Xml),
        ("xhtml", Html),
        ("xml", Xml),
        ("xsd", Xml),
        ("xsl", Xml),
        ("xslt", Xml),
        ("yaml", Yaml),
        ("yml", Yaml),
        ("zsh", Zsh),
        ("zshrc", Zsh),
        // </block>
    ]
};

/// The names whose variants, such as `Dockerfile.ci`, are files of the same language. Each has its
/// language, and the `.`-separated parts that make a variant some other kind of file.
/// `Dockerfile.dockerignore` is Docker's ignore file for `Dockerfile`, not a Dockerfile.
const NAME_RULES: &[(&str, Language, &[&str])] = &[
    ("Containerfile", Language::Dockerfile, &["dockerignore"]),
    ("Dockerfile", Language::Dockerfile, &["dockerignore"]),
];

/// Decides which language a file is, from its name alone.
pub(crate) struct LanguageResolver {
    /// Maps an extension or a whole file name to a language. For example, `ino` to
    /// `Language::Cpp` makes `.ino` files parse as C++.
    extension_mappings: HashMap<OsString, Language>,
}

impl LanguageResolver {
    /// A resolver where `extension_mappings` win over the languages from [`Language::from_key`].
    pub(crate) fn new(extension_mappings: HashMap<OsString, Language>) -> Self {
        Self { extension_mappings }
    }

    /// The language of the file at `path`, or `None` when no parser reads it. Only the file name
    /// counts, not the directories or the content.
    pub(crate) fn resolve(&self, path: &Path) -> Option<Language> {
        let file_name = path.file_name()?.to_str()?;

        // The last extension says what the file is now, so `Dockerfile.ci.yml` is YAML.
        for (i, _) in file_name.rmatch_indices('.') {
            #[expect(clippy::string_slice, reason = "`i` is the index of a `.`")]
            let extension = &file_name[i + 1..];
            if let Some(language) = self.language_of(extension) {
                return Some(language);
            }
        }

        self.language_of(file_name)
            .or_else(|| name_rule_language(file_name))
    }

    /// The language of `key`, an extension or a whole file name, with the mappings applied.
    fn language_of(&self, key: &str) -> Option<Language> {
        match self.extension_mappings.get(OsStr::new(key)) {
            Some(language) => Some(*language),
            None => Language::from_key(key),
        }
    }
}

/// The language of `file_name` by [`NAME_RULES`], such as Dockerfile for `Dockerfile.ci`.
fn name_rule_language(file_name: &str) -> Option<Language> {
    let (name, variant) = file_name.split_once('.')?;
    let &(_, language, excluded_parts) = NAME_RULES
        .iter()
        .find(|(rule_name, _, _)| *rule_name == name)?;
    if variant
        .split('.')
        .any(|part| excluded_parts.contains(&part))
    {
        return None;
    }
    // A mapping of `name` doesn't count: it is meant for `name` itself, not for its variants.
    Some(language)
}

#[cfg(test)]
mod tests {
    use super::*;
    use Language::*;

    /// Lists each `(file name, expected, actual)` whose actual language is not the expected one.
    fn mismatches<'a>(
        resolver: &LanguageResolver,
        rows: &[(&'a str, Option<Language>)],
    ) -> Vec<(&'a str, Option<Language>, Option<Language>)> {
        rows.iter()
            .map(|&(file_name, expected)| {
                (file_name, expected, resolver.resolve(Path::new(file_name)))
            })
            .filter(|(_, expected, actual)| expected != actual)
            .collect()
    }

    #[test]
    fn file_name_resolves_to_its_language() {
        let rows = [
            // <block name="language-name-rows">
            ("BUCK", Some(Starlark)),
            ("BUILD", Some(Starlark)),
            ("Brewfile", Some(Ruby)),
            ("CMakeLists.txt", Some(CMake)),
            ("Containerfile", Some(Dockerfile)),
            ("Dockerfile", Some(Dockerfile)),
            ("Fastfile", Some(Ruby)),
            ("GNUmakefile", Some(Makefile)),
            ("Gemfile", Some(Ruby)),
            ("Jenkinsfile", Some(Groovy)),
            ("Makefile", Some(Makefile)),
            ("Pipfile", Some(Toml)),
            ("Podfile", Some(Ruby)),
            ("Rakefile", Some(Ruby)),
            ("Tiltfile", Some(Starlark)),
            ("Vagrantfile", Some(Ruby)),
            ("WORKSPACE", Some(Starlark)),
            ("a.bash", Some(Bash)),
            (".bash_profile", Some(Bash)),
            (".bashrc", Some(Bash)),
            ("MODULE.bazel", Some(Starlark)),
            ("a.builder", Some(Ruby)),
            ("a.bzl", Some(Starlark)),
            ("WORKSPACE.bzlmod", Some(Starlark)),
            ("a.c", Some(C)),
            ("a.cc", Some(Cpp)),
            ("CITATION.cff", Some(Yaml)),
            ("a.cjs", Some(JavaScript)),
            ("a.cmake", Some(CMake)),
            ("a.containerfile", Some(Dockerfile)),
            ("a.cpp", Some(Cpp)),
            ("a.cs", Some(CSharp)),
            ("a.csproj", Some(Xml)),
            ("a.css", Some(Css)),
            ("a.cts", Some(TypeScript)),
            ("a.cxx", Some(Cpp)),
            ("a.d.ts", Some(TypeScript)),
            ("a.dart", Some(Dart)),
            ("a.dockerfile", Some(Dockerfile)),
            (".envrc", Some(Bash)),
            ("a.ex", Some(Elixir)),
            ("a.exs", Some(Elixir)),
            ("a.fsproj", Some(Xml)),
            ("a.gemspec", Some(Ruby)),
            ("a.go", Some(Go)),
            ("go.mod", Some(GoModule)),
            ("go.sum", Some(GoModule)),
            ("go.work", Some(GoModule)),
            ("a.gql", Some(GraphQl)),
            ("build.gradle", Some(Groovy)),
            ("a.graphql", Some(GraphQl)),
            ("a.graphqls", Some(GraphQl)),
            ("a.groovy", Some(Groovy)),
            ("a.h", Some(Cpp)),
            ("a.hcl", Some(Hcl)),
            ("a.hh", Some(Cpp)),
            ("a.hpp", Some(Cpp)),
            ("a.htm", Some(Html)),
            ("a.html", Some(Html)),
            ("a.hxx", Some(Cpp)),
            ("a.java", Some(Java)),
            ("a.jbuilder", Some(Ruby)),
            ("a.jenkinsfile", Some(Groovy)),
            ("a.js", Some(JavaScript)),
            ("a.json", Some(Json)),
            ("a.jsonc", Some(Json)),
            ("a.jsx", Some(JavaScript)),
            ("a.kt", Some(Kotlin)),
            ("a.kts", Some(Kotlin)),
            ("a.lean", Some(Lean)),
            ("a.lua", Some(Lua)),
            ("a.mak", Some(Makefile)),
            ("a.makefile", Some(Makefile)),
            ("a.markdown", Some(Markdown)),
            ("a.md", Some(Markdown)),
            ("a.mdx", Some(Mdx)),
            ("a.mjs", Some(JavaScript)),
            ("a.mk", Some(Makefile)),
            ("a.ml", Some(OCaml)),
            ("a.mli", Some(OCaml)),
            ("a.mts", Some(TypeScript)),
            ("a.nix", Some(Nix)),
            ("a.nomad", Some(Hcl)),
            ("a.php", Some(Php)),
            ("a.phtml", Some(Php)),
            ("a.podspec", Some(Ruby)),
            ("a.props", Some(Xml)),
            ("a.proto", Some(Proto)),
            ("a.py", Some(Python)),
            ("a.pyi", Some(Python)),
            ("a.pyw", Some(Python)),
            ("a.rake", Some(Ruby)),
            ("a.rb", Some(Ruby)),
            ("a.rockspec", Some(Lua)),
            ("main.rs", Some(Rust)),
            ("config.ru", Some(Ruby)),
            ("build.sbt", Some(Scala)),
            ("a.scala", Some(Scala)),
            ("a.sh", Some(Bash)),
            ("a.sql", Some(Sql)),
            ("a.star", Some(Starlark)),
            ("a.swift", Some(Swift)),
            ("a.targets", Some(Xml)),
            ("a.tf", Some(Hcl)),
            ("a.tfvars", Some(Hcl)),
            ("a.tofu", Some(Hcl)),
            ("a.toml", Some(Toml)),
            ("a.ts", Some(TypeScript)),
            ("a.tsx", Some(Tsx)),
            ("a.vbproj", Some(Xml)),
            ("a.vcxproj", Some(Xml)),
            ("site.webmanifest", Some(Json)),
            ("a.xaml", Some(Xml)),
            ("a.xhtml", Some(Html)),
            ("a.xml", Some(Xml)),
            ("a.xsd", Some(Xml)),
            ("a.xsl", Some(Xml)),
            ("a.xslt", Some(Xml)),
            ("a.yaml", Some(Yaml)),
            ("a.yml", Some(Yaml)),
            ("a.zsh", Some(Zsh)),
            (".zshrc", Some(Zsh)),
            // </block>
        ];

        assert_eq!(
            mismatches(&LanguageResolver::new(HashMap::new()), &rows),
            []
        );
    }

    #[test]
    fn name_with_suffix_resolves_to_its_language() {
        let rows = [
            ("Dockerfile.ci", Some(Dockerfile)),
            ("Containerfile.prod", Some(Dockerfile)),
            ("Dockerfile.ci.prod", Some(Dockerfile)),
            ("Dockerfile.", Some(Dockerfile)),
            // An extension with a language comes first.
            ("Dockerfile.ci.yml", Some(Yaml)),
            ("a.rs.unknown.json", Some(Json)),
        ];

        assert_eq!(
            mismatches(&LanguageResolver::new(HashMap::new()), &rows),
            []
        );
    }

    #[test]
    fn mapped_name_resolves_to_mapped_language() {
        // Each row is the mapping's key, its language, the file name and the expected language.
        let rows = [
            ("ino", Cpp, "a.ino", Some(Cpp)),
            ("Buildfile", Dockerfile, "Buildfile", Some(Dockerfile)),
            ("x.y", Rust, "a.x.y", Some(Rust)),
            // A mapping replaces the language a name has without it.
            ("h", C, "a.h", Some(C)),
            ("Dockerfile", Bash, "Dockerfile", Some(Bash)),
            // The shorter extension comes first, mapped or not.
            ("ino", Cpp, "a.rs.ino", Some(Cpp)),
            ("ino", Cpp, "a.ino.rs", Some(Rust)),
            ("y.rs", Python, "a.y.rs", Some(Rust)),
            // A mapping doesn't give a name a suffix rule, or take one away.
            ("Buildfile", Dockerfile, "Buildfile.ci", None),
            ("Dockerfile", Bash, "Dockerfile.ci", Some(Dockerfile)),
        ];

        let mismatches: Vec<_> = rows
            .iter()
            .map(|&(key, language, file_name, expected)| {
                let resolver = LanguageResolver::new(HashMap::from([(key.into(), language)]));
                (
                    key,
                    file_name,
                    expected,
                    resolver.resolve(Path::new(file_name)),
                )
            })
            .filter(|(_, _, expected, actual)| expected != actual)
            .collect();
        assert_eq!(mismatches, []);
    }

    #[test]
    fn nested_file_resolves_by_its_name() {
        let rows = [
            ("nested/dir/main.rs", Some(Rust)),
            ("nested/Dockerfile", Some(Dockerfile)),
            ("nested/Dockerfile.ci", Some(Dockerfile)),
            ("nested.rs/README", None),
        ];

        assert_eq!(
            mismatches(&LanguageResolver::new(HashMap::new()), &rows),
            []
        );
    }

    #[test]
    fn unknown_name_resolves_to_none() {
        let rows = [
            ("a.unknown", None),
            ("README", None),
            (".hidden", None),
            // `Dockerfile.dockerignore` is Docker's ignore file for `Dockerfile`.
            ("Dockerfile.dockerignore", None),
            ("Containerfile.dockerignore", None),
            ("Dockerfile.ci.dockerignore", None),
            ("Dockerfile.dockerignore.old", None),
            // A suffix rule needs the whole name, then a `.`.
            ("Dockerfile-dev", None),
            ("DockerfileWithoutDot", None),
            ("NotDockerfile.ci", None),
            // Only some names have a suffix rule.
            ("Jenkinsfile.release", None),
            ("Gemfile.lock", None),
            // Matching is case-sensitive.
            ("a.RS", None),
            ("DOCKERFILE", None),
            ("dockerfile.ci", None),
        ];

        assert_eq!(
            mismatches(&LanguageResolver::new(HashMap::new()), &rows),
            []
        );
    }
}
