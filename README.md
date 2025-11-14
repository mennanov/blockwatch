# BlockWatch: smart language agnostic linter

[![Build Status](https://github.com/mennanov/blockwatch/actions/workflows/rust.yml/badge.svg)](https://github.com/mennanov/blockwatch/actions)
[![codecov](https://codecov.io/gh/mennanov/blockwatch/graph/badge.svg?token=LwUfGTZ551)](https://codecov.io/gh/mennanov/blockwatch)
[![Crates.io](https://img.shields.io/crates/v/blockwatch)](https://crates.io/crates/blockwatch)
[![Downloads](https://img.shields.io/crates/d/blockwatch)](https://crates.io/crates/blockwatch)

## Why

Have you ever updated a function but forgotten to update the `README.md` example that uses it?
Or changed a list of supported items in your configuration but forgot to update the corresponding list in the
documentation?

Keeping everything in sync and valid manually is tedious and error-prone.

## Features

**Blockwatch** keeps your codebase consistent by making dependencies and formatting requirements explicit and
automatically verifiable.

[//]: # (<block name="validators-list">)

- 📖 Keeps your docs up to date with the code
- 🔤 Enforces formatting rules (e.g. sorted lines)
- ✅ Ensures unique lines
- 🔍 Validates lines against a regex pattern
- 🔢 Enforces number of lines
- 🤖 Validates blocks with conditions checked by AI (LLM)

[//]: # (</block>)

## How It Works

Blocks are declared as XML tags in the source code comments:

```python
fruits = [
    # <block keep-sorted="asc">
    "apple",
    "banana",
    "orange"
    # </block>
]
```

When changes are made running the following command will validate them:

```shell
git diff --patch | blockwatch
```

### Tracking Dependencies

Use the `affects` attribute to create relationships between blocks:

Mark a "source" block of code and give a name to a "dependent" block (possibly in a different file e.g.
your documentation).

In `src/parsers/mod.rs`, we define a list of languages. This block is marked as
`affects="README.md:supported-grammar-example"`, creating a dependency link:

```rust
pub(crate) fn language_parsers() -> anyhow::Result<HashMap<String, Rc<Box<dyn BlocksParser>>>> {
    Ok(HashMap::from([
        // Will report a violation if this list is updated, but the block `README.md:supported-grammar-example` is not,
        // which helps keeping the docs up-to-date:
        // <block affects="example.py:supported-grammar-example">
        ("rs".into(), rust_parser),
        ("js".into(), Rc::clone(&js_parser)),
        ("go".into(), go_parser),
        // </block>
    ]))
}
```

In `example.py`, we define the block that depends on the code above:

```python
# example.py

supported_languages = [
    # <block name="supported-grammar-example">
    "Go",
    "JavaScript",
    "Rust",
    # </block>
]
```

This simple mechanism ensures your documentation and code never drift apart.

### Maintaining Lines Order

Use the `keep-sorted` attribute to ensure content stays properly sorted:

```rust
const MONTHS: [&str; 12] = [
    // Will report a violation if not sorted:
    // <block keep-sorted="asc">
    "April",
    "August",
    "December",
    "February",
    "January",
    "July",
    "June",
    "March",
    "May",
    "November",
    "October",
    "September",
    // </block>
];
```

Empty lines and spaces are ignored.

You can optionally provide `keep-sorted-pattern` to sort by a specific part of each line using a Regular Expression:

- If a named capture group `value` exists, its text is used for sorting
- If no named group exists, the entire regex match is used
- Lines that do not match the regex are ignored

```python
items = [
    # Sort ascending by the numeric id only; comments are ignored
    # <block keep-sorted="asc" keep-sorted-pattern="^id: (?P<value>\d+)">
    "id: 1  apple",
    "id: 2  banana",
    "this line is skipped",
    "id: 10 orange",
    # </block>
]
```

Another example using full match (no named group):

```python
kv = [
    # <block keep-sorted="asc" keep-sorted-pattern="^x=\d+">
    "x=1; note",
    "ignore me",
    "x=2; note",
    # </block>
]
```

### Ensuring Unique Lines

Use the `keep-unique` attribute with an optional RegExp to ensure there are no duplicate lines inside a block.

- Default behavior (empty attribute): uses the entire line as the value to check for uniqueness.
- Regex behavior (non-empty attribute): treats the attribute as a Regular Expression. If a named capture group "value"
  is present, that group's text is used; otherwise, the entire match is used. Lines that do not match the regex are
  ignored.

```python
contributors = [
    # <block keep-unique>
    "Alice",
    "Bob",
    "Carol"
    # </block>
]
```

Regex example using a named group to only consider the numeric ID for uniqueness and ignore non-matching lines:

```python
ids = [
    # <block keep-unique="^ID:(?P<value>\d+)">
    "ID:1 Alice",
    "ID:2 Bob",
    "this line is skipped",
    "ID:1 Carol",  # duplicate by extracted ID
    # </block>
]
```

Empty lines and spaces are ignored.

### Validating Line Patterns

Use the `line-pattern` attribute to ensure every line in the block matches a Regular Expression:

```python
slugs = [
    # <block line-pattern="[a-z0-9-]+">
    "hello-world",
    "rust-2025",
    "blockwatch"
    # </block>
]
```

Empty lines and spaces are ignored.

### Validating Block Line Count

Use the `line-count` attribute to ensure the total number of lines in a block meets a constraint:

- `line-count="<50"` — strictly less than 50 lines
- `line-count=">=3"` — at least 3 lines
- `line-count="==10"` — exactly 10 lines

```python
my_list = [
    # <block line-count="<=3">
    "a",
    "b",
    "c"
    # </block>
]
```

Empty lines are ignored.

### Validating with AI (LLM)

Use the `check-ai` attribute to validate a block against a natural-language condition using an LLM.
The model will return an actionable error message if the condition is not met.

#### Attributes

- `check-ai` (required) — Natural language condition that the block must satisfy
- `check-ai-pattern` (optional) — Regular Expression to extract specific content from the block before validation
    - If a named capture group `value` is present, only that group's text is used
    - If no named group exists, the entire match is used
    - Lines that don't match the pattern are ignored

#### Examples

Basic validation:

```html
<h1>Policy Section</h1>

<!-- <block name="policy" check-ai="The block must mention the word 'banana' at least once."> -->
We like apples and oranges.
<!-- </block> -->
```

Using `check-ai-pattern` to validate only extracted content:

```python
# Validate only the numeric values, ignoring the rest
prices = [
    # <block check-ai="All prices must be under $100" check-ai-pattern="\$(?P<value>\d+)">
    "Item A: $50",
    "Item B: $75",
    "Item C: $120",  # This will fail validation
    # </block>
]
```

If the content does not satisfy the condition, BlockWatch will report a violation.

#### Configuration

[//]: # (<block name="check-ai-env-vars">)
- Set `BLOCKWATCH_AI_API_KEY` env variable to contain an LLM API key.
- Optional: Set `BLOCKWATCH_AI_API_URL` env variable to point to an OpenAi-compatible LLM API (default:
  `https://api.openai.com/v1`).
- Optional: Set `BLOCKWATCH_AI_MODEL` to override the default model (default: `gpt-4o-mini`).


[//]: # (</block>)

#### ⚠️ Security Warning

> **When used in CI make sure it can be triggered by trusted users only.**
> Otherwise, an API quota may be exhausted.

-----

## Installation

### Homebrew (macOS and Linux)

If you use Homebrew:

```shell
brew tap mennanov/tap
brew install blockwatch
```

- To upgrade later: `brew upgrade blockwatch`
- To uninstall: `brew uninstall blockwatch`

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

[//]: # (<block name="cli-docs">)

The simplest way to run it is by piping a git diff into the command:

```shell
git diff --patch | blockwatch
```

#### Disabling Validators

You can selectively disable specific validators using the `-d` or `--disable` flag.

> NOTE: `--disable` flag can't be used together with the `--enable` flag.

**Examples:**

```shell
# Disable a single validator
git diff --patch | blockwatch --disable=keep-sorted

# Disable multiple validators (use multiple -d flags)
git diff --patch | blockwatch -d keep-sorted -d line-count
```

#### Enabling Validators

You can selectively enable specific validators using the `-e` or `--enable` flag.

Only the enabled validators will run the checks.

> NOTE: `--enable` flag can't be used together with the `--disable` flag.

**Examples:**

```shell
# Enable a single validator, other validators will not run
git diff --patch | blockwatch --enable=keep-sorted

# Enable multiple validators (use multiple -e flags)
git diff --patch | blockwatch -e keep-sorted -e line-count
```

##### Available validators

[//]: # (<block name="available-validators">)

- [`affects`](#tracking-dependencies)
- [`check-ai`](#validating-with-ai-llm)
- [`keep-sorted`](#maintaining-lines-order)
- [`keep-unique`](#ensuring-unique-lines)
- [`line-count`](#validating-block-line-count)
- [`line-pattern`](#validating-line-patterns)

[//]: # (</block>)

[//]: # (</block>)

### Pre-commit Hook

For automatic checks before each commit, use it with the [`pre-commit`](https://pre-commit.com) framework.
Add this to your `.pre-commit-config.yaml`:

```yaml
repos:
  - repo: local
    hooks:
      - id: blockwatch
        name: blockwatch
        entry: bash -c 'git diff --patch --cached --unified=0 | blockwatch'
        language: system
        stages: [ pre-commit ]
        pass_filenames: false
```

### GitHub Action

Add to `.github/workflows/your_workflow.yml`:

```yaml
# 
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
- Ruby (`.rb`)
- Rust (`.rs`)
- SQL (`.sql`)
- Swift (`.swift`)
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

## Examples

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

- Deleted blocks are ignored.
- Files with unsupported grammar are ignored.

## Contributing

Contributions are welcome! A good place to start is
by [adding support for a new grammar](https://github.com/mennanov/blockwatch/pull/2).

### Run Tests

```shell
cargo test
```