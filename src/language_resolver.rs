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
///
/// Most names are borrowed from GitHub Linguist, MIT license, at commit `d456527f`:
/// <https://github.com/github-linguist/linguist/blob/d456527f/lib/linguist/languages.yml>.
/// A name is left out when Linguist gives it to several languages, such as `.inc`. Such a name
/// doesn't tell which parser to use.
pub(crate) const NAMES: &[(&str, Language)] = {
    use Language::*;
    &[
        /* <block name="language-names"
        affects="src/language_resolver.rs:language-name-rows"
        keep-sorted keep-unique='\("(?P<value>[^"]+)"' same-as-pattern='\("(?P<value>[^"]+)"'> */
        (".all-contributorsrc", Json),
        (".arcconfig", Json),
        (".auto-changelog", Json),
        (".autorc", Json),
        (".babelrc", Json),
        (".bash_aliases", Bash),
        (".bash_functions", Bash),
        (".bash_logout", Bash),
        (".c8rc", Json),
        (".clang-format", Yaml),
        (".clang-tidy", Yaml),
        (".clangd", Yaml),
        (".classpath", Xml),
        (".flaskenv", Bash),
        (".gclient", Python),
        (".gemrc", Yaml),
        (".htmlhintrc", Json),
        (".imgbotconfig", Json),
        (".irbrc", Ruby),
        (".jscsrc", Json),
        (".jshintrc", Json),
        (".jslintrc", Json),
        (".kshrc", Bash),
        (".luacheckrc", Lua),
        (".nycrc", Json),
        (".php_cs", Php),
        (".php_cs.dist", Php),
        (".profile", Bash),
        (".project", Xml),
        (".pryrc", Ruby),
        (".secrets.baseline", Json),
        (".simplecov", Ruby),
        (".swcrc", Json),
        (".tern-config", Json),
        (".tern-project", Json),
        (".watchmanconfig", Json),
        (".xinitrc", Bash),
        (".xsession", Bash),
        (".zlogin", Zsh),
        (".zlogout", Zsh),
        (".zprofile", Zsh),
        (".zshenv", Zsh),
        ("4DForm", Json),
        ("4DProject", Json),
        ("App.config", Xml),
        ("Appraisals", Ruby),
        ("BSDmakefile", Makefile),
        ("BUCK", Starlark),
        ("BUILD", Starlark),
        ("Berksfile", Ruby),
        ("Brewfile", Ruby),
        ("Buildfile", Ruby),
        ("CMakeLists.txt", CMake),
        ("Capfile", Ruby),
        ("Cargo.lock", Toml),
        ("Cargo.toml.orig", Toml),
        ("Containerfile", Dockerfile),
        ("DEPS", Python),
        ("Dangerfile", Ruby),
        ("Deliverfile", Ruby),
        ("Dockerfile", Dockerfile),
        ("Fastfile", Ruby),
        ("GNUmakefile", Makefile),
        ("Gemfile", Ruby),
        ("Gopkg.lock", Toml),
        ("Guardfile", Ruby),
        ("JSON-tmLanguage", Json),
        ("Jakefile", JavaScript),
        ("Jarfile", Ruby),
        ("Jenkinsfile", Groovy),
        ("Kbuild", Makefile),
        ("MODULE.bazel.lock", Json),
        ("Makefile", Makefile),
        ("Mavenfile", Ruby),
        ("NuGet.config", Xml),
        ("PKGBUILD", Bash),
        ("Package.resolved", Json),
        ("Phakefile", Php),
        ("Pipfile", Toml),
        ("Pipfile.lock", Json),
        ("Podfile", Ruby),
        ("Puppetfile", Ruby),
        ("Rakefile", Ruby),
        ("SConscript", Python),
        ("SConstruct", Python),
        ("Settings.StyleCop", Xml),
        ("Snapfile", Ruby),
        ("Steepfile", Ruby),
        ("Thorfile", Ruby),
        ("Tiltfile", Starlark),
        ("Vagrantfile", Ruby),
        ("WORKSPACE", Starlark),
        ("Web.Debug.config", Xml),
        ("Web.Release.config", Xml),
        ("Web.config", Xml),
        ("_js", JavaScript),
        ("adml", Xml),
        ("admx", Xml),
        ("avsc", Json),
        ("aw", Php),
        ("axaml", Xml),
        ("axml", Xml),
        ("bash", Bash),
        ("bash_profile", Bash),
        ("bashrc", Bash),
        ("bats", Bash),
        ("bazel", Starlark),
        ("bones", JavaScript),
        ("builder", Ruby),
        ("buildfile", Ruby),
        ("bzl", Starlark),
        ("bzlmod", Starlark),
        ("c", C),
        ("c++", Cpp),
        ("cats", C),
        ("cc", Cpp),
        ("ccproj", Xml),
        ("ccxml", Xml),
        ("cff", Yaml),
        ("cjs", JavaScript),
        ("clixml", Xml),
        ("cmake", CMake),
        ("code-snippets", Json),
        ("code-workspace", Json),
        ("composer.lock", Json),
        ("containerfile", Dockerfile),
        ("contents.lr", Markdown),
        ("cpp", Cpp),
        ("cppm", Cpp),
        ("cproject", Xml),
        ("cs", CSharp),
        ("cs.pp", CSharp),
        ("cscfg", Xml),
        ("csdef", Xml),
        ("csproj", Xml),
        ("css", Css),
        ("csx", CSharp),
        ("ct", Xml),
        ("ctp", Php),
        ("cts", TypeScript),
        ("cxx", Cpp),
        ("d.ts", TypeScript),
        ("dart", Dart),
        ("deno.lock", Json),
        ("depproj", Xml),
        ("dita", Xml),
        ("ditamap", Xml),
        ("ditaval", Xml),
        ("dll.config", Xml),
        ("dockerfile", Dockerfile),
        ("dotsettings", Xml),
        ("eliom", OCaml),
        ("eliomi", OCaml),
        ("envrc", Bash),
        ("es6", JavaScript),
        ("ex", Elixir),
        ("exs", Elixir),
        ("eye", Ruby),
        ("flake.lock", Json),
        ("fsproj", Xml),
        ("fxml", Xml),
        ("gemspec", Ruby),
        ("glade", Xml),
        ("glide.lock", Yaml),
        ("gmx", Xml),
        ("go", Go),
        ("go.mod", GoModule),
        ("go.sum", GoModule),
        ("go.work", GoModule),
        ("go.work.sum", GoModule),
        ("god", Ruby),
        ("gql", GraphQl),
        ("gradle", Groovy),
        ("gradlew", Bash),
        ("graphql", GraphQl),
        ("graphqls", GraphQl),
        ("groovy", Groovy),
        ("grt", Groovy),
        ("grxml", Xml),
        ("gtpl", Groovy),
        ("gvy", Groovy),
        ("gyp", Python),
        ("gypi", Python),
        ("h", Cpp),
        ("h++", Cpp),
        ("hcl", Hcl),
        ("hh", Cpp),
        ("hlean", Lean),
        ("hpp", Cpp),
        ("hta", Html),
        ("htm", Html),
        ("html", Html),
        ("html.hl", Html),
        ("hujson", Json),
        ("hxx", Cpp),
        ("hzp", Xml),
        ("icls", Xml),
        ("idc", C),
        ("iml", Xml),
        ("inl", Cpp),
        ("ino", Cpp),
        ("ipp", Cpp),
        ("ivy", Xml),
        ("ixx", Cpp),
        ("jake", JavaScript),
        ("jav", Java),
        ("java", Java),
        ("javascript", JavaScript),
        ("jbuilder", Ruby),
        ("jelly", Xml),
        ("jenkinsfile", Groovy),
        ("js", JavaScript),
        ("jsb", JavaScript),
        ("jscad", JavaScript),
        ("jsfl", JavaScript),
        ("jsh", Java),
        ("jslib", JavaScript),
        ("jsm", JavaScript),
        ("json", Json),
        ("json5", Json),
        ("jsonc", Json),
        ("jspre", JavaScript),
        ("jsproj", Xml),
        ("jss", JavaScript),
        ("jsx", JavaScript),
        ("kojo", Scala),
        ("ksh", Bash),
        ("kt", Kotlin),
        ("ktm", Kotlin),
        ("kts", Kotlin),
        ("lean", Lean),
        ("linq", CSharp),
        ("livemd", Markdown),
        ("lmi", Python),
        ("lua", Lua),
        ("mak", Makefile),
        ("makefile", Makefile),
        ("markdown", Markdown),
        ("mcmeta", Json),
        ("mcmod.info", Json),
        ("md", Markdown),
        ("mdown", Markdown),
        ("mdpolicy", Xml),
        ("mdwn", Markdown),
        ("mdx", Mdx),
        ("meta4", Xml),
        ("mir", Yaml),
        ("mise.local.lock", Toml),
        ("mise.lock", Toml),
        ("mix.lock", Elixir),
        ("mjml", Xml),
        ("mjs", JavaScript),
        ("mk", Makefile),
        ("mkd", Markdown),
        ("mkdn", Markdown),
        ("mkdown", Markdown),
        ("mkfile", Makefile),
        ("ml", OCaml),
        ("ml4", OCaml),
        ("mli", OCaml),
        ("mspec", Ruby),
        ("mts", TypeScript),
        ("mvnw", Bash),
        ("mxml", Xml),
        ("mysql", Sql),
        ("natvis", Xml),
        ("ndproj", Xml),
        ("nix", Nix),
        ("njs", JavaScript),
        ("nomad", Hcl),
        ("nproj", Xml),
        ("nse", Lua),
        ("nuspec", Xml),
        ("odd", Xml),
        ("p8", Lua),
        ("pac", JavaScript),
        ("packages.config", Xml),
        ("pacscript", Bash),
        ("pd_lua", Lua),
        ("pdm.lock", Toml),
        ("php", Php),
        ("php3", Php),
        ("php4", Php),
        ("php5", Php),
        ("phps", Php),
        ("phpt", Php),
        ("phtml", Php),
        ("pixi.lock", Yaml),
        ("pkgproj", Xml),
        ("podspec", Ruby),
        ("poetry.lock", Toml),
        ("prawn", Ruby),
        ("props", Xml),
        ("proto", Proto),
        ("ps1xml", Xml),
        ("psc1", Xml),
        ("pubxml", Xml),
        ("py", Python),
        ("py3", Python),
        ("pyde", Python),
        ("pyi", Python),
        ("pyp", Python),
        ("pyt", Python),
        ("pyw", Python),
        ("qhelp", Xml),
        ("rabl", Ruby),
        ("rake", Ruby),
        ("rb", Ruby),
        ("rbi", Ruby),
        ("rbuild", Ruby),
        ("rbw", Ruby),
        ("rbx", Ruby),
        ("rbxmx", Xml),
        ("rbxs", Lua),
        ("rdf", Xml),
        ("reek", Yaml),
        ("resx", Xml),
        ("rockspec", Lua),
        ("ronn", Markdown),
        ("rs", Rust),
        ("rss", Xml),
        ("ru", Ruby),
        ("ruby", Ruby),
        ("rviz", Yaml),
        ("sbatch", Bash),
        ("sbt", Scala),
        ("scala", Scala),
        ("scxml", Xml),
        ("sfproj", Xml),
        ("sh", Bash),
        ("shproj", Xml),
        ("sjs", JavaScript),
        ("slnlaunch", Json),
        ("slnx", Xml),
        ("slurm", Bash),
        ("sql", Sql),
        ("srdf", Xml),
        ("ssjs", JavaScript),
        ("stTheme", Xml),
        ("star", Starlark),
        ("storyboard", Xml),
        ("sublime-build", Json),
        ("sublime-color-scheme", Json),
        ("sublime-commands", Json),
        ("sublime-completions", Json),
        ("sublime-keymap", Json),
        ("sublime-macro", Json),
        ("sublime-menu", Json),
        ("sublime-mousemap", Json),
        ("sublime-project", Json),
        ("sublime-settings", Json),
        ("sublime-snippet", Xml),
        ("sublime-syntax", Yaml),
        ("sublime-theme", Json),
        ("sublime-workspace", Json),
        ("sublime_metrics", Json),
        ("sublime_session", Json),
        ("swift", Swift),
        ("tac", Python),
        ("targets", Xml),
        ("tcc", Cpp),
        ("tf", Hcl),
        ("tfvars", Hcl),
        ("thor", Ruby),
        ("tmCommand", Xml),
        ("tmLanguage", Xml),
        ("tmPreferences", Xml),
        ("tmSnippet", Xml),
        ("tmTheme", Xml),
        ("tml", Xml),
        ("tofu", Hcl),
        ("toml", Toml),
        ("tpp", Cpp),
        ("ts", TypeScript),
        ("tsx", Tsx),
        ("txx", Cpp),
        ("udf", Sql),
        ("urdf", Xml),
        ("uv.lock", Toml),
        ("ux", Xml),
        ("vbproj", Xml),
        ("vcxproj", Xml),
        ("viw", Sql),
        ("vpy", Python),
        ("vsixmanifest", Xml),
        ("vssettings", Xml),
        ("vstemplate", Xml),
        ("vxml", Xml),
        ("watchr", Ruby),
        ("webapp", Json),
        ("webmanifest", Json),
        ("wixproj", Xml),
        ("wlua", Lua),
        ("wscript", Python),
        ("wsdl", Xml),
        ("wsf", Xml),
        ("wsgi", Python),
        ("wxi", Xml),
        ("wxl", Xml),
        ("wxs", Xml),
        ("x3d", Xml),
        ("xacro", Xml),
        ("xaml", Xml),
        ("xht", Html),
        ("xhtml", Html),
        ("xib", Xml),
        ("xlf", Xml),
        ("xliff", Xml),
        ("xmi", Xml),
        ("xml", Xml),
        ("xproj", Xml),
        ("xpy", Python),
        ("xsd", Xml),
        ("xsjs", JavaScript),
        ("xsjslib", JavaScript),
        ("xsl", Xml),
        ("xslt", Xml),
        ("xspec", Xml),
        ("xul", Xml),
        ("yaml", Yaml),
        ("yaml-tmlanguage", Yaml),
        ("yml", Yaml),
        ("yyp", Json),
        ("zcml", Xml),
        ("zsh", Zsh),
        ("zsh-theme", Zsh),
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
    /// Maps an extension or a whole file name to a language. For example, `cu` to
    /// `Language::Cpp` makes `.cu` files parse as C++.
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
            &[("cu", Cpp)],
            &[("a.rs.cu", Some(Cpp)), ("a.cu.rs", Some(Rust))],
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
        assert_resolves_with(&[("cu", Cpp)], &[("a.cu.in", Some(Cpp))]);
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
        assert_resolves_with(&[("cu", Cpp)], &[("a.cu", Some(Cpp))]);
        assert_resolves_with(&[("x.y", Rust)], &[("a.x.y", Some(Rust))]);
        assert_resolves_with(
            &[("Earthfile", Dockerfile)],
            &[("Earthfile", Some(Dockerfile))],
        );
        assert_resolves_with(&[("h", C)], &[("a.h", Some(C))]);
        assert_resolves_with(&[("Dockerfile", Bash)], &[("Dockerfile", Some(Bash))]);
    }

    /// A mapping of a name covers that name only, not its variants. To map a variant, map its
    /// whole name.
    #[test]
    fn extension_mapping_does_not_apply_to_variants() {
        assert_resolves_with(
            &[("Earthfile", Dockerfile)],
            &[("Earthfile.ci", None), ("Earthfile-dev", None)],
        );
        assert_resolves_with(
            &[("Dockerfile", Bash)],
            &[("Dockerfile.ci", Some(Dockerfile))],
        );
        assert_resolves_with(
            &[("Earthfile.ci", Dockerfile)],
            &[("Earthfile.ci", Some(Dockerfile))],
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

    /// blockwatch picks a language by the name alone, so a name that several languages use, by
    /// GitHub Linguist's list, has none.
    #[test]
    fn name_shared_by_several_languages_has_no_language() {
        assert_no_language(&[
            "a.inc",
            "a.spec",
            "a.fcgi",
            "a.cgi",
            "a.d",
            "a.man",
            ".eslintrc",
        ]);
    }

    /// Some names that Linguist gives one language are left out: a syntax the parser can't read,
    /// machine-written data, generic extensions, and bare twins of dotfiles, which would also
    /// match as extensions.
    #[test]
    fn name_left_out_on_purpose_has_no_language() {
        assert_no_language(&[
            // Yarn 1 doesn't write YAML.
            "yarn.lock",
            // ocamllex and ocamlyacc grammars are not OCaml.
            "lexer.mll",
            "parser.mly",
            // csh and tmux are not shell.
            ".cshrc",
            ".tmux.conf",
            // Machine-written data can be large, or binary, as a PyTorch `model.pt` is.
            "data.jsonl",
            "trace.har",
            "report.sarif",
            "model.gltf",
            "map.osm",
            "a.geojson",
            "a.topojson",
            "terraform.tfstate",
            "terraform.tfstate.backup",
            "places.kml",
            "track.gpx",
            "data.tab",
            "photo.xmp",
            ".bash_history",
            "bun.lock",
            "model.pt",
            // An extension this generic would also give a language to a file with no extension,
            // such as a script named `tool`.
            "tool",
            "command",
            "make",
            "ui",
            "launch",
            "filters",
            "builds",
            "ant",
            "proj",
            "syntax",
            "workbook",
            // `profile` would make `nginx.profile` a shell script.
            "profile",
            "nginx.profile",
        ]);
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
            (".all-contributorsrc", Json),
            (".arcconfig", Json),
            (".auto-changelog", Json),
            (".autorc", Json),
            (".babelrc", Json),
            (".bash_aliases", Bash),
            (".bash_functions", Bash),
            (".bash_logout", Bash),
            (".c8rc", Json),
            (".clang-format", Yaml),
            (".clang-tidy", Yaml),
            (".clangd", Yaml),
            (".classpath", Xml),
            (".flaskenv", Bash),
            (".gclient", Python),
            (".gemrc", Yaml),
            (".htmlhintrc", Json),
            (".imgbotconfig", Json),
            (".irbrc", Ruby),
            (".jscsrc", Json),
            (".jshintrc", Json),
            (".jslintrc", Json),
            (".kshrc", Bash),
            (".luacheckrc", Lua),
            (".nycrc", Json),
            (".php_cs", Php),
            (".php_cs.dist", Php),
            (".profile", Bash),
            (".project", Xml),
            (".pryrc", Ruby),
            (".secrets.baseline", Json),
            (".simplecov", Ruby),
            (".swcrc", Json),
            (".tern-config", Json),
            (".tern-project", Json),
            (".watchmanconfig", Json),
            (".xinitrc", Bash),
            (".xsession", Bash),
            (".zlogin", Zsh),
            (".zlogout", Zsh),
            (".zprofile", Zsh),
            (".zshenv", Zsh),
            ("a.4DForm", Json),
            ("a.4DProject", Json),
            ("App.config", Xml),
            ("Appraisals", Ruby),
            ("BSDmakefile", Makefile),
            ("BUCK", Starlark),
            ("BUILD", Starlark),
            ("Berksfile", Ruby),
            ("Brewfile", Ruby),
            ("Buildfile", Ruby),
            ("CMakeLists.txt", CMake),
            ("Capfile", Ruby),
            ("Cargo.lock", Toml),
            ("Cargo.toml.orig", Toml),
            ("Containerfile", Dockerfile),
            ("DEPS", Python),
            ("Dangerfile", Ruby),
            ("Deliverfile", Ruby),
            ("Dockerfile", Dockerfile),
            ("Fastfile", Ruby),
            ("GNUmakefile", Makefile),
            ("Gemfile", Ruby),
            ("Gopkg.lock", Toml),
            ("Guardfile", Ruby),
            ("a.JSON-tmLanguage", Json),
            ("Jakefile", JavaScript),
            ("Jarfile", Ruby),
            ("Jenkinsfile", Groovy),
            ("Kbuild", Makefile),
            ("MODULE.bazel.lock", Json),
            ("Makefile", Makefile),
            ("Mavenfile", Ruby),
            ("NuGet.config", Xml),
            ("PKGBUILD", Bash),
            ("Package.resolved", Json),
            ("Phakefile", Php),
            ("Pipfile", Toml),
            ("Pipfile.lock", Json),
            ("Podfile", Ruby),
            ("Puppetfile", Ruby),
            ("Rakefile", Ruby),
            ("SConscript", Python),
            ("SConstruct", Python),
            ("Settings.StyleCop", Xml),
            ("Snapfile", Ruby),
            ("Steepfile", Ruby),
            ("Thorfile", Ruby),
            ("Tiltfile", Starlark),
            ("Vagrantfile", Ruby),
            ("WORKSPACE", Starlark),
            ("Web.Debug.config", Xml),
            ("Web.Release.config", Xml),
            ("Web.config", Xml),
            ("a._js", JavaScript),
            ("a.adml", Xml),
            ("a.admx", Xml),
            ("a.avsc", Json),
            ("a.aw", Php),
            ("a.axaml", Xml),
            ("a.axml", Xml),
            ("a.bash", Bash),
            (".bash_profile", Bash),
            (".bashrc", Bash),
            ("a.bats", Bash),
            ("MODULE.bazel", Starlark),
            ("a.bones", JavaScript),
            ("a.builder", Ruby),
            ("buildfile", Ruby),
            ("a.bzl", Starlark),
            ("WORKSPACE.bzlmod", Starlark),
            ("a.c", C),
            ("a.c++", Cpp),
            ("a.cats", C),
            ("a.cc", Cpp),
            ("a.ccproj", Xml),
            ("a.ccxml", Xml),
            ("CITATION.cff", Yaml),
            ("a.cjs", JavaScript),
            ("a.clixml", Xml),
            ("a.cmake", CMake),
            ("a.code-snippets", Json),
            ("a.code-workspace", Json),
            ("composer.lock", Json),
            ("a.containerfile", Dockerfile),
            ("contents.lr", Markdown),
            ("a.cpp", Cpp),
            ("a.cppm", Cpp),
            ("a.cproject", Xml),
            ("a.cs", CSharp),
            ("a.cs.pp", CSharp),
            ("a.cscfg", Xml),
            ("a.csdef", Xml),
            ("a.csproj", Xml),
            ("a.css", Css),
            ("a.csx", CSharp),
            ("a.ct", Xml),
            ("a.ctp", Php),
            ("a.cts", TypeScript),
            ("a.cxx", Cpp),
            ("a.d.ts", TypeScript),
            ("a.dart", Dart),
            ("deno.lock", Json),
            ("a.depproj", Xml),
            ("a.dita", Xml),
            ("a.ditamap", Xml),
            ("a.ditaval", Xml),
            ("a.dll.config", Xml),
            ("a.dockerfile", Dockerfile),
            ("a.dotsettings", Xml),
            ("a.eliom", OCaml),
            ("a.eliomi", OCaml),
            (".envrc", Bash),
            ("a.es6", JavaScript),
            ("a.ex", Elixir),
            ("a.exs", Elixir),
            ("a.eye", Ruby),
            ("flake.lock", Json),
            ("a.fsproj", Xml),
            ("a.fxml", Xml),
            ("a.gemspec", Ruby),
            ("a.glade", Xml),
            ("glide.lock", Yaml),
            ("a.gmx", Xml),
            ("a.go", Go),
            ("go.mod", GoModule),
            ("go.sum", GoModule),
            ("go.work", GoModule),
            ("go.work.sum", GoModule),
            ("a.god", Ruby),
            ("a.gql", GraphQl),
            ("build.gradle", Groovy),
            ("gradlew", Bash),
            ("a.graphql", GraphQl),
            ("a.graphqls", GraphQl),
            ("a.groovy", Groovy),
            ("a.grt", Groovy),
            ("a.grxml", Xml),
            ("a.gtpl", Groovy),
            ("a.gvy", Groovy),
            ("a.gyp", Python),
            ("a.gypi", Python),
            ("a.h", Cpp),
            ("a.h++", Cpp),
            ("a.hcl", Hcl),
            ("a.hh", Cpp),
            ("a.hlean", Lean),
            ("a.hpp", Cpp),
            ("a.hta", Html),
            ("a.htm", Html),
            ("a.html", Html),
            ("a.html.hl", Html),
            ("a.hujson", Json),
            ("a.hxx", Cpp),
            ("a.hzp", Xml),
            ("a.icls", Xml),
            ("a.idc", C),
            ("a.iml", Xml),
            ("a.inl", Cpp),
            ("a.ino", Cpp),
            ("a.ipp", Cpp),
            ("a.ivy", Xml),
            ("a.ixx", Cpp),
            ("a.jake", JavaScript),
            ("a.jav", Java),
            ("a.java", Java),
            ("a.javascript", JavaScript),
            ("a.jbuilder", Ruby),
            ("a.jelly", Xml),
            ("a.jenkinsfile", Groovy),
            ("a.js", JavaScript),
            ("a.jsb", JavaScript),
            ("a.jscad", JavaScript),
            ("a.jsfl", JavaScript),
            ("a.jsh", Java),
            ("a.jslib", JavaScript),
            ("a.jsm", JavaScript),
            ("a.json", Json),
            ("a.json5", Json),
            ("a.jsonc", Json),
            ("a.jspre", JavaScript),
            ("a.jsproj", Xml),
            ("a.jss", JavaScript),
            ("a.jsx", JavaScript),
            ("a.kojo", Scala),
            ("a.ksh", Bash),
            ("a.kt", Kotlin),
            ("a.ktm", Kotlin),
            ("a.kts", Kotlin),
            ("a.lean", Lean),
            ("a.linq", CSharp),
            ("a.livemd", Markdown),
            ("a.lmi", Python),
            ("a.lua", Lua),
            ("a.mak", Makefile),
            ("a.makefile", Makefile),
            ("a.markdown", Markdown),
            ("a.mcmeta", Json),
            ("mcmod.info", Json),
            ("a.md", Markdown),
            ("a.mdown", Markdown),
            ("a.mdpolicy", Xml),
            ("a.mdwn", Markdown),
            ("a.mdx", Mdx),
            ("a.meta4", Xml),
            ("a.mir", Yaml),
            ("mise.local.lock", Toml),
            ("mise.lock", Toml),
            ("mix.lock", Elixir),
            ("a.mjml", Xml),
            ("a.mjs", JavaScript),
            ("a.mk", Makefile),
            ("a.mkd", Markdown),
            ("a.mkdn", Markdown),
            ("a.mkdown", Markdown),
            ("mkfile", Makefile),
            ("a.ml", OCaml),
            ("a.ml4", OCaml),
            ("a.mli", OCaml),
            ("a.mspec", Ruby),
            ("a.mts", TypeScript),
            ("mvnw", Bash),
            ("a.mxml", Xml),
            ("a.mysql", Sql),
            ("a.natvis", Xml),
            ("a.ndproj", Xml),
            ("a.nix", Nix),
            ("a.njs", JavaScript),
            ("a.nomad", Hcl),
            ("a.nproj", Xml),
            ("a.nse", Lua),
            ("a.nuspec", Xml),
            ("a.odd", Xml),
            ("a.p8", Lua),
            ("a.pac", JavaScript),
            ("packages.config", Xml),
            ("a.pacscript", Bash),
            ("a.pd_lua", Lua),
            ("pdm.lock", Toml),
            ("a.php", Php),
            ("a.php3", Php),
            ("a.php4", Php),
            ("a.php5", Php),
            ("a.phps", Php),
            ("a.phpt", Php),
            ("a.phtml", Php),
            ("pixi.lock", Yaml),
            ("a.pkgproj", Xml),
            ("a.podspec", Ruby),
            ("poetry.lock", Toml),
            ("a.prawn", Ruby),
            ("a.props", Xml),
            ("a.proto", Proto),
            ("a.ps1xml", Xml),
            ("a.psc1", Xml),
            ("a.pubxml", Xml),
            ("a.py", Python),
            ("a.py3", Python),
            ("a.pyde", Python),
            ("a.pyi", Python),
            ("a.pyp", Python),
            ("a.pyt", Python),
            ("a.pyw", Python),
            ("a.qhelp", Xml),
            ("a.rabl", Ruby),
            ("a.rake", Ruby),
            ("a.rb", Ruby),
            ("a.rbi", Ruby),
            ("a.rbuild", Ruby),
            ("a.rbw", Ruby),
            ("a.rbx", Ruby),
            ("a.rbxmx", Xml),
            ("a.rbxs", Lua),
            ("a.rdf", Xml),
            ("a.reek", Yaml),
            ("a.resx", Xml),
            ("a.rockspec", Lua),
            ("a.ronn", Markdown),
            ("main.rs", Rust),
            ("a.rss", Xml),
            ("config.ru", Ruby),
            ("a.ruby", Ruby),
            ("a.rviz", Yaml),
            ("a.sbatch", Bash),
            ("build.sbt", Scala),
            ("a.scala", Scala),
            ("a.scxml", Xml),
            ("a.sfproj", Xml),
            ("a.sh", Bash),
            ("a.shproj", Xml),
            ("a.sjs", JavaScript),
            ("a.slnlaunch", Json),
            ("a.slnx", Xml),
            ("a.slurm", Bash),
            ("a.sql", Sql),
            ("a.srdf", Xml),
            ("a.ssjs", JavaScript),
            ("a.stTheme", Xml),
            ("a.star", Starlark),
            ("a.storyboard", Xml),
            ("a.sublime-build", Json),
            ("a.sublime-color-scheme", Json),
            ("a.sublime-commands", Json),
            ("a.sublime-completions", Json),
            ("a.sublime-keymap", Json),
            ("a.sublime-macro", Json),
            ("a.sublime-menu", Json),
            ("a.sublime-mousemap", Json),
            ("a.sublime-project", Json),
            ("a.sublime-settings", Json),
            ("a.sublime-snippet", Xml),
            ("a.sublime-syntax", Yaml),
            ("a.sublime-theme", Json),
            ("a.sublime-workspace", Json),
            ("a.sublime_metrics", Json),
            ("a.sublime_session", Json),
            ("a.swift", Swift),
            ("a.tac", Python),
            ("a.targets", Xml),
            ("a.tcc", Cpp),
            ("a.tf", Hcl),
            ("a.tfvars", Hcl),
            ("a.thor", Ruby),
            ("a.tmCommand", Xml),
            ("a.tmLanguage", Xml),
            ("a.tmPreferences", Xml),
            ("a.tmSnippet", Xml),
            ("a.tmTheme", Xml),
            ("a.tml", Xml),
            ("a.tofu", Hcl),
            ("a.toml", Toml),
            ("a.tpp", Cpp),
            ("a.ts", TypeScript),
            ("a.tsx", Tsx),
            ("a.txx", Cpp),
            ("a.udf", Sql),
            ("a.urdf", Xml),
            ("uv.lock", Toml),
            ("a.ux", Xml),
            ("a.vbproj", Xml),
            ("a.vcxproj", Xml),
            ("a.viw", Sql),
            ("a.vpy", Python),
            ("a.vsixmanifest", Xml),
            ("a.vssettings", Xml),
            ("a.vstemplate", Xml),
            ("a.vxml", Xml),
            ("a.watchr", Ruby),
            ("a.webapp", Json),
            ("site.webmanifest", Json),
            ("a.wixproj", Xml),
            ("a.wlua", Lua),
            ("wscript", Python),
            ("a.wsdl", Xml),
            ("a.wsf", Xml),
            ("a.wsgi", Python),
            ("a.wxi", Xml),
            ("a.wxl", Xml),
            ("a.wxs", Xml),
            ("a.x3d", Xml),
            ("a.xacro", Xml),
            ("a.xaml", Xml),
            ("a.xht", Html),
            ("a.xhtml", Html),
            ("a.xib", Xml),
            ("a.xlf", Xml),
            ("a.xliff", Xml),
            ("a.xmi", Xml),
            ("a.xml", Xml),
            ("a.xproj", Xml),
            ("a.xpy", Python),
            ("a.xsd", Xml),
            ("a.xsjs", JavaScript),
            ("a.xsjslib", JavaScript),
            ("a.xsl", Xml),
            ("a.xslt", Xml),
            ("a.xspec", Xml),
            ("a.xul", Xml),
            ("a.yaml", Yaml),
            ("a.yaml-tmlanguage", Yaml),
            ("a.yml", Yaml),
            ("a.yyp", Json),
            ("a.zcml", Xml),
            ("a.zsh", Zsh),
            ("a.zsh-theme", Zsh),
            (".zshrc", Zsh),
            // </block>
        ]);
    }
}
