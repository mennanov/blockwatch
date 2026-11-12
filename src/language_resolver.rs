//! Which language a file is, decided from its name.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::sync::LazyLock;

/// A language that blockwatch parses, one variant per parser.
///
/// Two names that need different parsers are two variants, even when people call them one
/// language: `Bash` and `Zsh`, `TypeScript` and `Tsx`, `Go` and `GoModule`.
// <block name="languages" affects="src/blocks.rs:supported-extensions, README.md:supported-grammar">
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
        affects="src/language_resolver.rs:language-name-rows"
        keep-sorted keep-unique='\("(?P<value>[^"]+)"' same-as-pattern='\("(?P<value>[^"]+)"'> */
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

/* <block name="detection-code"
affects="docs/cli.md:detection-rules, src/skill.md#/blockwatch/Symbols/Variants and copies"> */
/// The suffixes that mark a copy of another file in the same language, such as
/// `config.toml.example`. `.in` is the input that autoconf or CMake turns into the file.
const COPY_SUFFIXES: &[&str] = &[".dist", ".example", ".in", ".sample", ".template"];

/// The names whose variants are files of the same language, such as `Dockerfile.ci`,
/// `Dockerfile-dev` and `Dockerfile_prod`. Each row has the spellings of a name, their language,
/// and the `.`-separated parts that make a variant some other kind of file:
///
/// - `Dockerfile.dockerignore` is Docker's ignore file for `Dockerfile`.
/// - `Makefile.PL` is Perl's MakeMaker script, and `Makefile.SH` a shell script that writes a
///   Makefile. `Makefile.bat` and `Makefile.cmd` are Windows batch files.
const NAME_RULES: &[(&[&str], Language, &[&str])] = &[
    (
        &["Dockerfile", "dockerfile", "Containerfile", "containerfile"],
        Language::Dockerfile,
        &["dockerignore"],
    ),
    (&["Jenkinsfile", "jenkinsfile"], Language::Groovy, &[]),
    (
        &["GNUmakefile", "Makefile", "makefile"],
        Language::Makefile,
        &["PL", "SH", "bat", "cmd"],
    ),
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
    /// counts, not the directories or the content. The first of these that gives a language wins:
    ///
    /// 1. the endings of the name, the longest first, with the extension mappings;
    /// 2. the rest of the name before a copy suffix, by its endings and then its variant rule;
    /// 3. the variant rule of the name.
    pub(crate) fn resolve(&self, path: &Path) -> Option<Language> {
        let file_name = path.file_name()?.to_str()?;
        self.language_of_ending(file_name)
            .or_else(|| {
                // The copy suffix goes before the variant rule runs, because `Dockerfile.yml.example`
                // is an example YAML file, not a Dockerfile.
                let rest = COPY_SUFFIXES
                    .iter()
                    .find_map(|suffix| file_name.strip_suffix(suffix))?;
                self.language_of_ending(rest)
                    .or_else(|| name_rule_language(rest))
            })
            .or_else(|| name_rule_language(file_name))
    }

    /// The language of the longest ending of `name` that has one, where an ending is the whole name
    /// or what follows one of its `.`. Returns `None` when no ending has a language.
    fn language_of_ending(&self, name: &str) -> Option<Language> {
        // A longer ending is the more exact rule: `d.ts` over `ts`, and the name `CMakeLists.txt`
        // over a mapped `txt`. A shorter one only counts when no longer one has a language, as in
        // `Dockerfile.ci.yml`.
        if let Some(language) = self.language_of(name) {
            return Some(language);
        }
        for (i, _) in name.match_indices('.') {
            #[expect(clippy::string_slice, reason = "`i` is the index of a `.`")]
            let ending = &name[i + 1..];
            if let Some(language) = self.language_of(ending) {
                return Some(language);
            }
        }
        None
    }

    /// The language of `key`, an extension or a whole file name, with the mappings applied.
    fn language_of(&self, key: &str) -> Option<Language> {
        match self.extension_mappings.get(OsStr::new(key)) {
            Some(language) => Some(*language),
            None => Language::from_key(key),
        }
    }
}

