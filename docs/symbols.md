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

A [virtual block](cli.md#blocks-in-the-config-file) can wrap a symbol too. It is a block that the config file
declares, instead of tags in a comment.

## Files with symbols

<!-- <block name="extensions-with-symbols" same-as="src/language_parsers/mod.rs:extensions-with-symbols"
     same-as-pattern="`\.?(?P<value>[A-Za-z]+)`"> -->

| Language | Files                                                    | Paths                 |
|----------|----------------------------------------------------------|-----------------------|
| JSON     | `.json`, `.jsonc`, `.webmanifest`                        | [JSON](#json)         |
| TOML     | `.toml`, `Pipfile`                                       | [TOML](#toml)         |
| YAML     | `.yaml`, `.yml`, `.cff`                                  | [YAML](#yaml)         |
| Markdown | `.md`, `.markdown`, `.mdx`                               | [Markdown](#markdown) |
| Go       | `.go`                                                    | [Go](#go)             |
| C, C++   | `.c`, `.h`, `.cc`, `.cpp`, `.cxx`, `.hh`, `.hpp`, `.hxx` | [C and C++](#c-and-c) |

<!-- </block> -->

Other files have no symbols yet. blockwatch still reads their blocks: see
[Supported Languages](../README.md#supported-languages). A path into one of them stops the run.

A file you map to a language with `-E` gets that language's symbols. With `-E avsc=json`, `user.avsc#/name` works.

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

## Picking an item by a key's value

A position breaks when someone adds an item above the one you mean. A **filter** picks the item by what it holds
instead: `~[key=value]` is the one item whose `key` holds `value`.

```yaml
services:
  - name: web
    port: 8080
  - name: worker
    port: 9090
```

| Path                             | Points at |
|----------------------------------|-----------|
| `#/services/1/port`              | `9090`    |
| `#/services/~[name=worker]/port` | `9090`    |

- **Exactly one item must match.** No match is like a missing key. When the items have the key, the message suggests
  the values that only one item holds: `did you mean: /services/~[name=worker]/port`. Several matches are a violation
  too: `2 items match ~[name=web], at 2:5, 6:5`.
- **The value compares as a symbol reads it:** a string without its quotes, any other value as written. So
  `~[port=8080]` matches `port: 8080` and `port: "8080"`.
- **Only a key right under the item counts.** A `name` deeper inside the item does not make it match.
- **A filter picks a map entry too.** In `{"a": {"name": "x"}}`, `#/~[name=x]` is `#/a`.
- **A path can hold several filters**, such as `#/groups/~[name=api]/services/~[name=web]/port`. A filter can come
  first, in a file that is a list: `#/~[name=web]`.
- **The value starts after the first `=` and ends at the last `]`,** so it can hold both. The key can't hold `=`.
  Inside a filter, escape the key and the value like any key: `/` as `~1`, `,` as `%2C`, and so on.

Keys that look like filters still work. `[python]`, as in VS Code settings, needs no escape. A key that starts with
`~[` is written `~0[`, as RFC 6901 already requires.

## When a path does not resolve

- **A missing key is a violation.** The message suggests similar paths: `symbol not found; did you mean: /version`.
- **A broken file stops the run.** blockwatch does not guess what a broken file meant. In a code file, only a path to
  a name near the error stops it: see [Code files](#code-files).
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

## Markdown

<!-- <block name="markdown-paths"> -->

Every section is a symbol. A section is a heading and everything up to the next heading of the same or a higher level.
Its path is the text of each heading it is under, then its own:

```markdown
# Guide

## Install

Run `cargo install blockwatch`.

### From source

Clone the repository.
```

| Path                          | Points at                                   |
|-------------------------------|---------------------------------------------|
| `#/Guide`                     | everything below `# Guide`                  |
| `#/Guide/Install`             | `Run …` and the whole `From source` section |
| `#/Guide/Install/From source` | `Clone the repository.`                     |

- **The top `#` title is part of every path**, as above.
- **A section holds its subsections.** `affects` counts a change to any of its lines, the heading included.
- **A section's value is its text without the heading line.** `same-as` compares that text. The blank lines around it
  don't count.
- **A heading counts as a reader sees it.** ``## The `--diff` flag`` is `#/The --diff flag`. Code, emphasis and links
  lose their markup.
- **Headings underlined with `===` or `---` count.** A `#` line in a code block doesn't. Neither does a heading in a
  quote, a list item or an HTML block.
- **The same heading twice under one parent is a violation**, because blockwatch can't tell which one you mean. Two
  `### Example` sections under different `##` headings are fine.
- **Escape a heading like any key.** `## Input/Output` is `#/Input~1Output`, and `## Step 1: build` is
  `#/Step 1%3A build`.
- **An `.mdx` file gets the same paths.** The text of a JSX element in a heading counts, and so does the code in
  braces: `## Red <Badge>new</Badge> {version}` is `#/Red new version`. A heading inside a JSX element doesn't count.
  A file that isn't valid MDX stops the run.

<!-- </block> -->

## Code files

<!-- <block name="code-files"> -->

In a code file, each top-level name is a symbol: a constant, a variable, a type, a function or a macro. Its path is
`#/NAME`.
With `const MaxPorts = 8` in `consts.go`, `consts.go#/MaxPorts` points at `8`.

- **A value is the initializer as written.** blockwatch never works it out: `20 * 1024 * 1024` stays as it is, and so
  does `0x10`. Use `same-as-pattern` to take the part you need.
- **A string counts as the text it holds.** Quotes and escapes are removed.
- **Parentheses around the whole value are removed.** `(32)` is `32`. In `(1) + (2)`, they stay.
- **A name without a value**, such as a function or a type, gives the text of its whole definition.
- **A symbol covers its own definition**, keywords included, but not the comments above it. `affects` counts a change
  to that text only.
- **A name defined twice is a violation**, because blockwatch can't tell which copy you mean.
- **A syntax error stops the run only for the names near it**: a name whose definition holds the error, and any name in
  the text that the parser had to skip. Other names in the file still resolve. The message shows where the error is.
- **A string that blockwatch can't decode stops the run** for its name only.
- **Members are not symbols yet**, such as a method or a field.

<!-- </block> -->

## Go

<!-- <block name="go-paths"> -->

```go
const ExprStatusBits = 4

const (
    DNSPortsMax = 8
    ModeA Mode = iota
    ModeB
)

var Name, Port = "web", 8080

type Server struct{ port int }

func Helper() {}
```

| Path               | Points at                                  |
|--------------------|--------------------------------------------|
| `#/ExprStatusBits` | `4`                                        |
| `#/DNSPortsMax`    | `8`                                        |
| `#/ModeB`          | `ModeB`, its line, as it has no value      |
| `#/Port`           | `8080`                                     |
| `#/Server`         | the whole `type Server struct{ port int }` |
| `#/Helper`         | the whole function                         |

- **In a group, such as `const ( ... )`, a name covers only its own line.** Outside a group, it covers the whole
  declaration, `const` included.
- **Names get their values by position.** In `var x, y = f()`, one call gives both names, so neither has a value.
- **A type is not part of a value.** `const X Mode = 3` is `3`.
- **Strings follow Go's escapes.** `"\xc3\xa9"` is `é`. A raw string loses its carriage returns. A rune, such as `'r'`,
  stays as written. A string whose bytes are not UTF-8 text stops the run for its name.
- **Methods, `func init` and `_` are not symbols.**
- **`go.mod`, `go.sum` and `go.work` have no symbols.**

<!-- </block> -->

## C and C++

<!-- <block name="c-paths"> -->

```c
#define DNS_PORTS_MAX 8
#define FLAGS (1u << 4) // bits
#define NAME "ab" "cd"

static const int Retries = 3;

typedef enum throttle_mode { THROTTLE_A, THROTTLE_B = 2 } throttle_mode_t;

int helper(void) { return 0; }
```

| Path                | Points at                                        |
|---------------------|--------------------------------------------------|
| `#/DNS_PORTS_MAX`   | `8`                                              |
| `#/FLAGS`           | `1u << 4`                                        |
| `#/NAME`            | `abcd`                                           |
| `#/Retries`         | `3`                                              |
| `#/throttle_mode_t` | the whole `typedef`, as does `#/throttle_mode`   |
| `#/THROTTLE_B`      | `2`                                              |
| `#/helper`          | the whole function                               |

- **A macro is a symbol wherever it is defined**, even inside an `#if` or a function. Its body reads like a variable's
  value, without comments. A body over several lines keeps its backslashes. A body that is not one value, such as
  `static inline`, is kept as written. `#define FOO_H` is the empty string. A macro with parameters has no value.
- **A macro defined in each branch of an `#if` is defined twice.** Wrap the copy you mean in a block instead.
- **A `/* */` comment inside a macro, such as `#define X 1 /* c */ + 2`, stops the run for that macro.** The parser
  reads the rest of the line as code. A comment at the end of the line is fine.
- **The enumerators of a plain `enum` are top-level names**, as in C. Those of an `enum class` are not.
- **In `int a = 1, b = 2;`, a name covers only its own part.** Otherwise a name covers the whole declaration.
- **Prototypes and `extern` variables are not symbols.** The definition is.
- **`template <...>` and `extern "C"` are part of the definition.** The contents of `extern "C" { ... }` are top-level.
- **Strings follow C's escapes, and strings written one after the other are joined.** A raw string, `R"(...)"`, is its
  text. `L`, `u` and `U` strings and character literals stay as written.
- **Namespaces, class members and names such as `Foo::bar` are not symbols yet.**

<!-- </block> -->

---

[← Return to README](../README.md)
