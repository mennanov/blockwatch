# BlockWatch

[![Build Status](https://github.com/mennanov/blockwatch/actions/workflows/rust.yml/badge.svg)](https://github.com/mennanov/blockwatch/actions)
[![codecov](https://codecov.io/gh/mennanov/blockwatch/graph/badge.svg?token=LwUfGTZ551)](https://codecov.io/gh/mennanov/blockwatch)
[![Crates.io](https://img.shields.io/crates/v/blockwatch)](https://crates.io/crates/blockwatch)
[![Downloads](https://img.shields.io/crates/d/blockwatch)](https://crates.io/crates/blockwatch)

**BlockWatch** is a language-agnostic linter that keeps your code, documentation, and configuration in sync.

It allows you to:

- **Link code to documentation** to ensure updates in one place are reflected in another.
- **Enforce formatting rules** like sorted lists or unique lines.
- **Validate content** using Regex or even AI (LLMs).

BlockWatch can run on your **entire codebase** or check only **changed files** in a git diff.

## Features

[//]: # (<block name="available-validators">)

- 🔗 **Drift Detection**: Explicitly link blocks of code. If one changes, the other must be updated.
- 🧹 **Content Enforcement**:
    - `keep-sorted`: Keep lists sorted.
    - `keep-unique`: Ensure no duplicates.
    - `line-pattern`: Validate lines against Regex.
    - `line-count`: Enforce block size limits.
- 🤖 **AI Validation**: Use natural language rules to validate code or docs (e.g., "Must mention 'banana'").
- 🌍 **Language Agnostic**: Works with almost any language (Rust, Python, JS, Go, Markdown, YAML, etc.).
- 🚀 **Flexible Execution**: Run on specific files, glob patterns, or git diffs.

[//]: # (</block>)

## Installation

### Homebrew (macOS/Linux)

```shell
brew tap mennanov/tap
brew install blockwatch
```

### From Source (Rust)

```shell
cargo install blockwatch
```

### Prebuilt Binaries

Download from [Releases](https://github.com/mennanov/blockwatch/releases).

## Usage

### 1. Scan Your Project

Validate all blocks in your project:

```shell
# Check all files (defaults to "**")
blockwatch

# Check specific file types
blockwatch "src/**/*.rs" "**/*.md"

# Ignore specific files
blockwatch "**/*.rs" --ignore "**/generated/**"
```

> **Note:** Always quote glob patterns to prevent shell expansion.

### 2. Check Modified Files (CI / Hooks)

Pipe a git diff to validate only changed blocks or blocks affected by changes:

```shell
# Check unstaged changes
git diff --patch | blockwatch

# Check staged changes
git diff --cached --patch | blockwatch

# Check changes in a specific file only
git diff --patch path/to/file | blockwatch

# Check changes and some other (possibly unchanged) files
git diff --patch | blockwatch "src/always_checked.rs" "**/*.md"
```

### 3. CI Integration

**Pre-commit Hook**:
Add to `.pre-commit-config.yaml`:

```yaml
- repo: local
  hooks:
    - id: blockwatch
      name: blockwatch
      entry: bash -c 'git diff --patch --cached --unified=0 | blockwatch'
      language: system
      stages: [ pre-commit ]
      pass_filenames: false
```

**GitHub Action**:
Add to `.github/workflows/your_workflow.yml`:

```yaml
- uses: mennanov/blockwatch-action@v1
```

## Validators

Blocks are defined using XML-like tags in comments.

### Linking Code & Docs (`affects`)

Ensure that when code changes, the documentation is updated.

**src/lib.rs**:

```rust
// <block affects="README.md:supported-langs">
pub enum Language {
    Rust,
    Python,
}
// </block>
```

**README.html**:

```html
<!-- <block name="supported-langs"> -->

- Rust
- Python

<!-- </block> -->
```

If you change `src/lib.rs`, BlockWatch will complain if you don't also update `README.md`.

### Enforcing Sort Order (`keep-sorted`)

Sort lines alphabetically (`asc` or `desc`).

```python
# <block keep-sorted="asc">
"apple",
"banana",
"cherry",
# </block>
```

**Advanced: Sort by Regex Match**
Use `keep-sorted-pattern` to sort by a specific part of the line.

- If a named capture group `value` exists, it is used for sorting.
- Otherwise, the entire match is used.

```python
items = [
    # <block keep-sorted="asc" keep-sorted-pattern="id: (?P<value>\d+)">
    "id: 1  apple",
    "id: 2  banana",
    "id: 10 orange",
    # </block>
]
```

### Enforcing Uniqueness (`keep-unique`)

Ensure no duplicate lines.

```python
# <block keep-unique>
"user_1",
"user_2",
"user_3",
# </block>
```

**Advanced: Uniqueness by Regex**
Use the attribute value as a regex to determine uniqueness.

- If a named capture group `value` exists, it is used for comparison.
- Otherwise, the entire match is used.

```python
ids = [
    # <block keep-unique="^ID:(?P<value>\d+)">
    "ID:1 Alice",
    "ID:2 Bob",
    "ID:1 Carol",  # Violation: ID:1 is already used
    # </block>
]
```

### Regex Validation (`line-pattern`)

Ensure every line matches a specific regex pattern.

```python
slugs = [
    # <block line-pattern="^[a-z0-9-]+$">
    "valid-slug",
    "another-one",
    # </block>
]
```

### Line Count (`line-count`)

Enforce the number of lines in a block.
Supported operators: `<`, `>`, `<=`, `>=`, `==`.

```python
# <block line-count="<=5">
"a",
"b",
"c"
# </block>
```

### AI Validation (`check-ai`)

Validate logic or style using an LLM.

```html
<!-- <block check-ai="Must mention the company name 'Acme Corp'"> -->
<p>Welcome to Acme Corp!</p>
<!-- </block> -->
```

#### Advanced: Extract Content for AI

Use `check-ai-pattern` to send only relevant parts of the text to the LLM.

```python
prices = [
    # <block check-ai="Prices must be under $100" check-ai-pattern="\$(?P<value>\d+)">
    "Item A: $50",
    "Item B: $150",  # Violation
    # </block>
]
```

#### `check-ai` configuration

[//]: # (<block name="check-ai-env-vars">)

- `BLOCKWATCH_AI_API_KEY`: API Key (OpenAI compatible).
- `BLOCKWATCH_AI_MODEL`: Model name (default: `gpt-5-nano`).
- `BLOCKWATCH_AI_API_URL`: Custom API URL (optional).

[//]: # (</block>)

## Supported Languages

BlockWatch supports comments in:

[//]: # (<block name="supported-grammar" keep-sorted="asc">)

- Bash
- C#
- C/C++
- CSS
- Go
- HTML
- Java
- JavaScript
- Kotlin
- Markdown
- PHP
- Python
- Ruby
- Rust
- SQL
- Swift
- TOML
- TypeScript
- XML
- YAML

[//]: # (</block>)

## Configuration

[//]: # (<block name="cli-docs">)

- **Extensions**: Map custom extensions: `blockwatch -E cxx=cpp`
- **Disable Validators**: `blockwatch -d check-ai`
- **Enable Validators**: `blockwatch -e keep-sorted`
- **Ignore Files**: Ignore files matching glob patterns: `blockwatch --ignore "**/generated/**"`

[//]: # (</block>)

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