/// The language of `file_name` by [`NAME_RULES`], such as Dockerfile for `Dockerfile-dev`.
fn name_rule_language(file_name: &str) -> Option<Language> {
    // A mapping of a name doesn't count: it is meant for the name itself, not for its variants.
    NAME_RULES
        .iter()
        .find_map(|&(names, language, excluded_parts)| {
            let variant = names.iter().find_map(|name| variant(file_name, name))?;
            if variant
                .split('.')
                .any(|part| excluded_parts.contains(&part))
            {
                return None;
            }
            Some(language)
        })
}

/// What follows `name` and its separator in `file_name`, such as `ci` in `Dockerfile.ci`. Returns
/// `None` when `file_name` is not a variant of `name`.
fn variant<'f>(file_name: &'f str, name: &str) -> Option<&'f str> {
    let rest = file_name.strip_prefix(name)?;
    if let Some(variant) = rest.strip_prefix('.') {
        return Some(variant);
    }
    let variant = rest.strip_prefix(['-', '_'])?;
    // Here a `.` starts an extension that blockwatch doesn't know, as in `Dockerfile-logo.png`, so
    // the file is something else.
    if variant.contains('.') {
        None
    } else {
        Some(variant)
    }
}
// </block>

#[cfg(test)]
mod tests {
    use super::*;
    use Language::*;

    /// Asserts that each `(file name, language)` row resolves to that language when the run has
    /// the extension `mappings`. A failure lists every row that resolves to something else.
    fn assert_resolves_with(mappings: &[(&str, Language)], rows: &[(&str, Option<Language>)]) {
        let resolver = LanguageResolver::new(
            mappings
                .iter()
                .map(|&(key, language)| (key.into(), language))
                .collect(),
        );
        let mismatches: Vec<_> = rows
            .iter()
            .map(|&(file_name, expected)| {
                (file_name, expected, resolver.resolve(Path::new(file_name)))
            })
            .filter(|(_, expected, actual)| expected != actual)
            .collect();
        assert_eq!(mismatches, [], "with the mappings {mappings:?}");
    }

    /// Asserts that each `(file name, language)` row resolves to that language.
    fn assert_resolves(rows: &[(&str, Language)]) {
        let rows: Vec<_> = rows
            .iter()
            .map(|&(file_name, language)| (file_name, Some(language)))
            .collect();
        assert_resolves_with(&[], &rows);
    }

    /// Asserts that none of `file_names` has a language.
    fn assert_no_language(file_names: &[&str]) {
        let rows: Vec<_> = file_names
            .iter()
            .map(|&file_name| (file_name, None))
            .collect();
        assert_resolves_with(&[], &rows);
    }

    /// The extension picks the language.
    #[test]
    fn file_with_known_extension_resolves_to_its_language() {
        assert_resolves(&[
            ("main.rs", Rust),
            ("app.ts", TypeScript),
            ("config.yml", Yaml),
            // A whole file name in the list counts as an extension too.
            ("ci.Dockerfile", Dockerfile),
        ]);
    }

    /// Some files are known by their whole name, which can hold a dot.
    #[test]
    fn file_with_known_name_resolves_to_its_language() {
        assert_resolves(&[
            ("Dockerfile", Dockerfile),
            ("CMakeLists.txt", CMake),
            ("go.mod", GoModule),
        ]);
    }

