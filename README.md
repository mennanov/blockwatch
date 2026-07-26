# BlockWatch

[![Build Status](https://github.com/mennanov/blockwatch/actions/workflows/rust.yml/badge.svg)](https://github.com/mennanov/blockwatch/actions)
[![codecov](https://codecov.io/gh/mennanov/blockwatch/graph/badge.svg?token=LwUfGTZ551)](https://codecov.io/gh/mennanov/blockwatch)
[![Crates.io](https://img.shields.io/crates/v/blockwatch)](https://crates.io/crates/blockwatch)
[![Downloads](https://img.shields.io/crates/d/blockwatch)](https://crates.io/crates/blockwatch)

A language-agnostic linter that keeps co-dependent code, docs, and config from drifting apart. You declare the rule in a
comment, next to the thing it guards.

<p>
  <img src="demo.gif" alt="BlockWatch Demo">
</p>

## Why BlockWatch

- **Catches cross-file drift.** Link an enum to the docs that describe it, or a constant to the config that duplicates
  it. Change one side and forget the other, and the build tells you.
- **Rules live where they apply** — inside comments, in the file they govern. There is no central config file to fall
  out of date with the code.
- **Free to adopt.** With a diff on stdin, only the blocks your change touched are checked. Adding a rule never
  retroactively fails anyone else's work, so you can annotate one file at a time.
- **One tool for the whole repo.** 30+ languages, plus Markdown, YAML, TOML, and Dockerfiles — which is what makes
  linking code to its documentation possible in the first place.

## Install

```shell
brew install mennanov/blockwatch/blockwatch   # macOS/Linux
cargo install blockwatch                      # from source
```

Prebuilt binaries are on the [Releases](https://github.com/mennanov/blockwatch/releases) page.

## Quick start

Put `<block>` tags in a comment in any [supported file](#supported-languages):

**src/lib.rs**:

```rust
// <block affects="README.html:supported-langs">
pub enum Language {
    Rust,
    Python,
}
// </block>
```

**README.html**:

```html
<!-- <block name="supported-langs"> -->
<ul>
    <li>Rust</li>
    <li>Python</li>
</ul>
<!-- </block> -->
```

Add a variant to the enum and BlockWatch fails until you touch `supported-langs` too.
[`affects`](docs/validators/affects.md) checks that the two were edited together;
[`same-as`](docs/validators/same-as.md) goes further and checks they still hold the same *value*.

## Let an agent annotate your repo

Adding the first tags by hand is the tedious part, and agents are good at it. This repo ships a
[skill](.agents/skills/blockwatch/SKILL.md) that tells them where blocks are worth adding and how to verify their own
work.

Claude Code users install it once, for every project:

```text
/plugin marketplace add mennanov/blockwatch
/plugin install blockwatch@blockwatch
```

Cursor, Copilot, and Codex setup, plus a prompt to start from: [docs/agents.md](docs/agents.md).

## Validators

[//]: # (<block name="available-validators">)

| Attribute                                         | Enforces                                                             |
|---------------------------------------------------|----------------------------------------------------------------------|
| [`affects`](docs/validators/affects.md)           | Linked blocks are edited together — code and its docs, config, tests |
| [`same-as`](docs/validators/same-as.md)           | Two or more blocks still hold the same value                         |
| [`keep-sorted`](docs/validators/keep-sorted.md)   | A list stays ordered, lexicographically or numerically               |
| [`keep-unique`](docs/validators/keep-unique.md)   | No duplicate entries                                                 |
| [`line-pattern`](docs/validators/line-pattern.md) | Every line matches a regex                                           |
| [`line-count`](docs/validators/line-count.md)     | A block stays within a size bound                                    |
| [`check-ai`](docs/validators/check-ai.md)         | A rule stated in plain English, checked by an LLM                    |
| [`check-lua`](docs/validators/check-lua.md)       | Custom logic, written as a Lua script                                |

[//]: # (</block>)

Any block also accepts `name` and
[`severity`](docs/validators/README.md#severity) — set `severity="warning"` to report a rule without failing the build
while you clean up existing violations.

Full reference: [docs/validators/](docs/validators/README.md).

## Usage

```shell
blockwatch                              # check everything
blockwatch "src/**/*.rs" "**/*.md"      # check some globs (quote them)
git diff --patch | blockwatch           # check only the blocks you touched
git diff --cached --patch | blockwatch  # same, for staged changes
blockwatch list                         # JSON dump of every block found
```

Options for ignoring paths, mapping extensions, and turning individual validators on and off are in
the [CLI reference](docs/cli.md).

## CI integration

Pre-commit hook, in `.pre-commit-config.yaml`:

```yaml
- repo: https://github.com/mennanov/blockwatch
  rev: v0.2.27  # use the latest release tag
  hooks:
    - id: blockwatch
```

GitHub Actions:

```yaml
- uses: mennanov/blockwatch-action@v1
```

Plain git hooks, the local pre-commit form, and how to keep fork PRs from running untrusted Lua:
[docs/ci.md](docs/ci.md).

## Supported languages

[//]: # (<block name="supported-grammar" keep-sorted="asc">)

- Bash
- C#
- C/C++
- CMake (`CMakeLists.txt`, `.cmake`)
- CSS
- Dart
- Dockerfile (with `Containerfile` and `.dockerfile` support)
- Elixir (`.ex`, `.exs`)
- Go (with `go.mod`, `go.sum` and `go.work` support)
- GraphQL (`.graphql`, `.gql`)
- Groovy (with `.gradle` and `Jenkinsfile` support)
- HCL (Terraform: `.tf`, `.tfvars`, `.hcl`)
- HTML
- Java
- JavaScript
- Kotlin
- Lua
- Makefile
- Markdown
- Nix
- PHP
- Protocol Buffers (`.proto`)
- Python
- Ruby
- Rust
- SQL
- Scala (with `.sbt` support)
- Starlark (Bazel: `BUILD`, `WORKSPACE`, `MODULE.bazel`, `.bzl`, `.bzlmod`, `.star`)
- Swift
- TOML
- TypeScript
- XML
- YAML

[//]: # (</block>)

Map an unrecognized extension onto a supported grammar with `-E cxx=cpp`.

## Known limitations

- Deleted blocks are ignored.
- Files with unsupported grammar are ignored.

## Contributing

Contributions are welcome. A good place to start is
[adding support for a new grammar](https://github.com/mennanov/blockwatch/pull/2).

```shell
cargo test
```
