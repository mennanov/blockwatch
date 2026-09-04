# BlockWatch

[![Build Status](https://github.com/mennanov/blockwatch/actions/workflows/rust.yml/badge.svg)](https://github.com/mennanov/blockwatch/actions)
[![codecov](https://codecov.io/gh/mennanov/blockwatch/graph/badge.svg?token=LwUfGTZ551)](https://codecov.io/gh/mennanov/blockwatch)
[![Crates.io](https://img.shields.io/crates/v/blockwatch)](https://crates.io/crates/blockwatch)
[![Downloads](https://img.shields.io/crates/d/blockwatch)](https://crates.io/crates/blockwatch)

[//]: # (<block name="pitch">)
Some parts of your codebase must change together: a function and its docs, a value across config files, etc. Blockwatch
makes these relationships explicit and fails a CI run or a pre-commit hook if they drift.

Supports 33 languages. No config files are needed.

[//]: # (</block>)

## Quick Start

Wrap the code in a block and point it at the docs.

**src/lib.rs**:

```rust
// <block name="languages" affects="README.md:supported-languages">
pub enum Language {
    Rust,
    Python,
}
// </block>
```

**README.md**:

```markdown
<!-- <block name="supported-languages"> -->

- Rust
- Python

<!-- </block> -->
```

Now add a `Go` variant to the enum in the Rust code and pass the diff to `blockwatch --diff`:

```console
$ git diff --patch | blockwatch --diff
{
  "src/lib.rs": [
    {
      "code": "affects",
      "data": {
        "affected_block_file_path": "README.md",
        "affected_block_name": "supported-languages"
      },
      "message": "Block src/lib.rs:languages at line 1 is modified, but README.md:supported-languages is not",
      "range": {
        "end": {"character": 63, "line": 1},
        "start": {"character": 4, "line": 1}
      },
      "severity": 1
    }
  ]
}
```

Update the contents of the block in `README.md` and it will pass the check.

## Validators

`affects` and `same-as` are the two that work across files: one forces a co-edit, the other compares the actual values
and needs no diff to do it. The rest check a single block on its own, and are the things you'd otherwise nitpick in code
review.

[//]: # (<block name="available-validators">)

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

[//]: # (</block>)

Blocks can have a `name`, so other blocks can point at it, and [`severity`](docs/validators/README.md#severity). The
`error` severity fails with a non-zero exit code.

Violations can be kept out of the exit code without editing the source:
`blockwatch --suppress FILE[:BLOCK[:VALIDATOR[:HASH]]]` keeps them in the output and stops them failing the run. The
shorter the address, the more it covers: from a single violation up to every violation in a file. Only named blocks can
be addressed below the file level — see [Suppressing a Violation](docs/cli.md#suppressing-a-violation).

See the [Validators Reference](docs/validators/README.md) for full details.

## Installation

```shell
brew install mennanov/blockwatch/blockwatch   # macOS / Linux
cargo install blockwatch                      # from source
```

Prebuilt binaries are on the [Releases](https://github.com/mennanov/blockwatch/releases) page.

## Let an agent integrate this tool

This repository ships a [skill](.agents/skills/blockwatch/SKILL.md) that tells an agent which blocks are worth linking
and how to verify its own edits.

For **Claude Code**:

```text
/plugin marketplace add mennanov/blockwatch
/plugin install blockwatch@blockwatch
```

For Cursor, Copilot, Codex, and other setup options, see [docs/agents.md](docs/agents.md).

## Usage

A bare run checks every block in the repository. Pass `--diff` to read a unified diff from stdin, which marks the blocks
the diff changed, and add `--only-changed` to narrow the run down to those blocks:

```shell
# Check every block in the repository
blockwatch

# Check specific globs
blockwatch "src/**/*.rs" "**/*.md"

# Check every block, and enforce the rules that need a diff, such as `affects`
git diff --patch | blockwatch --diff

# Check only the blocks the diff changed
git diff --patch | blockwatch --diff --only-changed

# The same, for staged changes
git diff --cached --patch | blockwatch --diff --only-changed

# Dump all discovered blocks as JSON
blockwatch list
```

Everything is flags and comments. There is no config file, deliberately: a central config is one more thing that drifts
away from the code it describes, which is the problem this tool exists to solve.

See [docs/cli.md](docs/cli.md) for the run modes in full, CLI flags, path exclusions, and custom extension mappings.

## CI Integration

**pre-commit** (`.pre-commit-config.yaml`):

```yaml
- repo: https://github.com/mennanov/blockwatch
  rev: v0.5.1  # Use latest release
  hooks:
    - id: blockwatch
```

**GitHub Actions**:

```yaml
- uses: mennanov/blockwatch-action@v1
```

BlockWatch exits `1` when it finds at least one `error` severity violation, and `0` otherwise. Warnings, info, and hints
are printed but don't fail the run.

For plain git hooks, local pre-commit setups, and sandboxing untrusted Lua scripts in fork pull requests,
see [docs/ci.md](docs/ci.md).

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

Only the extensions listed above are recognized. Anything else is ignored, including spellings a grammar would otherwise
handle, `.hpp`, `.hxx` and `.cxx` among them. Map those to a supported syntax with `-E`:

```shell
blockwatch -E cxx=cpp -E hpp=cpp
```

## Known Limitations

- **Deleting a block deletes its rule, quietly.** Remove a file, or just strip the tags out of it, and the links it
  declared are gone. The run passes and nothing tells you a rule disappeared. Blocks still *pointing* at the deleted one
  do fail, as a missing reference.
- **A file needs comments to hold a block.** JSON, CSV, and `.env` files have nowhere to put a tag. Link to them with a
  whole-file [`affects`](docs/validators/affects.md#whole-files) instead.
- **Unsupported extensions are skipped silently.** A run that read nothing looks exactly like a run that found no
  problems. `blockwatch --verbosity summary` prints how many files were actually read.

## Contributing

Contributions are welcome. A great first issue
is [adding support for a new grammar](https://github.com/mennanov/blockwatch/pull/2).

To run tests locally:

```shell
cargo test
```
