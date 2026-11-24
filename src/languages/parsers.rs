//! The language parsers of a run, one for each language.

use super::file_names::LanguageResolver;
use super::{
    Language, LanguageParser, bash, c, c_sharp, cmake, cpp, css, dart, dockerfile, elixir, go,
    graphql, groovy, hcl, html, java, javascript, json, kotlin, lean, lua, makefile, markdown, mdx,
    nix, ocaml, php, proto, python, ruby, rust, scala, sql, starlark, swift, toml, tsx, typescript,
    xml, yaml,
};
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::Path;
use std::sync::Mutex;
use strum::IntoEnumIterator;

/// A parser for one language, shared between files and threads.
///
/// Each parser owns a mutable tree-sitter `Parser` and is therefore behind a `Mutex`.
pub(crate) type SharedLanguageParser = Mutex<Box<dyn LanguageParser>>;

/// The language parsers of a run, one for each [`Language`], and how a file name selects one.
pub(crate) struct LanguageParsers {
    resolver: LanguageResolver,
    /// The parser of each [`Language`], in the order the enum lists them. So the value of a
    /// language cast to `usize` is the index of its parser.
    parsers: Vec<SharedLanguageParser>,
}

impl LanguageParsers {
    /// Builds every language parser, with no extension mappings.
    ///
    /// # Panics
    /// Panics if the tree-sitter crate doesn't support a grammar's ABI version, or if the PHP
    /// grammar lacks the nodes its comments query reads. Either is a build bug.
    pub(crate) fn new() -> Self {
        Self {
            resolver: LanguageResolver::new(HashMap::new()),
            parsers: Language::iter()
                .map(|language| Mutex::new(new_parser(language)))
                .collect(),
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
        &self.parsers[language as usize]
    }
}

/// Builds the parser of `language`.
///
/// # Panics
/// Panics in the cases that [`LanguageParsers::new`] lists.
fn new_parser(language: Language) -> Box<dyn LanguageParser> {
    match language {
        Language::Bash => Box::new(bash::parser()),
        Language::C => Box::new(c::parser()),
        Language::CMake => Box::new(cmake::parser()),
        Language::CSharp => Box::new(c_sharp::parser()),
        Language::Cpp => Box::new(cpp::parser()),
        Language::Css => Box::new(css::parser()),
        Language::Dart => Box::new(dart::parser()),
        Language::Dockerfile => Box::new(dockerfile::parser()),
        Language::Elixir => Box::new(elixir::parser()),
        Language::Go => Box::new(go::parser()),
        Language::GoModule => Box::new(go::module_files_parser()),
        Language::GraphQl => Box::new(graphql::parser()),
        Language::Groovy => Box::new(groovy::parser()),
        Language::Hcl => Box::new(hcl::parser()),
        Language::Html => Box::new(html::parser()),
        Language::Java => Box::new(java::parser()),
        Language::JavaScript => Box::new(javascript::parser()),
        Language::Json => Box::new(json::parser()),
        Language::Kotlin => Box::new(kotlin::parser()),
        Language::Lean => Box::new(lean::parser()),
        Language::Lua => Box::new(lua::parser()),
        Language::Makefile => Box::new(makefile::parser()),
        Language::Markdown => Box::new(markdown::parser()),
        Language::Mdx => Box::new(mdx::parser()),
        Language::Nix => Box::new(nix::parser()),
        Language::OCaml => Box::new(ocaml::parser()),
        Language::Php => Box::new(php::parser()),
        Language::Proto => Box::new(proto::parser()),
        Language::Python => Box::new(python::parser()),
        Language::Ruby => Box::new(ruby::parser()),
        Language::Rust => Box::new(rust::parser()),
        Language::Scala => Box::new(scala::parser()),
        Language::Sql => Box::new(sql::parser()),
        Language::Starlark => Box::new(starlark::parser()),
        Language::Swift => Box::new(swift::parser()),
        Language::Toml => Box::new(toml::parser()),
        Language::Tsx => Box::new(tsx::parser()),
        Language::TypeScript => Box::new(typescript::parser()),
        Language::Xml => Box::new(xml::parser()),
        Language::Yaml => Box::new(yaml::parser()),
        Language::Zsh => Box::new(bash::zsh_parser()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::languages::file_names::NAMES;
    use crate::symbols::SymbolPath;

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
            let result = parsers
                .parser(language)
                .lock()
                .expect("a thread panicked while it held the parser")
                .parse_symbols("")
                .map(|_| ());
            let has_symbols = match result {
                Ok(()) => true,
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

#[cfg(test)]
mod supported_languages_tests {

    use std::path::Path;

    use super::LanguageParsers;

    // <block name="supported-extensions">
    #[test]
    fn all_language_extensions_are_supported() -> anyhow::Result<()> {
        let parsers = LanguageParsers::new();
        let files = [
            (
                "BUILD".to_string(),
                "# <block>\ncc_library(name = \"foo\")\n# </block>".to_string(),
            ),
            (
                "MODULE.bazel".to_string(),
                "# <block>\nmodule(name = \"m\")\n# </block>".to_string(),
            ),
            (
                "WORKSPACE".to_string(),
                "# <block>\nworkspace(name = \"w\")\n# </block>".to_string(),
            ),
            (
                "WORKSPACE.bzlmod".to_string(),
                "# <block>\n# migration stub\n# </block>".to_string(),
            ),
            (
                "CMakeLists.txt".to_string(),
                "# <block>\nadd_library(foo foo.c)\n# </block>".to_string(),
            ),
            (
                "cmake.cmake".to_string(),
                "#[[ <block> ]]\nset(X 1)\n# </block>".to_string(),
            ),
            (
                "bash.bash".to_string(),
                "# <block>\necho \"hello\"\n# </block>".to_string(),
            ),
            (
                ".bash_profile".to_string(),
                "# <block>\nsource ~/.bashrc\n# </block>".to_string(),
            ),
            (
                ".bashrc".to_string(),
                "# <block>\nexport EDITOR=vim\n# </block>".to_string(),
            ),
            (
                ".envrc".to_string(),
                "# <block>\nexport API_URL=http://localhost\n# </block>".to_string(),
            ),
            (
                "zsh.zsh".to_string(),
                "# <block>\necho \"hello\"\n# </block>".to_string(),
            ),
            (
                ".zshrc".to_string(),
                "# <block>\nexport PATH=\"$HOME/bin:$PATH\"\n# </block>".to_string(),
            ),
            (
                "bzl.bzl".to_string(),
                "# <block>\ndef my_macro():\n    pass\n# </block>".to_string(),
            ),
            (
                "c.c".to_string(),
                "/* <block> */\nint main() { return 0; }\n/* </block> */".to_string(),
            ),
            (
                "cc.cc".to_string(),
                "// <block>\nint main() { return 0; }\n// </block>".to_string(),
            ),
            (
                "cpp.cpp".to_string(),
                "// <block>\nint main() { return 0; }\n// </block>".to_string(),
            ),
            (
                "cxx.cxx".to_string(),
                "// <block>\nint main() { return 0; }\n// </block>".to_string(),
            ),
            (
                "hh.hh".to_string(),
                "// <block>\nvoid foo();\n// </block>".to_string(),
            ),
            (
                "hpp.hpp".to_string(),
                "// <block>\nclass Foo {};\n// </block>".to_string(),
            ),
            (
                "hxx.hxx".to_string(),
                "// <block>\nstruct Bar;\n// </block>".to_string(),
            ),
            (
                "cs.cs".to_string(),
                "// <block>\nclass Program { }\n// </block>".to_string(),
            ),
            (
                "css.css".to_string(),
                "/* <block> */\nbody { margin: 0; }\n/* </block> */".to_string(),
            ),
            (
                "Containerfile".to_string(),
                "# <block>\nFROM fedora\n# </block>".to_string(),
            ),
            (
                "containerfile".to_string(),
                "# <block>\nFROM centos\n# </block>".to_string(),
            ),
            (
                "Dockerfile".to_string(),
                "# <block>\nFROM alpine\n# </block>".to_string(),
            ),
            (
                "app.dockerfile".to_string(),
                "# <block>\nFROM debian\n# </block>".to_string(),
            ),
            (
                "dart.dart".to_string(),
                "// <block>\nvoid main() {}\n// </block>".to_string(),
            ),
            (
                "ex.ex".to_string(),
                "# <block>\ndefmodule Foo do\nend\n# </block>".to_string(),
            ),
            (
                "exs.exs".to_string(),
                "# <block>\nIO.puts(:hello)\n# </block>".to_string(),
            ),
            (
                "go.go".to_string(),
                "// <block>\nfunc main() {}\n// </block>".to_string(),
            ),
            (
                "go.mod".to_string(),
                "// <block>\nmodule example.com/m\n// </block>".to_string(),
            ),
            (
                "go.sum".to_string(),
                "// <block>\nexample.com/dep v1.0.0 h1:abc\n// </block>".to_string(),
            ),
            (
                "go.work".to_string(),
                "// <block>\nuse ./mod\n// </block>".to_string(),
            ),
            (
                "gql.gql".to_string(),
                "# <block>\ntype Mutation {\n  noop: Boolean\n}\n# </block>".to_string(),
            ),
            (
                "gradle.gradle".to_string(),
                "// <block>\nversion = '1.0'\n// </block>".to_string(),
            ),
            (
                "graphql.graphql".to_string(),
                "# <block>\ntype Query {\n  hello: String\n}\n# </block>".to_string(),
            ),
            (
                "graphqls.graphqls".to_string(),
                "# <block>\ntype Query {\n  ping: String\n}\n# </block>".to_string(),
            ),
            (
                "groovy.groovy".to_string(),
                "// <block>\ndef x = 1\n// </block>".to_string(),
            ),
            (
                "h.h".to_string(),
                "// <block>\nvoid foo();\n// </block>".to_string(),
            ),
            (
                "hcl.hcl".to_string(),
                "// <block>\nregion = \"eu-west-1\"\n// </block>".to_string(),
            ),
            (
                "tofu.tofu".to_string(),
                "# <block>\nregion = \"eu-central-1\"\n# </block>".to_string(),
            ),
            (
                "nomad.nomad".to_string(),
                "# <block>\ndatacenters = [\"dc1\"]\n# </block>".to_string(),
            ),
            (
                "htm.htm".to_string(),
                "<!-- <block> -->\n<div>Content</div>\n<!-- </block> -->".to_string(),
            ),
            (
                "html.html".to_string(),
                "<!-- <block> -->\n<p>Hello</p>\n<!-- </block> -->".to_string(),
            ),
            (
                "xhtml.xhtml".to_string(),
                "<!-- <block> -->\n<p>Hi</p>\n<!-- </block> -->".to_string(),
            ),
            (
                "java.java".to_string(),
                "// <block>\nclass App {}\n// </block>".to_string(),
            ),
            (
                "Jenkinsfile".to_string(),
                "// <block>\npipeline { }\n// </block>".to_string(),
            ),
            (
                "jenkinsfile".to_string(),
                "// <block>\nnode { }\n// </block>".to_string(),
            ),
            (
                "js.js".to_string(),
                "// <block>\nconst x = 1;\n// </block>".to_string(),
            ),
            (
                "cjs.cjs".to_string(),
                "// <block>\nmodule.exports = {};\n// </block>".to_string(),
            ),
            (
                "mjs.mjs".to_string(),
                "// <block>\nexport const x = 1;\n// </block>".to_string(),
            ),
            (
                "json.json".to_string(),
                "// <block>\n{\"key\": 1}\n// </block>".to_string(),
            ),
            (
                "jsonc.jsonc".to_string(),
                "// <block>\n{\"key\": 1}\n// </block>".to_string(),
            ),
            (
                "site.webmanifest".to_string(),
                "// <block>\n{\"name\": \"app\"}\n// </block>".to_string(),
            ),
            (
                "jsx.jsx".to_string(),
                "// <block>\nconst Comp = () => <div/>;\n// </block>".to_string(),
            ),
            (
                "kt.kt".to_string(),
                "// <block>\nfun main() {}\n// </block>".to_string(),
            ),
            (
                "kts.kts".to_string(),
                "// <block>\nplugins { }\n// </block>".to_string(),
            ),
            (
                "lean.lean".to_string(),
                "-- <block>\ndef x := 1\n-- </block>".to_string(),
            ),
            (
                "lua.lua".to_string(),
                "-- <block>\nlocal x = 1\n-- </block>".to_string(),
            ),
            (
                "app-1.0-1.rockspec".to_string(),
                "-- <block>\npackage = \"app\"\n-- </block>".to_string(),
            ),
            (
                "makefile".to_string(),
                "# <block>\nall:\n\t@echo \"hello\"\n# </block>".to_string(),
            ),
            (
                "Makefile".to_string(),
                "# <block>\nall:\n\t@echo \"hello\"\n# </block>".to_string(),
            ),
            (
                "markdown.markdown".to_string(),
                "<div>\n<!-- <block> -->\n# Title\n<!-- </block> -->\n</div>".to_string(),
            ),
            (
                "md.md".to_string(),
                "<div>\n<!-- <block> -->\n## Heading\n<!-- </block> -->\n</div>".to_string(),
            ),
            (
                "mdx.mdx".to_string(),
                "{/* <block> */}\n## Heading\n{/* </block> */}".to_string(),
            ),
            (
                "mk.mk".to_string(),
                "# <block>\nall:\n\t@echo \"hello\"\n# </block>".to_string(),
            ),
            (
                "GNUmakefile".to_string(),
                "# <block>\nall:\n\t@echo \"hello\"\n# </block>".to_string(),
            ),
            (
                "mak.mak".to_string(),
                "# <block>\nall:\n\t@echo \"hello\"\n# </block>".to_string(),
            ),
            (
                "nix.nix".to_string(),
                "# <block>\n{ pkgs = null; }\n# </block>".to_string(),
            ),
            (
                "ml.ml".to_string(),
                "(* <block> *)\nlet x = 1\n(* </block> *)".to_string(),
            ),
            (
                "mli.mli".to_string(),
                "(* <block> *)\nval x : int\n(* </block> *)".to_string(),
            ),
            (
                "php.php".to_string(),
                "<?php\n# <block>\necho 'hello';\n# </block>\n?>".to_string(),
            ),
            (
                "phtml.phtml".to_string(),
                "<?php\n# <block>\necho 'world';\n# </block>\n?>".to_string(),
            ),
            (
                "proto.proto".to_string(),
                "// <block>\nsyntax = \"proto3\";\n// </block>".to_string(),
            ),
            (
                "py.py".to_string(),
                "# <block>\ndef main():\n    pass\n# </block>".to_string(),
            ),
            (
                "pyi.pyi".to_string(),
                "# <block>\ndef foo() -> None: pass\n# </block>".to_string(),
            ),
            (
                "pyw.pyw".to_string(),
                "# <block>\nimport tkinter\n# </block>".to_string(),
            ),
            (
                "rb.rb".to_string(),
                "# <block>\ndef hello\n  puts 'world'\nend\n# </block>".to_string(),
            ),
            (
                "Brewfile".to_string(),
                "# <block>\nbrew 'git'\n# </block>".to_string(),
            ),
            (
                "Fastfile".to_string(),
                "# <block>\nlane :beta do\nend\n# </block>".to_string(),
            ),
            (
                "Gemfile".to_string(),
                "# <block>\ngem 'rails'\n# </block>".to_string(),
            ),
            (
                "Podfile".to_string(),
                "# <block>\npod 'Alamofire'\n# </block>".to_string(),
            ),
            (
                "Rakefile".to_string(),
                "# <block>\ntask :default\n# </block>".to_string(),
            ),
            (
                "Vagrantfile".to_string(),
                "# <block>\nVagrant.configure('2') do |config|\nend\n# </block>".to_string(),
            ),
            (
                "index.atom.builder".to_string(),
                "# <block>\nxml.feed\n# </block>".to_string(),
            ),
            (
                "app.gemspec".to_string(),
                "# <block>\nGem::Specification.new\n# </block>".to_string(),
            ),
            (
                "show.json.jbuilder".to_string(),
                "# <block>\njson.name @user.name\n# </block>".to_string(),
            ),
            (
                "app.podspec".to_string(),
                "# <block>\nPod::Spec.new\n# </block>".to_string(),
            ),
            (
                "rake.rake".to_string(),
                "# <block>\ntask :build\n# </block>".to_string(),
            ),
            (
                "config.ru".to_string(),
                "# <block>\nrun App\n# </block>".to_string(),
            ),
            (
                "rs.rs".to_string(),
                r"/* <block> */fn a() {}/* </block> */".to_string(),
            ),
            (
                "sbt.sbt".to_string(),
                "// <block>\nname := \"app\"\n// </block>".to_string(),
            ),
            (
                "scala.scala".to_string(),
                "// <block>\nval x = 1\n// </block>".to_string(),
            ),
            (
                "sh.sh".to_string(),
                "# <block>\necho \"hello\"\n# </block>".to_string(),
            ),
            (
                "sql.sql".to_string(),
                "-- <block>\nSELECT * FROM users;\n-- </block>".to_string(),
            ),
            (
                "star.star".to_string(),
                "# <block>\nx = 42\n# </block>".to_string(),
            ),
            (
                "BUCK".to_string(),
                "# <block>\ncxx_library(name = \"foo\")\n# </block>".to_string(),
            ),
            (
                "Tiltfile".to_string(),
                "# <block>\nk8s_yaml('app.yaml')\n# </block>".to_string(),
            ),
            (
                "swift.swift".to_string(),
                "// <block>\nfunc main() {}\n// </block>".to_string(),
            ),
            (
                "tf.tf".to_string(),
                "# <block>\nregion = \"us-east-1\"\n# </block>".to_string(),
            ),
            (
                "tfvars.tfvars".to_string(),
                "# <block>\nregion = \"us-west-2\"\n# </block>".to_string(),
            ),
            (
                "toml.toml".to_string(),
                "# <block>\nname = \"test\"\n# </block>".to_string(),
            ),
            (
                "Pipfile".to_string(),
                "# <block>\n[packages]\nrequests = \"*\"\n# </block>".to_string(),
            ),
            (
                "ts.ts".to_string(),
                "// <block>\nconst x: number = 1;\n// </block>".to_string(),
            ),
            (
                "tsx.tsx".to_string(),
                "// <block>\nconst C = () => <div/>;\n// </block>".to_string(),
            ),
            (
                "typescript.d.ts".to_string(),
                "// <block>\ndeclare const x: number;\n// </block>".to_string(),
            ),
            (
                "cts.cts".to_string(),
                "// <block>\nconst x: number = 1;\n// </block>".to_string(),
            ),
            (
                "mts.mts".to_string(),
                "// <block>\nexport const x: number = 1;\n// </block>".to_string(),
            ),
            (
                "xml.xml".to_string(),
                "<!-- <block> -->\n<root/>\n<!-- </block> -->".to_string(),
            ),
            (
                "App.xaml".to_string(),
                "<!-- <block> -->\n<Application/>\n<!-- </block> -->".to_string(),
            ),
            (
                "Directory.Build.props".to_string(),
                "<!-- <block> -->\n<Project/>\n<!-- </block> -->".to_string(),
            ),
            (
                "Directory.Build.targets".to_string(),
                "<!-- <block> -->\n<Project/>\n<!-- </block> -->".to_string(),
            ),
            (
                "app.csproj".to_string(),
                "<!-- <block> -->\n<Project/>\n<!-- </block> -->".to_string(),
            ),
            (
                "app.fsproj".to_string(),
                "<!-- <block> -->\n<Project/>\n<!-- </block> -->".to_string(),
            ),
            (
                "app.vbproj".to_string(),
                "<!-- <block> -->\n<Project/>\n<!-- </block> -->".to_string(),
            ),
            (
                "app.vcxproj".to_string(),
                "<!-- <block> -->\n<Project/>\n<!-- </block> -->".to_string(),
            ),
            (
                "schema.xsd".to_string(),
                "<!-- <block> -->\n<xs:schema/>\n<!-- </block> -->".to_string(),
            ),
            (
                "style.xsl".to_string(),
                "<!-- <block> -->\n<xsl:stylesheet/>\n<!-- </block> -->".to_string(),
            ),
            (
                "style.xslt".to_string(),
                "<!-- <block> -->\n<xsl:stylesheet/>\n<!-- </block> -->".to_string(),
            ),
            (
                "yaml.yaml".to_string(),
                "# <block>\nkey: value\n# </block>".to_string(),
            ),
            (
                "yml.yml".to_string(),
                "# <block>\nname: test\n# </block>".to_string(),
            ),
            (
                "CITATION.cff".to_string(),
                "# <block>\nversion: 1.0.0\n# </block>".to_string(),
            ),
        ];
        for (file_name, contents) in &files {
            let parser = parsers
                .parser_for(Path::new(file_name))
                .unwrap_or_else(|| panic!("{file_name} should have a language"));
            let has_comments = parser
                .lock()
                .expect("a thread panicked while it held the parser")
                .parse_comments(contents)?
                .next()
                .is_some();
            assert!(has_comments, "{file_name} should have comments");
        }
        Ok(())
    }
    // </block>
}
