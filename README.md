# blockwatch

[![Build Status](https://github.com/mennanov/blockwatch/actions/workflows/rust.yml/badge.svg)](https://github.com/mennanov/blockwatch/actions)
[![codecov](https://codecov.io/gh/mennanov/blockwatch/graph/badge.svg?token=LwUfGTZ551)](https://codecov.io/gh/mennanov/blockwatch)
[![Crates.io](https://img.shields.io/crates/v/blockwatch)](https://crates.io/crates/blockwatch)
[![Downloads](https://img.shields.io/crates/d/blockwatch)](https://crates.io/crates/blockwatch)

[//]: # (<block name="pitch">)
Some parts of a codebase have to change together: a function and its docs, or a value duplicated across configs.
blockwatch lets you write those rules in comments, right next to the code. If they fall out of sync, your pre-commit
hook or CI run fails.

Supports [34 languages](#supported-languages).

[//]: # (</block>)

## Quick Start

Install blockwatch:

```shell
brew install mennanov/blockwatch/blockwatch
```

For Windows and other ways to install, see [Installation](#installation).

Then paste this prompt into your AI coding agent:

<!-- <block name="agent-prompt"> -->

```text
Run `blockwatch skill` and save its output as `blockwatch/SKILL.md` in this project's skills directory. Then use the
skill to annotate the project and add `blockwatch.toml` if needed. List each block you added and the mistake it
catches.
```

<!-- </block> -->

Review the suggested blocks and commit them. The skill stays in the project, so agents working on it later know
the rules too. When you upgrade blockwatch, the skill tells the agent to save it again. To check every future change,
add blockwatch to your [pre-commit hook or CI](#ci-integration).

## How it works

A block is a pair of tags inside comments around some lines of code. The rules go in the start tag. In this example, the
enum variants must match the list in `README.md`. Each block uses `same-as-pattern` to pick the values to compare. Here,
`\w+` matches each word, so commas and dashes don't count:

**src/lib.rs**:

```rust
pub enum Language {
    // <block name="languages" same-as="README.md:supported-languages" same-as-pattern="\w+">
    Rust,
    Python,
    // </block>
}
```

**README.md**:

```markdown
<!-- <block name="supported-languages" same-as-pattern="\w+"> -->

- Rust
- Python

<!-- </block> -->
```

If you add a `Go` variant to the enum without updating `README.md`, `blockwatch` fails with exit code 1 and this
message:

```text
Block src/lib.rs:languages at line 2 disagrees with README.md:supported-languages: ["Rust", "Python", "Go"] != ["Rust", "Python"]
```

The output shows the values on each side. Add `Go` to `README.md` too, and the run passes.

Because `same-as` compares block contents directly, it doesn't need a diff. Running `blockwatch` on its own finds
mismatches anywhere in the repository. The [other validators](#validators) make code and docs change together, keep
lists sorted, and more.

## Validators

Two validators work across files:

- `affects` fails when a diff touches a block without also changing what it points to.
- `same-as` fails when a block and its target hold different values. It needs no diff.

Both can point to a block, a whole file, or a specific value in a file like `package.json#/version`
(see [Symbols](docs/symbols.md)). The other validators check a single block on their own: the things you'd otherwise
have to nitpick in code review.

<!-- <block name="available-validators"
     same-as="src/validators/mod.rs:validator-registry, docs/validators/README.md:validators-index"
     same-as-pattern='^\|\s*\[`(?P<value>[a-z-]+)`\]'> -->

| Validator                                         | Description                                                                                  | Attributes                                                     |
|---------------------------------------------------|----------------------------------------------------------------------------------------------|----------------------------------------------------------------|
| [`affects`](docs/validators/affects.md)           | Forces linked blocks to be edited together (e.g. code and its docs); needs a diff            | `affects`                                                      |
| [`same-as`](docs/validators/same-as.md)           | Asserts two or more blocks hold the same value, across languages and formats; no diff needed | `same-as`, `same-as-pattern`, `same-as-mode`, `same-as-format` |
| [`keep-sorted`](docs/validators/keep-sorted.md)   | Enforces alphabetical or numerical ordering on list items                                    | `keep-sorted`, `keep-sorted-pattern`, `keep-sorted-format`     |
| [`keep-unique`](docs/validators/keep-unique.md)   | Prevents duplicate lines within a block                                                      | `keep-unique`                                                  |
| [`line-pattern`](docs/validators/line-pattern.md) | Enforces that every line matches a specified regex                                           | `line-pattern`                                                 |
| [`line-count`](docs/validators/line-count.md)     | Enforces lower or upper bounds on the number of lines in a block                             | `line-count`                                                   |
| [`check-ai`](docs/validators/check-ai.md)         | Validates content against natural language rules using an LLM                                | `check-ai`, `check-ai-pattern`                                 |
| [`check-lua`](docs/validators/check-lua.md)       | Runs custom validation logic written in Lua                                                  | `check-lua`, `check-lua-pattern`, `check-lua-timeout`          |

<!-- </block> -->

Every block can also take an optional `name` so other blocks can point to it, and a
[`severity`](docs/validators/README.md#severity). Only `error`, the default, fails the run.

See the [Validators Reference](docs/validators/README.md) for full details.

## Installation

```shell
# macOS and Linux, with Homebrew
brew install mennanov/blockwatch/blockwatch

# macOS and Linux, without Homebrew
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/mennanov/blockwatch/releases/latest/download/blockwatch-installer.sh | sh

# Windows
powershell -ExecutionPolicy Bypass -c "irm https://github.com/mennanov/blockwatch/releases/latest/download/blockwatch-installer.ps1 | iex"

# From source, with Rust
cargo install blockwatch
```

Prebuilt binaries are on the [Releases](https://github.com/mennanov/blockwatch/releases) page.

## AI Agents

The [skill](src/skill.md) tells an agent where blocks make sense, how to format them, and how to check its own work.
`blockwatch skill` prints it. The [Quick Start](#quick-start) prompt sets it up in a single project. To install it
globally for every project in Claude Code instead:

```text
/plugin marketplace add mennanov/blockwatch
/plugin install blockwatch@blockwatch
```

See [docs/agents.md](docs/agents.md) for Cursor, Copilot, Codex, and other agent setups.

## Usage

Running `blockwatch` on its own checks every block in the repository. With `--diff`, blockwatch reads a unified diff
from stdin to enforce rules that need one, like `affects`. Adding `--only-changed` limits the check to only the blocks
touched by that diff:

```shell
# Check every block in the repository
blockwatch

# Check specific files, directories or globs
blockwatch src/main.rs docs "**/*.md"

# Check every block, and enforce the rules that need a diff, such as `affects`
git diff --patch | blockwatch --diff

# Check only the blocks the diff changed
git diff --patch | blockwatch --diff --only-changed

# The same, for staged and unstaged changes
git diff --patch HEAD | blockwatch --diff --only-changed

# Dump all discovered blocks as JSON
blockwatch list
```

Rules live in comments, next to the code they describe. Settings that apply to the whole project, such as ignored
paths, extension mappings, disabled validators, and skipped blocks, go into `blockwatch.toml` at the repository root:

```toml
ignore = ['**/generated/**']
disable = ['check-ai']

[extensions]
cxx = 'cpp'
```

The config file can also declare [blocks around a key](docs/cli.md#blocks-in-the-config-file) in a JSON, TOML, or YAML
file instead of using comment tags. That is how files without comment support, such as `package.json`, get rules.

To keep a known violation from failing the run without editing the source, pass its `address` from the output to
`--suppress`. The violation is still reported. For the failure in [How it works](#how-it-works):

```shell
blockwatch --suppress src/lib.rs:languages:same-as:270d21b4
```

A shorter address like `src/lib.rs` covers more violations. See
[Suppressing a Violation](docs/cli.md#suppressing-a-violation).

To check only some blocks, or to skip some, pass `FILE:BLOCK_NAME` to `--only-block` or `--skip-block`. The
`only-blocks` and `skip-blocks` keys of `blockwatch.toml` do the same for every run. See
[Selecting Blocks](docs/cli.md#selecting-blocks).

See [docs/cli.md](docs/cli.md) for all CLI flags, execution modes, the [config file](docs/cli.md#config-file), path
exclusions, and custom extension mappings.

## CI Integration

**pre-commit** (`.pre-commit-config.yaml`):

<!-- <block name="pre-commit-rev" same-as="Cargo.toml:crate-version" same-as-mode="subset"
     same-as-pattern='rev: v(?P<value>\d+\.\d+\.\d+)'> -->

```yaml
- repo: https://github.com/mennanov/blockwatch
  rev: v0.10.0  # Use latest release
  hooks:
    - id: blockwatch
```

<!-- </block> -->

**GitHub Actions**:

```yaml
- uses: mennanov/blockwatch-action@v1
```

blockwatch exits with `1` when it finds at least one `error` violation, and `0` otherwise. Warnings, info, and hints
are printed, but won't fail the run.

For GitHub code scanning, `blockwatch --format sarif` writes violations as a SARIF log instead of JSON. See
[SARIF Output](docs/cli.md#sarif-output).

For plain git hooks, local pre-commit setups, and sandboxing untrusted Lua scripts in fork pull requests, see
[docs/ci.md](docs/ci.md).

## Supported Languages

[//]: # (<block name="supported-grammar" keep-sorted="asc" affects=":pitch">)

- Bash
- C#
- C/C++ (`.c`, `.cc`, `.cpp`, `.h`)
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
- JSON (`.json`, `.jsonc`)
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

blockwatch only inspects files with the extensions listed above and skips everything else, even `.hpp`, `.hxx`, or
`.cxx`. To check those too, map them to a supported extension with `-E` or in the `extensions` table of
`blockwatch.toml`:

```shell
blockwatch -E cxx=cpp -E hpp=cpp
```

## Known Limitations

- **Deleting a block deletes its rule, quietly.** If you delete a file or strip its tags, its rules are gone. The run
  passes without warning you that a rule disappeared. Blocks still *pointing* at the deleted one do fail, as a missing
  reference.
- **A file needs comments or symbols to hold a block.** Files like CSV and `.env` have nowhere to put a tag. Link to
  such a file as a [whole file](docs/validators/affects.md#whole-files) instead. For files with
  [symbols](docs/symbols.md), like plain JSON, you can define blocks in the
  [config file](docs/cli.md#blocks-in-the-config-file).
- **A file with blocks must be UTF-8.** A file in another encoding, such as Latin-1, is skipped if it has no blocks. If
  it has blocks, the run fails and shows the file. A UTF-16 file is skipped even when it has tags, because blockwatch
  can't find them.
- **Unsupported extensions are skipped silently.** A run that read nothing looks identical to a run that found no
  problems. Run `blockwatch --verbosity summary` to see how many files were actually checked.

## Contributing

PRs and issues are welcome!

To run tests locally:

```shell
cargo test
```
