# Symbols

A **symbol** is one value inside a file, such as the `version` key in `package.json`. A block points at a symbol with
a target like `package.json#/version`. The part after `#` is the symbol's **path**. A target like `#/version` points
into the block's own file.

```rust
// <block same-as="package.json#/version">
pub const VERSION: &str = "1.4.2";
// </block>
```

Three validators use symbols:

- [`same-as`](validators/same-as.md#symbols-as-targets) compares a block with a symbol's value.
- [`affects`](validators/affects.md#symbols) counts a symbol as changed only when the diff touches it.
- [`check-lua`](validators/check-lua.md) gives a script the value of each symbol the block's `affects` points at.

A [virtual block](cli.md#blocks-in-the-config-file) wraps a symbol too. It is a block that the config file declares
around a symbol, instead of tags in a comment.

## Files with symbols

<!-- <block name="extensions-with-symbols" same-as="src/language_parsers/mod.rs:extensions-with-symbols"
     same-as-pattern="`\.(?P<value>[a-z]+)`"> -->

| Language | Files             | Paths         |
|----------|-------------------|---------------|
| JSON     | `.json`, `.jsonc` | [JSON](#json) |
| TOML     | `.toml`           | [TOML](#toml) |
| YAML     | `.yaml`, `.yml`   | [YAML](#yaml) |

<!-- </block> -->

Other files have no symbols yet. blockwatch still reads their blocks: see
[Supported Languages](../README.md#supported-languages). A path into one of them stops the run.

A file you map to a language with `-E` gets that language's symbols. With `-E webmanifest=json`,
`site.webmanifest#/name` works.

## Writing a path

Paths follow [RFC 6901](https://www.rfc-editor.org/rfc/rfc6901), also called JSON Pointer:

- `/` separates keys: `#/dependencies/inngest`.
- An array item is its position, counted from 0: `#/files/0`.

Some characters must be escaped when a key contains them:

| Character | Escape | Why                                   |
|-----------|--------|---------------------------------------|
| `/`       | `~1`   | it separates keys                     |
| `~`       | `~0`   | it starts an escape                   |
| `,`       | `%2C`  | it separates targets                  |
| `:`       | `%3A`  | it separates a file from a block name |
| `%`       | `%25`  | it starts an escape                   |

For example, the key `@types/node` is `#/dependencies/@types~1node`.

## When a path does not resolve

- **A missing key is a violation.** The message suggests similar paths: `symbol not found; did you mean: /version`.
- **A broken file stops the run.** blockwatch does not guess what a broken file meant.
- **A missing file stops the run**, and so does a file without symbols.

A virtual block's target is stricter: a missing key, or a key written twice, stops the run too. Without one symbol to
wrap, the block has no content.

## JSON

<!-- <block name="json-paths"> -->

Every key and every array item is a symbol:

```json
{
  "name": "app",
  "files": ["dist", "README.md"],
  "scripts": { "build": "tsc" }
}
```

| Path              | Points at       |
|-------------------|-----------------|
| `#/name`          | `"app"`         |
| `#/files`         | the whole array |
| `#/files/1`       | `"README.md"`   |
| `#/scripts/build` | `"tsc"`         |

- **A file can be an array.** Then `#/0` is its first item.
- **A comment is not an array item.** It does not shift the positions of the items after it.
- **Comments are allowed** in `.json` and `.jsonc` alike. A trailing comma is an error in both, so it stops the run.
- **A key written twice is a violation**, because blockwatch can't tell which copy you mean. The message shows where
  each copy is: `ambiguous symbol, defined at 2:3, 5:3`.

<!-- </block> -->

## TOML

<!-- <block name="toml-paths"> -->

Every key, every table and every array item is a symbol. A path follows the keys, however the file writes them:

```python
# <block same-as="pyproject.toml#/project/version" same-as-pattern="\d+\.\d+\.\d+">
__version__ = "0.8.4"
# </block>
```

This works whether `pyproject.toml` has `version = "0.8.4"` under `[project]`, or `project.version = "0.8.4"` at the
top.

- **Headers and dotted keys make one path.** `[tool.ruff]` with `line-length = 88` below it is
  `#/tool/ruff/line-length`, and so is `tool.ruff.line-length = 88`.
- **A quoted key is one part of the path**, dots and all. Under `[a]`, `"b.c" = 1` is `#/a/b.c`.
- **Each `[[bin]]` entry has a position**, counted from 0: `#/bin/0/name`. Entries of other arrays, written in between,
  don't count.
- **A sub-table belongs to the entry above it.** `[fruits.physical]` after the first `[[fruits]]` is
  `#/fruits/0/physical`.
- **A table covers every place it is written.** `#/package` covers `[package]` and `[package.metadata.docs]`, even
  with other tables in between.
- **A key written twice breaks the file.** TOML does not allow it, so it stops the run.

<!-- </block> -->

## YAML

<!-- <block name="yaml-paths"> -->

Every key and every list item is a symbol. Block style and flow style give the same paths:

```yaml
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: cargo test
```

| Path                      | Points at        |
|---------------------------|------------------|
| `#/jobs/test/runs-on`     | `ubuntu-latest`  |
| `#/jobs/test/steps`       | the whole list   |
| `#/jobs/test/steps/1/run` | `cargo test`     |

- **A string counts as the text it holds.** Quotes and escapes are removed. A string written on several lines, `|` and
  `>` blocks included, counts as YAML reads it.
- **Tags and anchors are not part of a value.** `!!str 123` gives `123`, and `&default 5` gives `5`.
- **An empty value**, as after `key:`, is an empty string.
- **An alias (`*name`) is not a symbol.** What it stands for is written elsewhere, so a path to it or through it is not
  found. In a list, an alias still takes its position. `<<` is an ordinary key.
- **A key that is a list or a mapping,** such as `? [a, b]`, is skipped with everything under it.
- **A key written twice is a violation**, because blockwatch can't tell which copy you mean.
- **A file with several documents stops the run.** A path can't say which document it means. Put the value in a named
  block instead.

<!-- </block> -->

---

[← Return to README](../README.md)