    /// The language comes from the end of the name. The longest ending with a language wins,
    /// because it is the most exact. The whole name is the longest ending.
    #[test]
    fn name_with_several_endings_resolves_by_the_longest_known_one() {
        assert_resolves(&[
            ("Dockerfile.ci.yml", Yaml),
            ("Dockerfile-dev.yml", Yaml),
            ("a.rs.unknown.json", Json),
        ]);
        // An extension in the middle of the name doesn't count.
        assert_no_language(&["a.rs.unknown"]);
        // Mapped endings follow the same rule.
        assert_resolves_with(
            &[("ino", Cpp)],
            &[("a.rs.ino", Some(Cpp)), ("a.ino.rs", Some(Rust))],
        );
        assert_resolves_with(&[("y.rs", Python)], &[("a.y.rs", Some(Python))]);
        assert_resolves_with(
            &[("ts", JavaScript)],
            &[
                ("index.d.ts", Some(TypeScript)),
                ("index.ts", Some(JavaScript)),
            ],
        );
        assert_resolves_with(&[("data.json", Yaml)], &[("data.json", Some(Yaml))]);
        // A mapped extension doesn't reach a name in the list.
        assert_resolves_with(
            &[("txt", Markdown)],
            &[
                ("CMakeLists.txt", Some(CMake)),
                ("CMakeLists.txt.in", Some(CMake)),
                ("notes.txt", Some(Markdown)),
            ],
        );
        // An ending with a language wins over stripping a copy suffix.
        assert_resolves_with(&[("in", Cpp)], &[("a.rs.in", Some(Cpp))]);
    }

    /// `Dockerfile`, `Containerfile`, `Jenkinsfile` and `Makefile`, in each spelling the list
    /// has, can be followed by `.`, `-` or `_` and anything else. The file keeps the name's
    /// language. An ending with a language still wins, as in `Dockerfile.ci.yml`.
    #[test]
    fn variant_of_known_name_resolves_to_its_language() {
        assert_resolves(&[
            ("Dockerfile.ci", Dockerfile),
            ("Dockerfile.ci.prod", Dockerfile),
            ("Dockerfile-dev", Dockerfile),
            ("Dockerfile_prod", Dockerfile),
            ("Dockerfile.", Dockerfile),
            ("Dockerfile-", Dockerfile),
            ("dockerfile.ci", Dockerfile),
            ("Containerfile.prod", Dockerfile),
            ("containerfile-dev", Dockerfile),
            ("Jenkinsfile.release", Groovy),
            ("jenkinsfile_release", Groovy),
            ("Makefile.am", Makefile),
            ("makefile.inc", Makefile),
            ("GNUmakefile-local", Makefile),
        ]);
    }

    /// A variant with some parts is another kind of file. `Dockerfile.dockerignore` is Docker's
    /// ignore file for `Dockerfile`, and `Makefile.PL` is Perl's MakeMaker script.
    #[test]
    fn variant_that_is_another_kind_of_file_has_no_language() {
        assert_no_language(&[
            "Dockerfile.dockerignore",
            "Dockerfile.ci.dockerignore",
            "Dockerfile.dockerignore.old",
            "Dockerfile-dev.dockerignore",
            "dockerfile.dockerignore",
            "Containerfile.dockerignore",
            "containerfile.dockerignore",
            "Makefile.PL",
            "Makefile.PL.old",
            "Makefile-PL",
            "makefile.PL",
            "GNUmakefile.PL",
            "Makefile.SH",
            "Makefile.bat",
            "Makefile.cmd",
        ]);
        // Each name has its own excluded parts.
        assert_resolves(&[("Dockerfile.PL", Dockerfile)]);
    }

    /// A variant needs the whole name at the start, then `.`, `-` or `_`. After `-` or `_`, a `.`
    /// starts an extension blockwatch doesn't know, so the file is something else. Other names
    /// have no variants.
    #[test]
    fn name_that_only_looks_like_a_variant_has_no_language() {
        assert_no_language(&[
            "Dockerfile-logo.png",
            "makefile-notes.txt",
            "dockerfile__tests__parse.snap",
            "Dockerfile-dev.ci",
            "Dockerfiles",
            "Makefiles",
            "Dockerfile+dev",
            "NotDockerfile.ci",
            "NotDockerfile-dev",
            "my-Dockerfile",
            "Gemfile.lock",
            "Vagrantfile-dev",
        ]);
    }

