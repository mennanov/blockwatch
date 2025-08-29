# BlockWatch: Never Let Your Code and Docs Go Stale Again

> Automatically spot inconsistencies in your documentation, configuration, and code whenever they change.

[![Build Status](https://github.com/mennanov/blockwatch/actions/workflows/rust.yml/badge.svg)](https://github.com/mennanov/blockwatch/actions)
[![codecov](https://codecov.io/gh/mennanov/blockwatch/graph/badge.svg?token=LwUfGTZ551)](https://codecov.io/gh/mennanov/blockwatch)
[![Crates.io](https://img.shields.io/crates/v/blockwatch)](https://crates.io/crates/blockwatch)
[![Downloads](https://img.shields.io/crates/d/blockwatch)](https://crates.io/crates/blockwatch)

## Why

Have you ever updated a function but forgotten to update the `README.md` example that uses it? Or changed a list of
supported items in your code but forgot to update the corresponding list in the documentation?

Keeping everything in sync manually is tedious and error-prone.

## What

**BlockWatch** is a dependency tracker for your codebase. You declare relationships between blocks of
code, if a block changes, BlockWatch will require that its dependents are also
updated, catching inconsistencies before they are committed.

It keeps your codebase consistent by making dependencies between code and documentation explicit and verifiable.

## How It Works

It's a simple two-step process:

### 1. Declare your blocks

Mark a "source" block of code and give a name to a "dependent" block in another file (like
your documentation).

In [src/parsers/mod.rs](./src/parsers/mod.rs), we define a list of languages. This block is marked as
`affects="README.md:supported-grammar"`, creating a dependency link.

```rust
// src/parsers/mod.rs
// <block affects="README.md:supported-grammar-example">
pub(crate) fn language_parsers() -> anyhow::Result<HashMap<String, Rc<Box<dyn BlocksParser>>>> {
    Ok(HashMap::from([
        ("rs".into(), rust_parser),
        ("js".into(), Rc::clone(&js_parser)),
        ("go".into(), go_parser),
    ]))
}
// </block>
```

In `README.md`, we define the block that depends on the code above.

```markdown
## Supported Languages

[//]: # (<block name="supported-grammar-example">)

- Rust
- JavaScript
- Go

[//]: # (</block>)
```

### 2. Run validation

When you make a change, BlockWatch checks the git diff. If you modify the
`SUPPORTED_LANGUAGES` array in `src/parsers/mod.rs` without also modifying the block in `README.md`, BlockWatch will
exit with an error, helping to prevent a bad commit.

```shell
# This command will now fail until README.md is updated
git diff --patch | blockwatch
```

This simple mechanism ensures your documentation and code never drift apart.

-----

## Key Features

- 🔗 **Link Code to Docs:** Create explicit, verifiable dependencies between code snippets and your documentation.
- 🤖 **Automated Git Workflow:** Integrates seamlessly with `git diff` to automatically validate changes before you
  commit.
- 🛠️ **Pre-commit & CI/CD Ready:** Includes support for pre-commit hooks and a dedicated GitHub Action for robust CI/CD
  pipelines.
- 🌍 **Broad Language Support:** Works with over 20 programming and markup languages out of the box.
- ⚡ **Fast and Lightweight:** Written in Rust for maximum performance with no runtime dependencies.

-----

## Installation

### From Source

Requires the [Rust toolchain](https://doc.rust-lang.org/cargo/getting-started/installation.html):

```shell
cargo install blockwatch
```

### Prebuilt Binary

Download a pre-built binary for your platform from the [Releases page](https://github.com/mennanov/blockwatch/releases).

-----

## Usage & Integration

### Command Line

The simplest way to run it is by piping a git diff into the command:

```shell
git diff --patch | blockwatch
```

### Pre-commit Hook

For automatic checks before each commit, use it with the [`pre-commit`](https://pre-commit.com) framework.
Add this to your `.pre-commit-config.yaml`:

```yaml
repos:
  - repo: local
    hooks:
      - id: blockwatch
        name: blockwatch
        entry: bash -c 'git diff --patch --unified=0 | blockwatch'
        language: system
        stages: [ pre-commit ]
        pass_filenames: false
```

### GitHub Action

Add the following job to your workflow file to check for inconsistencies in pull requests.

```yaml
# .github/workflows/main.yml
jobs:
  blockwatch:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3
        with:
          fetch-depth: 2 # Required to diff against the base branch
      - uses: mennanov/blockwatch-action@v1
```

-----

## Supported Languages

BlockWatch supports a wide range of common languages.

[//]: # (<block name="supported-grammar" keep-sorted="asc">)
- Bash (`.sh`, `.bash`)
- C# (`.cs`)
- C/C++ (`.c`, `.cpp`, `.cc`, `.h`)
- CSS (`.css`)
- Golang (`.go`)
- HTML (`.html`, `.htm`)
- Java (`.java`)
- JavaScript (`.js`, `.jsx`)
- Kotlin (`.kt`, `.kts`)
- Markdown (`.md`, `.markdown`)
- PHP (`.php`, `.phtml`)
- Python (`.py`, `.pyi`)
- Rust (`.rs`)
- SQL (`.sql`)
- TOML (`.toml`)
- TypeScript (+TSX) (`.ts`, `.d.ts`, `.tsx`)
- XML (`.xml`)
- YAML (`.yaml`, `.yml`)

[//]: # (</block>)

**Have a custom file extension?**

You can map it to a supported grammar:

```shell
# Treat .xhtml files as .xml
git diff --patch | blockwatch -E xhtml=xml
```

-----

## Advanced Patterns

### Same-File Dependencies

Blocks can affect other blocks in the same file. Just omit the filename in the `affects` attribute.

```rust
// <block name="foo" affects=":bar, :buzz">
fn main() {
    println!("Blocks can affect multiple other blocks declared in the same file");
    println!("Just omit the file name in the 'affects' attribute");
}
// </block>

// <block name="bar">
// Some other piece of code.
// </block>

// <block name="buzz">
// One more.
// </block>
```

### Mutual Dependencies

Blocks can reference each other.

```rust
// <block name="alice" affects=":bob">
fn foo() {
    println!("Hi, Bob!");
}
// </block>

// <block name="bob" affects=":alice">
fn bar() {
    println!("Hi, Alice!");
}
// </block>
```

### Nested Blocks

Blocks can be nested inside one another.

```rust
// <block name="entire-file">
fn foo() {
    println!("Hello");
}

// <block name="small-block">
fn bar() {
    println!("Hi!");
}
// </block>
// </block>
```

-----

## Known Limitations

- Deleted blocks are currently ignored.
- Files with unsupported grammar are ignored.
- Multiple blocks cannot be declared on a single line: `<block><block>will not work</block></block>`.

## Contributing

Contributions are welcome! A good place to start is
by [adding support for a new grammar](https://github.com/mennanov/blockwatch/pull/2).

### Run Tests

```shell
cargo test
```