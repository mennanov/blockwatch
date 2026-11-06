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
     same-as-pattern="`\.?(?P<value>[A-Za-z._]+)`"> -->

| Language         | Files                                                                                                       | Paths                                                   |
|------------------|-------------------------------------------------------------------------------------------------------------|---------------------------------------------------------|
| JSON             | `.json`, `.jsonc`, `.webmanifest`                                                                           | [JSON](#json)                                           |
| TOML             | `.toml`, `Pipfile`                                                                                          | [TOML](#toml)                                           |
| YAML             | `.yaml`, `.yml`, `.cff`                                                                                     | [YAML](#yaml)                                           |
| Markdown         | `.md`, `.markdown`, `.mdx`                                                                                  | [Markdown](#markdown)                                   |
| XML              | `.xml`, `.csproj`, `.fsproj`, `.vbproj`, `.vcxproj`, `.props`, `.targets`, `.xaml`, `.xsd`, `.xsl`, `.xslt` | [XML](#xml)                                             |
| CSS              | `.css`                                                                                                      | [CSS](#css)                                             |
| Go               | `.go`                                                                                                       | [Go](#go)                                               |
| C, C++           | `.c`, `.h`, `.cc`, `.cpp`, `.cxx`, `.hh`, `.hpp`, `.hxx`                                                    | [C and C++](#c-and-c)                                   |
| Python           | `.py`, `.pyi`, `.pyw`                                                                                       | [Python and Starlark](#python-and-starlark)             |
| Starlark         | `.bzl`, `.bazel`, `.bzlmod`, `.star`, `BUILD`, `BUCK`, `WORKSPACE`, `Tiltfile`                              | [Python and Starlark](#python-and-starlark)             |
| Rust             | `.rs`                                                                                                       | [Rust](#rust)                                           |
| TypeScript       | `.ts`, `.mts`, `.cts`, `.d.ts`, `.tsx`                                                                      | [TypeScript and JavaScript](#typescript-and-javascript) |
| JavaScript       | `.js`, `.mjs`, `.cjs`, `.jsx`                                                                               | [TypeScript and JavaScript](#typescript-and-javascript) |
| Java             | `.java`                                                                                                     | [Java](#java)                                           |
| C#               | `.cs`                                                                                                       | [C#](#c)                                                |
| Kotlin           | `.kt`, `.kts`                                                                                               | [Kotlin](#kotlin)                                       |
| Swift            | `.swift`                                                                                                    | [Swift](#swift)                                         |
| Protocol Buffers | `.proto`                                                                                                    | [Protocol Buffers](#protocol-buffers)                   |
| HCL (Terraform)  | `.tf`, `.tofu`, `.tfvars`, `.hcl`, `.nomad`                                                                 | [HCL](#hcl)                                             |
| PHP              | `.php`, `.phtml`                                                                                            | [PHP](#php)                                             |
| Shell            | `.sh`, `.bash`, `.bashrc`, `.bash_profile`, `.envrc`                                                        | [Shell](#shell)                                         |
| Makefile         | `Makefile`, `GNUmakefile`, `makefile`, `.mk`, `.mak`                                                        | [Makefile](#makefile)                                   |
| Dart             | `.dart`                                                                                                     | [Dart](#dart)                                           |

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
- **When several items share a path, the rest of the path looks only inside the item the filter picked.** For
  example, two `ingress` blocks in an HCL resource share a path.
  `#/resource/aws_security_group/web/~[from_port=443]/to_port` is the `to_port` of the block whose `from_port` is
  `443`.
- **A filter that reaches an unreadable path stops the run**, as that path does. An item blockwatch can't read, such
  as one with a syntax error, also stops a filter that compares its key, because that item could match too.
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
- **Escape a heading like any key.** `## Input/Output` is `#/Input~1Output`.
- **An `.mdx` file gets the same paths.** The text of a JSX element in a heading counts, and so does the code in
  braces: `## Red <Badge>new</Badge> {version}` is `#/Red new version`. A heading inside a JSX element doesn't count.
  A file that isn't valid MDX stops the run.

<!-- </block> -->

## XML

`.xml` files have symbols, and so do the MSBuild files (`.csproj`, `.fsproj`, `.vbproj`, `.vcxproj`, `.props`,
`.targets`), `.xaml`, `.xsd`, `.xsl` and `.xslt`.

<!-- <block name="xml-paths"> -->

Every element and every attribute is a symbol. A path is each element's name, from the root element down. An attribute
is `@` and its name, under its element:

```xml
<project xmlns="http://maven.apache.org/POM/4.0.0">
  <version>1.4.2</version>
  <dependencies>
    <dependency>
      <artifactId>junit</artifactId>
      <version>5.10.0</version>
    </dependency>
    <dependency scope="test">
      <artifactId>guava</artifactId>
      <version>33.0.0-jre</version>
    </dependency>
  </dependencies>
  <description>
    Tools &amp; helpers
  </description>
</project>
```

| Path                                                 | Points at                       |
|------------------------------------------------------|---------------------------------|
| `#/project/version`                                  | `1.4.2`                         |
| `#/project/dependencies`                             | the whole element               |
| `#/project/dependencies/dependency/version`          | a violation: two elements match |
| `#/project/dependencies/~[artifactId=guava]/version` | `33.0.0-jre`                    |
| `#/project/dependencies/~[artifactId=guava]/@scope`  | `test`                          |
| `#/project/description`                              | `Tools & helpers`               |

- **Elements with one name under one parent share a path**, such as the two `dependency` elements above. A path through
  them is a violation, so pick one with a [filter](#picking-an-item-by-a-keys-value). Its key can be a child element,
  such as `artifactId`, or an attribute. In a `.csproj` file, `#/Project/ItemGroup/~[@Include=Serilog]/@Version` is
  the `Version` of the `PackageReference` whose `Include` is `Serilog`. A position, such as
  `#/project/modules/module/1`, is not found.
- **A prefix is part of the name, as written:** `#/manifest/application/@android:name`. Namespaces play no part,
  and `xmlns` attributes are not symbols. Names compare as written, case included.
- **An element without child elements is its text.** Entities and character references are decoded, CDATA stays as
  written, and comments are left out. The white space around the text is removed, but the line breaks and indents
  inside it stay. `<a/>` and `<a></a>` are empty strings.
- **An element with child elements has no value.** It gives its whole text, tags included, even when it also holds
  text.
- **An attribute is its decoded value.** Each tab and line break in it becomes a space, as XML requires. The white
  space around it stays.
- **An entity that the DOCTYPE declares stops the run for its path**, because it can stand for any text, even
  elements. So does a reference to a character that XML forbids, such as `&#0;`.
- **The XML declaration, the DOCTYPE, comments and processing instructions are not symbols.**
- **An element covers its start tag, its content and its end tag.** An attribute covers `name="value"`. A comment
  above an element is not part of it.
- **A syntax error anywhere stops the run**, and so does an empty file. A few valid files stop it too, because of the
  parser: a DOCTYPE without white space before its `]`, as in `[<!ENTITY e "x">]`, and a processing instruction with
  data, such as `<?pi x?>`, inside an element.

<!-- </block> -->

## CSS

`.css` files have symbols.

<!-- <block name="css-paths"> -->

Each rule is a symbol, and so is each at-rule with a block and each declaration in them. A path is the selector or
at-rule of each rule, from the top down, then a property:

```css
:root {
  --brand: #5097ba;
  --gap: 8px !important;
}

.dark { --brand: #ffffff; }

@media (max-width: 768px) {
  .card { padding: 4px; }
}

.card {
  display: -webkit-box;
  display: flex;
  content: "a\"b";
  &:hover { color: blue; }
}

h1, h2 { margin: 0; }

@keyframes spin { to { rotate: 1turn; } }
```

| Path                                        | Points at                                 |
|---------------------------------------------|-------------------------------------------|
| `#/:root/--brand`                           | `#5097ba`                                 |
| `#/:root/--gap`                             | `8px`, without `!important`               |
| `#/.dark/--brand`                           | `#ffffff`                                 |
| `#/--brand`                                 | not found: a property needs its rule      |
| `#/@media (max-width: 768px)/.card/padding` | `4px`                                     |
| `#/.card/padding`                           | not found: only the `@media` rule sets it |
| `#/.card/display`                           | a violation: it is set twice              |
| `#/.card/content`                           | `a"b`                                     |
| `#/.card/&:hover/color`                     | `blue`                                    |
| `#/.card`                                   | the whole rule                            |
| `#/h1%2C h2/margin`                         | `0`                                       |
| `#/@keyframes spin/to/rotate`               | `1turn`                                   |

- **A segment is the selector or the at-rule as written.** Comments are left out, and each run of white space is one
  space: `.card  >  a` is `#/.card > a`. An at-rule is its name and its prelude, such as `@media (max-width: 768px)`.
  `@supports`, `@layer`, `@container`, `@font-face`, Tailwind's `@theme` and any other at-rule with a block work the
  same way.
- **A selector list is one segment.** `h1, h2 { }` is `#/h1%2C h2`, with its `,` escaped. `#/h2` doesn't find it.
- **Escape `~` and `/` as in any key.** `a ~ b` is `#/a ~0 b`, and `@media (aspect-ratio: 16/9)` is
  `#/@media (aspect-ratio: 16~19)`.
- **A property needs its whole path.** `#/--brand` is not found. Write `#/:root/--brand`.
- **A selector or a property written twice is a violation**, though CSS uses the last one. A fallback such as
  `display: -webkit-box; display: flex;` does this, and so does one selector in two rules. Wrap the one you mean in a
  block instead.
- **Names compare as written, case included.** `#/.a/color` doesn't find `COLOR`. A position, such as `#/.card/0`, is
  not found.
- **A value runs from its first token to its last one.** The white space and comments around it are left out, and so
  is `!important` at its end. A comment between its tokens stays.
- **A value that is one string is decoded:** `"a\"b"` is `a"b`. Any other value stays as written, strings included,
  such as `"Inter", sans-serif`.
- **A rule covers its selector and its block. A declaration covers `name: value`, with `!important`.** A comment above
  either is not part of it.
- **At-rules without a block, such as `@import`, are not symbols.** Neither are `<style>` elements in HTML, nor
  `.scss`, `.sass` and `.less` files.
- **A part that CSS says to skip stops the run for each path with a word from it as a segment.** The IE hack
  `*zoom: 1` stops `#/.btn/zoom`. A declaration outside a rule, or a stray `}`, is skipped with the rule after it.
  Other paths in the file still resolve.

<!-- </block> -->

## Code files

<!-- <block name="code-files"> -->

In a code file, each top-level name is a symbol: a constant, a variable, a type, a function or a macro. Its path is
`#/NAME`.
With `const MaxPorts = 8` in `consts.go`, `consts.go#/MaxPorts` points at `8`. In Java, C#, Kotlin, Swift, Protocol
Buffers, PHP and Dart, the members of a type are symbols too, under the type's name: `Config.java#/Config/MAX_PORTS`.
In HCL, each block, attribute and object key is a symbol: `main.tf#/variable/region/default`.

- **A value is the initializer as written.** blockwatch never works it out: `20 * 1024 * 1024` stays as it is, and so
  does `0x10`. Use `same-as-pattern` to take the part you need.
- **A string counts as the text it holds.** Quotes and escapes are removed.
- **Parentheses around the whole value are removed.** `(32)` is `32`. In `(1) + (2)`, they stay.
- **A name without a value**, such as a function or a type, gives the text of its whole definition.
- **A symbol covers its own definition**, keywords included, but not the comments above it. `affects` counts a change
  to that text only.
- **A name defined twice is a violation**, because blockwatch can't tell which copy you mean.
- **A syntax error stops the run only for the names near it**: a name whose definition holds the error, and every path
  with a name from the text that the parser had to skip. Other names in the file still resolve. The message shows
  where the error is.
- **A path to a string that blockwatch can't decode stops the run.** Other paths in the file still resolve.
- **Members are not symbols yet**, such as a method or a field, except in Java, C#, Kotlin, Swift, Protocol Buffers,
  HCL, PHP and Dart.

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
  stays as written. A path to a string whose bytes are not UTF-8 text stops the run.
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
  text, with each `\r\n` line end read as `\n`. `L`, `u` and `U` strings and character literals stay as written.
- **Namespaces, class members and names such as `Foo::bar` are not symbols yet.**

<!-- </block> -->

## Python and Starlark

<!-- <block name="python-paths"> -->

```python
PYTHON_VERSION = "3.12"
TIMEOUT: Final[int] = 30
A = B = 5

if TYPE_CHECKING:
    MODE = "check"
else:
    MODE = "run"

@dataclass
class Point:
    x: int = 0

def load(path):
    return path
```

| Path               | Points at                                  |
|--------------------|--------------------------------------------|
| `#/PYTHON_VERSION` | `3.12`                                     |
| `#/TIMEOUT`        | `30`                                       |
| `#/B`              | `5`, as does `#/A`                         |
| `#/MODE`           | a violation, as `MODE` is defined twice    |
| `#/Point`          | the whole class, `@dataclass` included     |
| `#/load`           | the whole function                         |

- **The bodies of a top-level `if`, `try` or `with` are top-level too**, as Python treats them. So a name defined in
  two branches is defined twice. The bodies of `for`, `while` and `match` don't count.
- **`A = B = 5` gives both names the whole statement.** `X: int` has no value. `X += [...]` doesn't change the value
  of `X`.
- **A decorator is part of the definition.**
- **Imports, `as` targets, `a, b = 1, 2`, `obj.x = 1` and `_` are not symbols.** A type alias is one, without a
  value: `type Pair[T] = tuple[T, T]` is `#/Pair`.
- **Strings follow Python's escapes.** `"\xe9"` is `é`. An escape Python doesn't know, such as `\d`, stays as written.
  A raw string is its text. An f-string without a field is its text, with `{{` as `{`. An f-string with a field and
  bytes stay as written. A path to a string with a `\N{...}` escape stops the run, and so does a path to a string
  with an escape that Python rejects, such as `"\x4"` or `"\N"`.
- **Starlark files, such as `BUILD` and `.bzl`, follow the same rules.** A rule call, such as
  `cc_library(name = "lib")`, is not a symbol.
- **Class members are not symbols yet.**

<!-- </block> -->

## Rust

<!-- <block name="rust-paths"> -->

```rust
pub(crate) const MAX_MESSAGE_SIZE: usize = 20 * 1024 * 1024;
static NAMES: [&str; 2] = ["a", "b"];

#[cfg(unix)]
const PLATFORM: &str = "unix";
#[cfg(windows)]
const PLATFORM: &str = "windows";

#[derive(Debug)]
pub enum LogLevel {
    Error,
}

fn run() {}
```

| Path                 | Points at                                     |
|----------------------|-----------------------------------------------|
| `#/MAX_MESSAGE_SIZE` | `20 * 1024 * 1024`                            |
| `#/NAMES`            | `["a", "b"]`                                  |
| `#/PLATFORM`         | a violation, as `PLATFORM` is defined twice   |
| `#/LogLevel`         | the whole enum, `#[derive(Debug)]` included   |
| `#/run`              | the whole function                            |

- **Constants, statics, functions, types, traits, `macro_rules!` macros and modules are symbols.** A type is not part
  of a value: `const X: u8 = 3` is `3`.
- **The attributes above an item are part of it**, such as `#[cfg(...)]`. Its doc comments are not.
- **A module with a body, such as `mod tests { ... }`, is a symbol, but the items in it are not yet.** `mod file;` is
  not a symbol.
- **`impl` blocks, `use`, `extern` blocks, `const _` and items that a macro makes, such as in `lazy_static!`, are not
  symbols.**
- **Strings follow Rust's escapes.** `"\u{e9}"` is `é`. A backslash at the end of a line also skips the spaces that
  start the next line. A raw string is its text. Byte strings, C strings and characters stay as written.

<!-- </block> -->

## TypeScript and JavaScript

<!-- <block name="typescript-paths"> -->

```ts
export const MAX_RETRIES = 5;
const MODE = "dev" as const;
const A = 1, B = 2;

@sealed
export class Point {}

type Mode = "dev" | "prod";

function normalize(text: string): string {
  return text;
}
```

| Path             | Points at                                  |
|------------------|--------------------------------------------|
| `#/MAX_RETRIES`  | `5`                                        |
| `#/MODE`         | `dev`                                      |
| `#/B`            | `2`, and covers only `B = 2`               |
| `#/Point`        | the whole class, `@sealed` included        |
| `#/Mode`         | the whole type                             |
| `#/normalize`    | the whole function                         |

- **`const`, `let`, `var`, functions, classes, enums, interfaces, type aliases and namespaces are symbols.**
  `export default function main() {}` is `#/main`.
- **Type syntax is not part of a value.** `"dev" as const` is `dev`, `x satisfies T` is `x`, and `x!` is `x`.
- **A function with overloads is its implementation**, the one with a body.
- **`declare const`, `declare function`, `declare module "x"` and `declare global` are not symbols.** They only state
  types. A `declare class`, `enum` or `namespace` is one.
- **Imports, `export { ... }`, `const { a } = b`, `module.exports`, an anonymous `export default` and `_` are not
  symbols.**
- **Strings follow JavaScript's escapes.** `"\uD83D\uDE00"` is `😀`. A template without `${...}` is its text. A template
  with a field and a tagged template, such as ``String.raw`a` ``, stay as written. A path to a string with a legacy
  escape, such as `\07`, stops the run.
- **Enum members, class members and the contents of a namespace are not symbols yet.**

<!-- </block> -->

## Java

<!-- <block name="java-paths"> -->

```java
public class Config {
    @Native
    public static final int MAX_RETRIES = 20 * 1024;
    static final String QUERY = """
        SELECT *
        FROM t
        """;

    void run() {}
    void run(int times) {}

    enum Mode { RED, GREEN }

    static class Limits {
        static final long ID = 0x10L;
    }
}
```

| Path                   | Points at                                     |
|------------------------|-----------------------------------------------|
| `#/Config`             | the whole class                               |
| `#/Config/MAX_RETRIES` | `20 * 1024`                                   |
| `#/Config/QUERY`       | `SELECT *` and `FROM t`, on two lines         |
| `#/Config/run`         | a violation, as `run` is defined twice        |
| `#/Config/Mode/GREEN`  | `GREEN`, as it has no value                   |
| `#/Config/Limits/ID`   | `0x10L`                                       |

- **Each type is a symbol, and so is each member in it**: a field, a method, an enum constant, an annotation element
  or a nested type. A path goes from the top-level type down, as code refers to it: `#/Config/Limits/ID`.
- **Overloads are defined twice, and so are a field and a method with one name**, such as `name` and `name()`. Wrap the
  one you mean in a block instead.
- **Annotations are part of a definition. Javadoc is not.** In `int a = 1, b = 2;`, a name covers only its own part.
- **An annotation element's value is its `default`.** `String value() default "x";` is `x`.
- **A method without a body, such as one in an interface, is a symbol.**
- **Constructors, initializer blocks, record components, local and anonymous classes, and the members in the body of an
  enum constant are not symbols.** An accessor you write in a record, such as `int x()`, is a method.
- **A file with methods and fields at its top level, as Java 25 allows, gives them paths such as `#/main`.**
- **Strings follow Java's escapes.** `"caf\u00e9"` is `café`. A text block counts as the text Java reads, without the
  indentation its lines share and the white space at the ends of its lines. A character, such as `'a'`, and a string
  template stay as written. A path to a string with `\u005c` stops the run. Java turns `\u005c` into a backslash
  before it reads the other escapes, so `"\u005cn"` is `"\n"`.
- **A syntax error stops the run for the member that holds it, and for each type around that member.** The other
  members still resolve.

<!-- </block> -->

## C\#

<!-- <block name="c-sharp-paths"> -->

```csharp
namespace Acme.Billing;

public class Config
{
    [Obsolete]
    public const int MaxRetries = 20 * 1024;
    public static readonly string Query = """
        SELECT *
        FROM t
        """;
    public string Name { get; set; } = "default";
    public static string Url => $"https://{Host}/x";

    void Run() {}
    void Run(int times) {}

    public enum Mode { Red = 1, Green }

    class Limits { const long Id = 0x10L; }
}
```

| Path                               | Points at                              |
|------------------------------------|----------------------------------------|
| `#/Acme/Billing/Config`            | the whole class                        |
| `#/Acme/Billing/Config/MaxRetries` | `20 * 1024`                            |
| `#/Acme/Billing/Config/Query`      | `SELECT *` and `FROM t`, on two lines  |
| `#/Acme/Billing/Config/Name`       | `default`                              |
| `#/Acme/Billing/Config/Url`        | `$"https://{Host}/x"`, as written      |
| `#/Acme/Billing/Config/Run`        | a violation, as `Run` is defined twice |
| `#/Acme/Billing/Config/Mode/Green` | `Green`, as it has no value            |
| `#/Acme/Billing/Config/Limits/Id`  | `0x10L`                                |

- **A namespace is part of the path, but not a symbol.** `namespace Acme.Billing { }` and
  `namespace Acme { namespace Billing { } }` give the same paths as `namespace Acme.Billing;`. `#/Acme/Billing` alone
  is not found. A type outside any namespace starts at the top: `#/Config/MaxRetries`.
- **Each type is a symbol, and so is each member in it**: a field, a constant, a property, a method, an event, an enum
  member or a nested type. Classes, structs, interfaces, enums, records and delegates are types.
- **Overloads are defined twice.** So are the copies of a name in `#if` and `#else`, and `Cache` next to `Cache<T>`,
  as a path has no place for `<T>`. A member that only one of them has still resolves. Wrap the one you mean in a
  block instead.
- **A property's value is the one it starts with**: `v` in `X { get; set; } = v;` and in `X => v;`. A method has no
  value, even with `=> v`.
- **Attributes are part of a definition. `///` comments are not.** In `int a = 1, b = 2;`, a name covers only its own
  part. The code in `#if`, `#elif` and `#else` counts as if the directives were not there.
- **A name written with `@` keeps it**: `#/Config/@switch`.
- **Constructors, finalizers, indexers, operators, explicit interface members such as `void IDisposable.Dispose()`,
  record parameters and top-level statements are not symbols.** A property you write in a record's body is a symbol.
- **Strings follow C#'s escapes.** A verbatim string, such as `@"C:\temp"`, has none. A raw string counts as the text
  C# reads, without the white space before its closing quotes. `$"v1"` is `v1`. A string with a hole, such as
  `$"v{Major}"`, stays as written, and so do `"abc"u8` and a character such as `'a'`. A path to a string that C#
  rejects stops the run, such as `"\?"` or a raw string with a line less indented than its closing quotes.
- **A syntax error stops the run for the member that holds it, and for each type around that member.** The other
  members still resolve.

<!-- </block> -->

## Kotlin

<!-- <block name="kotlin-paths"> -->

```kotlin
const val MAX_RETRIES = 20 * 1024

data class Config(val timeout: Int = 30, val name: String) {
    @JvmField
    val url = "https://example.com"

    fun run() {}
    fun run(times: Int) {}

    companion object {
        const val ORIGIN = 0
    }

    enum class Mode { RED, GREEN }
}
```

| Path                  | Points at                                  |
|-----------------------|--------------------------------------------|
| `#/MAX_RETRIES`       | `20 * 1024`                                |
| `#/Config`            | the whole class                            |
| `#/Config/timeout`    | `30`                                       |
| `#/Config/name`       | `val name: String`, as it has no default   |
| `#/Config/url`        | `https://example.com`                      |
| `#/Config/run`        | a violation, as `run` is defined twice     |
| `#/Config/ORIGIN`     | `0`                                        |
| `#/Config/Mode/GREEN` | `GREEN`, as it has no value                |

- **Each top-level property, function, type and type alias is a symbol, and so is each member of a type**: a property,
  a function, an enum entry, or a nested class, interface or object. A path goes from the top-level type down:
  `#/Config/Mode/GREEN`.
- **A constructor parameter with `val` or `var` is a symbol.** Its value is its default. A parameter without `val` or
  `var` is not a symbol.
- **A companion object's members sit under the class**, as code reads them: `#/Config/ORIGIN`. A companion with a
  name works the same way: `companion object Builder` still gives `#/Config/ORIGIN`.
- **Overloads are defined twice.** So are a property and a function with one name, and two extension functions with
  one name, such as `fun Foo.toDto()` and `fun Bar.toDto()`. Wrap the one you mean in a block instead.
- **A property's value is its initializer, or the `v` of `get() = v`.** A property with `by`, such as
  `by lazy { 3 }`, has no value, and neither has one whose getter has a block body.
- **A name in backticks loses them.** `` fun `parses empty input`() `` in `Config` is
  `#/Config/parses empty input`.
- **Annotations are part of a definition. KDoc is not.** One case is left out: at the top of a file, an annotation with
  arguments on its own line, before a `fun` or `val` with no other modifier. The parser reads it as a separate
  expression.
- **Constructors, `init` blocks, the companion object itself, local declarations, and the members in the body of an
  enum entry are not symbols.** In a `.kts` script, the top-level properties, functions and types are symbols. Calls
  such as `plugins { ... }` and everything inside them are not.
- **Strings follow Kotlin's escapes.** `"caf\u00e9"` is `café`. A raw string (`"""..."""`) has no escapes, and each line
  end in it is a line feed. A string with a template, such as `"v$major"` or `"${a + b}"`, stays as written, and so
  does a character such as `'a'`. A path to a string that Kotlin rejects stops the run, such as `"\q"`.
- **A syntax error stops the run for the member that holds it, and for each type around that member.** The other
  members still resolve. The parser doesn't read some newer syntax, such as context parameters and `$$"..."` strings.
  It also fails on a class body on one line with a property in it, such as `class A { val x = 1 }`.

<!-- </block> -->

## Swift

<!-- <block name="swift-paths"> -->

```swift
public let maxRetries = 20 * 1024

struct Config {
    static let timeout = 30
    var name: String
    var ready: Bool { timeout > 0 }

    func run() {}
    func run(times: Int) {}

    enum Mode: String {
        case red = "r", green
    }
}

extension Config {
    static let url = "https://example.com"
}
```

| Path                  | Points at                                  |
|-----------------------|--------------------------------------------|
| `#/maxRetries`        | `20 * 1024`                                |
| `#/Config`            | the whole struct, without its extension    |
| `#/Config/timeout`    | `30`                                       |
| `#/Config/name`       | `var name: String`, as it has no value     |
| `#/Config/ready`      | `timeout > 0`                              |
| `#/Config/run`        | a violation, as `run` is defined twice     |
| `#/Config/Mode/red`   | `r`                                        |
| `#/Config/Mode/green` | `green`, as its raw value is implicit      |
| `#/Config/url`        | `https://example.com`                      |

- **Each top-level property, function, type, type alias and macro is a symbol, and so is each member of a type**: a
  property, a function, an enum case, an associated type or a nested type. A protocol is a type, and its requirements
  are members. A path goes from the top-level type down: `#/Config/Mode/red`.
- **An extension's members sit under the type it extends**, as code reads them: `#/Config/url`. The type can come from
  another file too, so `String+Format.swift` can have `#/String/trimmed`. The path follows the type's name as written:
  `extension Outer.Inner` gives `#/Outer/Inner/...`, and `extension Swift.String` gives `#/Swift/String/...`. An
  extension of a type without a name, such as `extension [Int]`, has no symbols.
- **Overloads are defined twice.** So are a property and a function with one name, copies of a name in `#if` and
  `#else`, and a protocol requirement and its default in an extension. Wrap the one you mean in a block instead.
- **A property's value is its initializer, or the `v` of `{ v }` or `{ return v }`.** A computed property with `get`
  or `set`, with several statements, or with one statement that is not an expression, such as a `for` loop, has no
  value. An enum case's value is its raw value, the `v` of `= v`. A case
  without one has no value, even when Swift gives it one.
- **In `let a = 1, b = 2` and `case a = 1, b`, each name covers only its own part.** The first part also covers the
  attributes, the modifiers and `let`, `var` or `case`: `#/a` is `let a = 1`, and `#/b` is `b = 2`.
- **A name in backticks loses them.** Attributes are part of a definition. Doc comments are not.
- **`init`, `deinit`, `subscript`, operators, precedence groups, extensions themselves, local declarations and
  top-level statements are not symbols.**
- **Strings follow Swift's escapes.** `"caf\u{e9}"` is `café`. A raw string, such as `#"C:\new"#`, reads only the
  escapes with as many `#` as its quotes, such as `\#n`. A multi-line string loses the white space before its closing
  quotes on each line, and each line end in it is a line feed. A string with an interpolation, such as `"v\(major)"`,
  stays as written, and so does a regex. A path to a string that Swift rejects stops the run, such as `"\u{D800}"`.
- **A syntax error stops the run for the member that holds it, and for each type around that member.** The other
  members still resolve. An extension is not part of the type it extends, so an error in an extension's member leaves
  the type readable. The parser fails on an empty tuple passed to a function, as in `f(())`, so a function with one in
  its body stops the run too.

<!-- </block> -->

## Protocol Buffers

<!-- <block name="proto-paths"> -->

```protobuf
syntax = "proto3";

package shop.v1;

option go_package = "shop/v1";

message Item {
  string name = 1;
  map<string, int32> counts = 2;
  enum Color {
    COLOR_UNSPECIFIED = 0;
    RED = 1;
    BLUE = -2;
  }
  oneof price {
    int64 cents = 3;
  }
}

service Items {
  rpc Get(Item) returns (Item);
}
```

| Path                 | Points at                    |
|----------------------|------------------------------|
| `#/Item`             | the whole message            |
| `#/Item/name`        | `1`                          |
| `#/Item/counts`      | `2`                          |
| `#/Item/Color`       | the whole enum               |
| `#/Item/Color/RED`   | `1`                          |
| `#/Item/Color/BLUE`  | `-2`                         |
| `#/Item/price`       | the whole oneof              |
| `#/Item/cents`       | `3`                          |
| `#/Items/Get`        | the whole rpc                |

- **Each message, enum and service is a symbol, at any depth, and so is each field, map field, oneof, enum value and
  rpc in it.** A proto2 group, such as `optional group Result = 4 { ... }` in `Item`, is a symbol too, with its
  fields under it: `#/Item/Result/code`. It has no value, though it has a number.
- **The package is not part of a path.** A file has only one, so `#/Item` is enough.
- **An enum value sits under its enum:** `#/Item/Color/RED`.
- **A field's value is its number, and an enum value's value is its number.** Both stay as written: `0x3` stays `0x3`,
  and `- 2` stays `- 2`. Options after the number don't change it: `BLUE = -2 [deprecated = true]` is `-2`.
- **A oneof's fields sit in its message**, as code reads them: `#/Item/cents`, not `#/Item/price/cents`.
- **An extension field sits where its `extend` is written**, not in the message it extends.
  `extend Other { int32 tag = 100; }` at the top of the file gives `#/tag`, and inside `Item` it gives `#/Item/tag`.
- **A definition covers its whole statement**, with its label, its options and its `;`, but not the comments above it.
- **Options are not symbols**, and neither are `syntax`, `package`, `import`, `extend` blocks, `reserved` and
  `extensions`. To check an option such as `go_package`, wrap it in a `[[block]]` entry with a `target-pattern`.
- **A syntax error stops the run for the definition that holds it, and for each message around it.** The other
  definitions still resolve.

<!-- </block> -->

## HCL

Terraform, OpenTofu, Terragrunt, Nomad and Packer files are HCL.

<!-- <block name="hcl-paths"> -->

```hcl
terraform {
  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.0"
    }
  }
}

variable "region" {
  default = "us-east-1"
}

locals {
  name = "app-${var.env}"
  tags = { Team = "core", "cost center" = 42 }
}

resource "aws_instance" "web" {
  ami       = data.aws_ami.ubuntu.id
  count     = (3)
  user_data = <<-EOT
    #!/bin/sh
    echo hi
  EOT
}
```

| Path                                         | Points at                                 |
|----------------------------------------------|-------------------------------------------|
| `#/terraform/required_providers/aws/version` | `~> 5.0`                                  |
| `#/variable/region`                          | the whole block                           |
| `#/variable/region/default`                  | `us-east-1`                               |
| `#/locals/name`                              | `"app-${var.env}"`, as written            |
| `#/locals/tags`                              | the whole object, as written              |
| `#/locals/tags/cost center`                  | `42`                                      |
| `#/resource/aws_instance/web/ami`            | `data.aws_ami.ubuntu.id`                  |
| `#/resource/aws_instance/web/count`          | `3`                                       |
| `#/resource/aws_instance/web/user_data`      | `#!/bin/sh` and `echo hi`, each on a line |

- **Each block is a symbol, at any depth, and so is each attribute and each key of an object.** A block's path is its
  type, then each of its labels: `#/resource/aws_instance/web`. In a `.tfvars` file, each attribute sits at the top:
  `#/region`.
- **A path with only some of a block's labels points at nothing.** `#/resource/aws_instance` is no symbol.
- **A block written twice in one body makes its path a violation**, such as two `ingress` blocks in a resource. Two
  `locals` blocks make `#/locals` a violation, but `#/locals/name` still resolves when only one of them sets `name`.
  A filter picks one of the blocks: `#/resource/aws_security_group/web/~[from_port=443]/to_port`.
- **A key of an object is a segment when it is a name, a plain string, `true` or `false`**, such as `Team` or
  `"cost center"`. A key written as a number or an expression, such as `3` or `(var.k)`, is not. The items of a list
  are not symbols.
- **A label or key with an escape, such as `"a\"b"`, gives no symbol**, and neither does anything under it.
- **A value is the expression as written**, without parentheses around it: `count = (3)` is `3`.
- **Strings follow HCL's escapes.** `"caf\u00e9"` is `café`, `$${` is `${`, and `%%{` is `%{`. A heredoc is its text,
  with each line end. A `<<-` heredoc loses the indent that HCL removes. A string or heredoc with `${...}` or `%{...}`
  stays as written. blockwatch doesn't turn text into Unicode NFC, as HCL does. A path to an escape that stands for no
  character, such as `"\uD800"`, stops the run.
- **A block covers its type, labels and body.** An attribute or a key covers its name and its value. Comments above
  them are not part of them.
- **A syntax error stops the run for the definition that holds it, and for each block around it.** After an error, the
  parser can read the rest of the file wrongly, so a later path may not resolve.

<!-- </block> -->

## PHP

<!-- <block name="php-paths"> -->

```php
<?php
namespace App\Billing;

const MAX_RETRIES = 3;
define('API_URL', 'https://api.example.com');
$version = '2.4.1';

if (!function_exists('money')) {
    function money(int $cents): string { return number_format($cents / 100, 2); }
}

#[Entity]
final class Invoice extends Model
{
    public const STATUS_PAID = 'paid';
    protected static int $perPage = 25;
    protected $table;

    public function __construct(private readonly string $currency = "EUR") {}

    public function table(): string { return $this->table; }
}

enum Status: string
{
    case Open = 'open';
    case Paid = 'paid';
}
```

| Path                    | Points at                                       |
|-------------------------|-------------------------------------------------|
| `#/MAX_RETRIES`         | `3`                                             |
| `#/API_URL`             | `https://api.example.com`                       |
| `#/$version`            | `2.4.1`                                         |
| `#/money`               | the whole function                              |
| `#/Invoice`             | the whole class, with `#[Entity]`               |
| `#/Invoice/STATUS_PAID` | `paid`                                          |
| `#/Invoice/$perPage`    | `25`                                            |
| `#/Invoice/$table`      | `protected $table;`, as it has no default       |
| `#/Invoice/$currency`   | `EUR`                                           |
| `#/Invoice/table`       | the whole method                                |
| `#/Status/Paid`         | `paid`                                          |
| `#/App`                 | nothing, as a namespace is not a symbol         |

- **Each top-level constant, variable, function, class, interface, trait and enum is a symbol, and so is each member
  of a type**: a constant, a property, a method or an enum case. Both `const NAME = value;` and
  `define('NAME', value);` define a constant.
- **The namespace is not part of a path.** Two namespaces in one file that define one name make that path a
  violation.
- **A property and a variable keep their `$`**: `#/Invoice/$perPage`. So `$table` and `table()` have different paths.
- **A top-level variable is a symbol**: `$version = '2.4.1';` is `#/$version`. Only `$name = value` counts, not
  `$items['key'] = 1` or `$object->name = 1`. In `$a = $b = 1`, only `$a` is a symbol, and its value is `$b = 1`.
- **A constructor parameter with a visibility is a property**: `private readonly string $currency` gives
  `#/Invoice/$currency`. Its value is its default.
- **A definition in an `if`, `else`, `try`, `catch`, `finally`, `switch`, `declare` or `{ }` block counts**, such as a
  function in `if (!function_exists('money'))`. One in a loop doesn't. A name defined in both branches of an `if` is a
  violation.
- **Inside a function or a method, only a `define` is a symbol, and its path has no parent.** `define('BOOTED', 1);`
  in the method `Kernel::boot()` gives `#/BOOTED`. Local variables, and functions and classes declared in a function,
  are not symbols. Neither are closures, arrow functions and anonymous classes, or a `define` in them.
- **A `define` counts only as a whole statement whose name is plain text.** It covers the statement, with its `;`.
  `define($name, 1)`, `define('A\\', 1)`, `defined('X') or define('X', 1)` and a call with named arguments are not
  symbols.
- **Names compare as written.** PHP ignores the case of class, function and method names, but `#/MONEY` doesn't find
  `function money`.
- **Attributes are part of a definition. Doc comments are not.** In `const A = 1, B = 2;` and `public $a = 1, $b;`,
  each name covers only its own part.
- **`namespace`, `use` imports, trait uses and `declare(strict_types=1)` are not symbols.** A trait's members sit under
  the trait, not under the classes that use it. The keys of an array, such as `return ['timezone' => 'UTC']` in a
  Laravel config file, are not symbols yet.
- **Strings follow PHP's escapes.** `'it\'s'` is `it's`, and `"caf\u{e9}"` is `café`. A heredoc or a nowdoc loses the
  closing marker's indent on each line. A string with a variable, such as `"v$major"` or `"{$a}"`, stays as written.
  So does a string whose bytes are not UTF-8, such as `"\xFF"`, and a string that PHP rejects.
- **In a template, the symbols come from the PHP code.** The HTML around it is skipped.
- **A syntax error stops the run for the definition that holds it, and for each type around it.** The other
  definitions still resolve. The parser fails on a constructor parameter with `readonly` but no visibility, such as
  `__construct(readonly int $id)`, so a path into its class stops the run.

<!-- </block> -->

## Shell

<!-- <block name="shell-paths"> -->

```bash
#!/usr/bin/env bash
GO_VERSION=1.22.5
export REGISTRY='ghcr.io/acme'
readonly IMAGE="${REGISTRY}/app:${GO_VERSION}"
PORT="${PORT:-8080}"
TARGETS=(linux darwin)

if [[ "$(uname)" == Darwin ]]; then
  SED=gsed
else
  SED=sed
fi

log() { echo "$@" >&2; }

while [[ $# -gt 0 ]]; do
  case "$1" in
    --go) GO_VERSION="$2"; shift 2 ;;
  esac
done
```

| Path           | Points at                                            |
|----------------|------------------------------------------------------|
| `#/GO_VERSION` | `1.22.5`, as the copy in the loop doesn't count      |
| `#/REGISTRY`   | `ghcr.io/acme`                                       |
| `#/IMAGE`      | `"${REGISTRY}/app:${GO_VERSION}"`, as written        |
| `#/PORT`       | `"${PORT:-8080}"`, as written                        |
| `#/TARGETS`    | `(linux darwin)`, as written                         |
| `#/SED`        | a violation, as both branches define it              |
| `#/log`        | the whole function                                   |

- **A variable or a function set outside functions, loops and subshells is a symbol.** The conditions and bodies of
  `if`, the items of `case`, `{ ... }` groups and the commands of `&&` and `||` count. So a name set in two branches
  is defined twice. Names set in `for`, `while` and `until` loops, in `( ... )`, in functions and in a statement that
  ends with `&` don't count, as bash sets them in a subshell or each time the loop runs.
- **`export`, `readonly`, `declare` and `typeset` set a name too**, with or without flags such as `-r`. When the
  statement sets one name, the symbol covers the whole statement. In `export A=1 B=2`, `A` covers only `A=1`. The
  value is still the one written: bash stores `abc` for `declare -l X=ABC` and `3` for `declare -i X=1+2`, but the
  path gives `ABC` and `1+2`.
- **These are not symbols:** `NAME+=value`, `A[0]=x`, `FOO=1 make` (it sets `FOO` only for `make`), `export NAME`
  without `=`, `local`, `: "${NAME:=default}"`, and names set by `read`, `for` or `getopts`.
- **Strings follow bash's quoting.** `'a b'` is `a b`. In `"..."`, only `\$`, `` \` ``, `\"` and `\\` lose their
  backslash, and a `\` at the end of a line goes away with the line end. Outside quotes, every backslash goes:
  `a\ b` is `a b`. Parts written together are joined: `"a"'b'c` is `abc`. `X=` is the empty string.
- **A value that bash expands stays as written**, such as `$HOME`, `$(cmd)`, `` `cmd` `` or `~/bin`. So does a word
  that starts with `~` after a quote, such as `"a"~/b`, and an array. Use `same-as-pattern` to take `8080` out of
  `"${PORT:-8080}"`.
- **A path to a `$'...'` or `$"..."` string stops the run.**
- **A function's name can hold `-`, `.` and `:`,** such as `#/ns::helper`.
- **zsh files (`.zsh`, `.zshrc`) have no symbols.** The parser misreads much of zsh's syntax.
- **After a syntax error, the parser often skips a large part of the file**, and every name in that part stops the
  run. It fails on `sed -e /^$/d`, on `${x/#$/y}`, and on a command whose name holds `::` after an assignment, such as
  `V=2 ns::log "x"`. A comment in backticks, such as `` find . `# skip vendor` -name x ``, is worse: the parser reads
  the rest of the file as the command's argument, so no name after it is found.
- **A value outside quotes that goes on to the next line after a `\` is not found.** The parser reads `X=a\` and `b`
  on the next line as `X=a` set only for a command `b`. Bash sets `X=ab`. Put the value in quotes instead.
- **A value that ends with a lone `$` takes in the next line.** In `Z=$` followed by `W=1`, `#/Z` gives `$`, the line
  end and `W=1`, as written, and `#/W` is not found. Bash sets `Z` to `$`. Write `Z='$'` instead.

<!-- </block> -->

## Makefile

<!-- <block name="makefile-paths"> -->

```make
GO_VERSION := 1.22.5
VERSION ?= $(shell git describe --tags)
export IMAGE = ghcr.io/acme/app:$(VERSION)
LINT := v2.10.1   # keep in sync with CI
TARGETS = linux \
          darwin
CFLAGS += -O2

ifeq ($(shell uname),Darwin)
SED = gsed
else
SED = sed
endif

define BANNER
Build $(VERSION)
endef

build: deps
	go build ./...

bin/app: $(SRC)
	go build -o $@ .

%.o: %.c
	$(CC) -c $<

.PHONY: build
```

| Path           | Points at                                         |
|----------------|---------------------------------------------------|
| `#/GO_VERSION` | `1.22.5`                                          |
| `#/VERSION`    | `$(shell git describe --tags)`, as written        |
| `#/IMAGE`      | `ghcr.io/acme/app:$(VERSION)`, as written         |
| `#/LINT`       | `v2.10.1`, without the comment                    |
| `#/TARGETS`    | `linux darwin`                                    |
| `#/CFLAGS`     | not found, as `+=` adds to a name                 |
| `#/SED`        | a violation, as both branches set it              |
| `#/BANNER`     | `Build $(VERSION)`                                |
| `#/build`      | the whole rule: the target line and the recipe    |
| `#/bin~1app`   | the whole rule                                    |
| `#/%25.o`      | not found, as a pattern rule is not a symbol      |
| `#/.PHONY`     | not found, as a special target is not a symbol    |

- **A variable or a target set at the top or in a conditional is a symbol.** Every branch of `ifeq`, `ifneq`,
  `ifdef` and `ifndef` counts, at any depth. So a name set in two branches is defined twice. Variables and targets
  share one set of names, so a variable and a target with the same name are defined twice too.
- **Every assignment but `+=` sets a name:** `=`, `:=`, `::=`, `:::=`, `?=` and `!=`, with any of `export`,
  `override` and `private` in front. So does `define NAME` ... `endef`, with the same keywords and operators.
  `X ?= 1` next to `X = 2` is defined twice too.
- **A target covers its whole rule.** In `a b: c`, both `a` and `b` cover the whole rule. A target in two rules is
  defined twice, even with `::`. Write each `/` in a target as `~1`: `bin/app` is `#/bin~1app`.
- **A rule ends at its last recipe line.** It takes in a conditional below it when a branch of the conditional starts
  with a recipe line, since make could read that branch. A rule in a branch of a conditional ends with that branch,
  since a path can't tell which branch make reads.
- **A line that starts with a tab is a recipe line only in a rule, as in make.** It is one when the last statement
  above it, apart from conditionals, comments and other recipe lines, is a rule. Each branch of a conditional starts
  from the statements above the conditional. After `endif`, a tab line is a recipe line when any branch ended in a
  rule. Anywhere else, an assignment on such a line sets a name, such as `CC = clang` after a tab, and a `#` on it
  starts a comment. After `.RECIPEPREFIX = >`, a recipe line starts with `>` in place of a tab.
- **These are not symbols:** `NAME += value`, a target-specific variable such as `build: NAME = x`, `export NAME`
  without a value, a computed name such as `$(PKG)_VERSION = 1` or `$(BIN):`, a pattern rule such as `%.o`, a suffix
  rule, and a special target. A suffix rule, such as `.c.o`, is the old way to write `%.o: %.c`. Its target is one or
  two of the suffixes that make knows, such as `.c`, `.o` or `.sh`, and it has no prerequisites. So
  `.sh: install.txt` is a symbol. A special target is `.` followed by only capital letters and `_`, such as
  `.PHONY`. So `.venv` and `.coverage` are symbols. The files that `include` reads are not searched. A path into
  such a file works.
- **A value is read as GNU make reads it, but it is never expanded:**
  - A `#` starts a comment, which is not part of the value. `\#` gives `#`.
  - A `\` at the end of a line joins the next line, with one space between them. The white space after the operator
    is left out, even when the value starts on the next line, as in `SRCS = \`.
  - `$(...)`, `${...}` and `$$` stay as written, with any `#` in them.
  - The white space at the end is left out. GNU make keeps it, but a `same-as` check would fail on spaces no one
    can see.
  - `X != cmd` gives `cmd`. The command is not run.
  - A `define` body keeps its comments and its `\#`, as make does.
  - `X =` gives the empty string.
  - A carriage return before a line feed is left out, as make does.
- **A line that make joins to a statement is part of it.** So it sets no name:
  - After a comment that ends in `\`, the next line is part of the comment.
  - In `A = a$\` with `B = 1` on the next line, `#/A` gives `a$ B = 1`, and `#/B` is not found.
  - The lines of a `define` body are part of it, up to the `endef` that make matches to it. A `define` inside it
    needs its own `endef`, and an `endef` after a tab doesn't count.
- **BSD make's own lines, such as `.if` and `.include`, set nothing.** An assignment between `.if` and `.endif`
  sets its name, as anywhere else.
- **A file that make stops on stops the run:** one with a `define` without `endef`, a conditional without `endif`,
  an `else` or `endif` outside a conditional, a second `else` in one conditional, or an `ifeq` without its arguments.
  The message shows the line. For a missing `endif`, that is the end of the file, as in make's own message.

<!-- <block name="makefile-nesting"> -->

- **A file nested more than 128 levels deep stops the run too,** with conditionals or with `$(...)` references.
  Make reads it, but the `makefile-lossless` crate skips the conditionals nested deeper, so a name set in them would
  be missing.

<!-- </block> -->

- **Known limits.** blockwatch reads Makefiles with the `makefile-lossless` crate, and keeps what it finds:
  - A rule takes in the comment lines after its last recipe line. In `test:`, a recipe line, then `# Tools.`, `#/test`
    covers `# Tools.`. So editing that comment counts as a change to `#/test`.
  - In a `define` body, a line that starts with a tab keeps a `\` at its end, and the line end after it. Make joins
    the two lines with one space.
  - A line that make stops on can give targets. `a b = 6` gives `#/a` and `#/b`, and `= 7` gives `#/7`. Make stops
    on them with "missing separator" and "empty variable name".
  - A comment is found on a recipe line whose first character after the tab is `#`, and after any `#` in a `define`
    body. So a block tag there is found. Make passes these lines on as they are, without reading a comment.
  - In `lint: N = x;y # value`, `# value` is read as a comment. Make keeps it in the value that `N` has for `lint`.
  - In `D = a$# b`, the `#` and the text after it are read as comments, and `#/D` gives `a$#`. Make reads `$#` as a
    variable reference, so the line has no comment, and the value is `a$# b`.

<!-- </block> -->

## Dart

<!-- <block name="dart-paths"> -->

```dart
const kApiVersion = '2.1.0';
int retries = 3, timeout = 30;
String get greeting => 'hi';

class Invoice {
  static const int maxLines = 20 * 1024;
  final int id;

  Invoice(this.id);
  Invoice.draft() : id = 0;

  int get total => _total;
  set total(int v) => _total = v;
  void send() {}
}

enum Mode { light, dark('d'); const Mode([this.code]); final String? code; }

extension StringX on String { bool get isBlank => trim().isEmpty; }
```

| Path                 | Points at                                         |
|----------------------|---------------------------------------------------|
| `#/kApiVersion`      | `2.1.0`                                           |
| `#/retries`          | `3`                                               |
| `#/greeting`         | `hi`                                              |
| `#/Invoice`          | the whole class                                   |
| `#/Invoice/maxLines` | `20 * 1024`                                       |
| `#/Invoice/id`       | `final int id;`, as it has no value               |
| `#/Invoice/draft`    | the whole constructor                             |
| `#/Invoice/total`    | a violation, as a getter and a setter share it    |
| `#/Invoice/send`     | the whole method                                  |
| `#/Mode/dark`        | `dark('d')`, as an enum value has no value        |
| `#/Mode/code`        | `final String? code;`                             |
| `#/StringX/isBlank`  | `trim().isEmpty`                                  |

- **Each top-level variable, function, getter, setter, type and type alias is a symbol, and so is each member of a
  type**: a field, a method, a getter, a setter, a named constructor or an enum value. Classes, mixins, enums,
  extensions and extension types are types. Dart has no nested types, so a path has at most two names:
  `#/Mode/dark`.
- **A named constructor is a symbol**, as code calls it by its name: `Invoice.draft()` is `#/Invoice/draft`. The
  unnamed constructor is not, even when it is written `Invoice.new()`. The header of an extension type can declare a
  named constructor and a field: `extension type Meters.from(double value)` gives `#/Meters/from` and
  `#/Meters/value`.
- **A named extension's members sit under its name**: `#/StringX/isBlank`. Two extensions of `String` in one file
  don't clash. An extension without a name has no symbols.
- **A getter and a setter with one name are defined twice**, and so are a named constructor and a field with one
  name, such as `Button.disabled()` and `disabled`. Wrap the one you mean in a block instead.
- **A variable's value is its initializer, and a getter's is the `v` of `=> v`.** A getter with a block body has no
  value, and neither has a function, a method or a constructor.
- **Annotations are part of a definition. Doc comments are not.** In `int a = 1, b = 2;`, each name covers only its
  own part.
- **The unnamed constructor, operators, `external` declarations, `library`, `import` and `part` directives, local
  declarations and closures are not symbols.** A file with `part of` has its own symbols.
- **Strings follow Dart's escapes.** `'caf\u{e9}'` is `café`. Adjacent strings, such as `'a' "b"`, are joined. A raw
  string, such as `r'C:\new'`, has no escapes. A `'''` or `"""` string loses a first line that holds only spaces and
  tabs. Each line end in a string is a line feed, unless a backslash comes before it. A string with `$name` or
  `${...}` in any part stays as written. A path to a string that Dart rejects, such as `'\u{110000}'`, or to half of a
  character, such as `'\u{D800}'`, stops the run.
- **A syntax error stops the run for the member that holds it, and for the type around it.** The other members still
  resolve.

<!-- </block> -->

---

[← Return to README](../README.md)
