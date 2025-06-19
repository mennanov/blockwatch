# BlockWatch

> A powerful code linter for tracking dependencies between documentation and code blocks across multiple files and
> languages

[![Build Status](https://github.com/mennanov/blockwatch/actions/workflows/rust.yml/badge.svg)](https://github.com/mennanov/blockwatch/actions)
[![codecov](https://codecov.io/gh/mennanov/blockwatch/graph/badge.svg?token=LwUfGTZ551)](https://codecov.io/gh/mennanov/blockwatch)
[![Crates.io](https://img.shields.io/crates/v/blockwatch)](https://crates.io/crates/blockwatch)
[![Downloads](https://img.shields.io/crates/d/blockwatch)](https://crates.io/crates/blockwatch)

🔍 Keep your codebase consistent by automatically tracking and validating dependencies between code blocks and
documentation.

## Features

- 📝 Track dependencies between code blocks across files
- 🔄 Automatic validation on git diff
- 🛠️ Pre-commit hook support
- 🚀 GitHub Actions integration
- 🌍 Support for 20+ programming languages
- ⚡ Fast and lightweight

## Usage

BlockWatch validates dependencies between code blocks by monitoring changes in your git diff:

1. Declare dependent blocks in your source files using special comment syntax
2. Run validation before commits: `git diff --patch | blockwatch`
3. _Optionally_: install a pre-commit hook
4. _Optionally_: integrate with CI/CD pipelines for automated checks

### Getting Started

First, mark your code blocks with dependency declarations:

### Example

Whenever some block is modified then all its affected blocks (possibly in different files) should also be updated.

`main.rs`:

```rust
// <block affects="README.md:supported-grammar">
const SUPPORTED_LANGUAGES = ["rust", "java", "python"];
// </block>
```

`README.md`:

```markdown
## Supported grammar

[//]: # (<block name="supported-grammar">)

- Java
- Rust

[//]: # (</block>)

```

If the block in `main.rs` is modified (e.g. added `python` to the list) then the following command will produce an
error:

```shell
git diff --patch | blockwatch
```

## Run as a GitHub Action

Add the following to your workflow `.yml` file:

```yaml
jobs:

  blockwatch:
    runs-on: ubuntu-latest

    steps:
      - uses: mennanov/blockwatch-action@v1
```

## Run as a pre-commit hook

Ensure `blockwatch` is installed and available in your `PATH`.

### Using [`pre-commit`](https://pre-commit.com/) framework

Add the following to your `.pre-commit-config.yaml`:

```yaml
repos:
  - repo: local
    hooks:
      - id: blockwatch
        name: blockwatch
        entry: bash -c 'git diff --cached --patch --unified=0 | blockwatch'
        language: system
        stages: [ pre-commit ]
        pass_filenames: false
```

## Install

### From source

```shell
cargo install blockwatch
```

### Prebuilt binary

See https://github.com/mennanov/blockwatch/releases

## Supported grammar

[//]: # (<block name="supported-grammar">)

- Bash (`.sh`, `.bash`)
- C/C++ (`.c`, `.cpp`, `.cc`, `.h`)
- C# (`.cs`)
- CSS (`.css`)
- Golang (`.go`)
- HTML (`.html`, `.htm`)
- Kotlin (`.kt`, `.kts`)
- Java (`.java`)
- JavaScript (`.js`, `.jsx`)
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

### Custom file extensions

Map unsupported file extensions to supported ones:

```shell
git diff --patch | blockwatch -E xhtml=xml -E unsupported=supported
```

## More examples

### Blocks may reference blocks in the same file

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

### Blocks may reference each other

```rust
// <block name="foo" affects=":bar">
fn foo() {
    println!("Hello");
}
// </block>

// <block name="bar" affects=":foo">
fn bar() {
    println!("Hi!");
}
// </block>
```

### Blocks can be nested

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

## Known limitations

- deleted blocks are ignored
- blocks declared in files with unsupported grammar are ignored
- multiple blocks can't be declared in a single line: `<block><block>will not work</block/</block>`

## Contributing

### Adding a grammar support

Follow the [pull request for Python](https://github.com/mennanov/blockwatch/pull/2) as an example.

## Run tests

```shell
cargo test
```