    /// A copy suffix marks a copy of another file in the same language: a sample (`.example`,
    /// `.sample`), a default (`.dist`), or an input that a tool turns into the file (`.in`,
    /// `.template`). It is stripped once, and the rest of the name picks the language by the rules
    /// above, mappings included. It is stripped before a variant is looked for, so
    /// `Dockerfile.yml.example` is an example YAML file.
    #[test]
    fn name_with_copy_suffix_resolves_by_the_rest() {
        assert_resolves(&[
            ("config.toml.example", Toml),
            ("config.h.in", Cpp),
            ("phpunit.xml.dist", Xml),
            ("settings.py.sample", Python),
            ("values.yaml.template", Yaml),
            ("CMakeLists.txt.in", CMake),
            ("Makefile.in", Makefile),
            ("Dockerfile-dev.example", Dockerfile),
            ("Dockerfile.yml.example", Yaml),
            // The rest can be a variant, as in gettext's `po/Makefile.in.in`.
            ("Makefile.in.in", Makefile),
        ]);
        assert_resolves_with(&[("ino", Cpp)], &[("a.ino.in", Some(Cpp))]);
        // A copy follows a mapping of the name it copies. A variant doesn't.
        assert_resolves_with(
            &[("Dockerfile", Bash)],
            &[
                ("Dockerfile.example", Some(Bash)),
                ("Dockerfile.ci", Some(Dockerfile)),
            ],
        );
        assert_no_language(&[
            // Other template languages and backups are not stripped.
            "values.yaml.j2",
            "a.rs.tmpl",
            "a.rs.orig",
            // The rest has no language.
            ".env.example",
            "config.in",
            "Dockerfile.dockerignore.example",
            // Only one suffix is stripped.
            "a.rs.in.in",
            "a.rs.example.in",
            // A suffix with nothing before it.
            ".example",
        ]);
    }

    /// An extension mapping, from `-E` or `[extensions]`, gives an extension or a whole file name
    /// a language, or replaces the one it has.
    #[test]
    fn extension_mapping_adds_or_replaces_a_language() {
        assert_resolves_with(&[("ino", Cpp)], &[("a.ino", Some(Cpp))]);
        assert_resolves_with(&[("x.y", Rust)], &[("a.x.y", Some(Rust))]);
        assert_resolves_with(
            &[("Buildfile", Dockerfile)],
            &[("Buildfile", Some(Dockerfile))],
        );
        assert_resolves_with(&[("h", C)], &[("a.h", Some(C))]);
        assert_resolves_with(&[("Dockerfile", Bash)], &[("Dockerfile", Some(Bash))]);
    }

    /// A mapping of a name covers that name only, not its variants. To map a variant, map its
    /// whole name.
    #[test]
    fn extension_mapping_does_not_apply_to_variants() {
        assert_resolves_with(
            &[("Buildfile", Dockerfile)],
            &[("Buildfile.ci", None), ("Buildfile-dev", None)],
        );
        assert_resolves_with(
            &[("Dockerfile", Bash)],
            &[("Dockerfile.ci", Some(Dockerfile))],
        );
        assert_resolves_with(
            &[("Buildfile.ci", Dockerfile)],
            &[("Buildfile.ci", Some(Dockerfile))],
        );
        assert_resolves_with(&[("Makefile.am", Bash)], &[("Makefile.am", Some(Bash))]);
    }

    /// Only the file name counts, not the directories.
    #[test]
    fn directory_does_not_change_the_language() {
        assert_resolves(&[
            ("nested/dir/main.rs", Rust),
            ("nested/Dockerfile", Dockerfile),
            ("nested/Dockerfile.ci", Dockerfile),
        ]);
        assert_no_language(&["nested.rs/README"]);
    }

    /// Case matters, because some languages differ only by case, such as `.C` (C++) and `.c` (C).
    #[test]
    fn name_is_matched_case_sensitively() {
        assert_no_language(&["a.RS", "DOCKERFILE", "DOCKERFILE.ci", "config.toml.EXAMPLE"]);
    }

    #[test]
    fn unknown_name_has_no_language() {
        assert_no_language(&["a.unknown", "README", ".hidden", "one-two-three"]);
    }

    /// Every extension and whole file name in [`NAMES`], as a file name that uses it.
    #[test]
    fn every_listed_name_resolves_to_its_language() {
        assert_resolves(&[
            // <block name="language-name-rows">
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
            ("a.bash", Bash),
            (".bash_profile", Bash),
            (".bashrc", Bash),
            ("MODULE.bazel", Starlark),
            ("a.builder", Ruby),
            ("a.bzl", Starlark),
            ("WORKSPACE.bzlmod", Starlark),
            ("a.c", C),
            ("a.cc", Cpp),
            ("CITATION.cff", Yaml),
            ("a.cjs", JavaScript),
            ("a.cmake", CMake),
            ("a.containerfile", Dockerfile),
            ("a.cpp", Cpp),
            ("a.cs", CSharp),
            ("a.csproj", Xml),
            ("a.css", Css),
            ("a.cts", TypeScript),
            ("a.cxx", Cpp),
            ("a.d.ts", TypeScript),
            ("a.dart", Dart),
            ("a.dockerfile", Dockerfile),
            (".envrc", Bash),
            ("a.ex", Elixir),
            ("a.exs", Elixir),
            ("a.fsproj", Xml),
            ("a.gemspec", Ruby),
            ("a.go", Go),
            ("go.mod", GoModule),
            ("go.sum", GoModule),
            ("go.work", GoModule),
            ("a.gql", GraphQl),
            ("build.gradle", Groovy),
            ("a.graphql", GraphQl),
            ("a.graphqls", GraphQl),
            ("a.groovy", Groovy),
            ("a.h", Cpp),
            ("a.hcl", Hcl),
            ("a.hh", Cpp),
            ("a.hpp", Cpp),
            ("a.htm", Html),
            ("a.html", Html),
            ("a.hxx", Cpp),
            ("a.java", Java),
            ("a.jbuilder", Ruby),
            ("a.jenkinsfile", Groovy),
            ("a.js", JavaScript),
            ("a.json", Json),
            ("a.jsonc", Json),
            ("a.jsx", JavaScript),
            ("a.kt", Kotlin),
            ("a.kts", Kotlin),
            ("a.lean", Lean),
            ("a.lua", Lua),
            ("a.mak", Makefile),
            ("a.makefile", Makefile),
            ("a.markdown", Markdown),
            ("a.md", Markdown),
            ("a.mdx", Mdx),
            ("a.mjs", JavaScript),
            ("a.mk", Makefile),
            ("a.ml", OCaml),
            ("a.mli", OCaml),
            ("a.mts", TypeScript),
            ("a.nix", Nix),
            ("a.nomad", Hcl),
            ("a.php", Php),
            ("a.phtml", Php),
            ("a.podspec", Ruby),
            ("a.props", Xml),
            ("a.proto", Proto),
            ("a.py", Python),
            ("a.pyi", Python),
            ("a.pyw", Python),
            ("a.rake", Ruby),
            ("a.rb", Ruby),
            ("a.rockspec", Lua),
            ("main.rs", Rust),
            ("config.ru", Ruby),
            ("build.sbt", Scala),
            ("a.scala", Scala),
            ("a.sh", Bash),
            ("a.sql", Sql),
            ("a.star", Starlark),
            ("a.swift", Swift),
            ("a.targets", Xml),
            ("a.tf", Hcl),
            ("a.tfvars", Hcl),
            ("a.tofu", Hcl),
            ("a.toml", Toml),
            ("a.ts", TypeScript),
            ("a.tsx", Tsx),
            ("a.vbproj", Xml),
            ("a.vcxproj", Xml),
            ("site.webmanifest", Json),
            ("a.xaml", Xml),
            ("a.xhtml", Html),
            ("a.xml", Xml),
            ("a.xsd", Xml),
            ("a.xsl", Xml),
            ("a.xslt", Xml),
            ("a.yaml", Yaml),
            ("a.yml", Yaml),
            ("a.zsh", Zsh),
            (".zshrc", Zsh),
            // </block>
        ]);
    }
}
