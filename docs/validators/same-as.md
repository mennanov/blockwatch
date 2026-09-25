# `same-as`

Checks that two or more places hold the **same value**. [`affects`](affects.md) only checks that linked blocks were
edited together. `same-as` compares what they contain. Use it for constants, lists and versions that are copied in
several places.

It runs on every scan, with or without a diff.

## Syntax

| Attribute         | Value                                               | Default           |
|-------------------|-----------------------------------------------------|-------------------|
| `same-as`         | One or more [targets](#targets), comma-separated    | —                 |
| `same-as-pattern` | A regex that picks the values to compare            | the whole content |
| `same-as-mode`    | `set`, `sequence`, `single` or `subset`             | `set`             |
| `same-as-format`  | `numeric`                                           | text              |

## Targets

| Target       | Compares against                                                                 |
|--------------|----------------------------------------------------------------------------------|
| `file:name`  | the block called `name` in `file`                                                |
| `:name`      | the block called `name` in the same file                                         |
| `file`       | all of `file` (see [Whole files](#whole-files-as-targets))                       |
| `file#/path` | one value inside `file`, such as a JSON key (see [Symbols](#symbols-as-targets)) |
| `#/path`     | one value inside the same file                                                   |

## Example

With no other attributes, both blocks must hold **identical** text. Blank lines and indentation don't count. This suits
text you can't share any other way, such as a command shown in two docs:

**README.md**:

```markdown
[//]: # (<block same-as="docs/ci.md:pre-commit">)

    git diff --patch --cached | blockwatch --diff --only-changed

[//]: # (</block>)
```

**docs/ci.md**:

```markdown
[//]: # (<block name="pre-commit">)

    git diff --patch --cached | blockwatch --diff --only-changed

[//]: # (</block>)
```

Usually, though, the two places don't hold the same text. Then each block says **which parts to compare**.

## Pick values with `same-as-pattern`

Each block has its own regex. Every match is a value: the `(?P<value>…)` group if there is one, otherwise the whole
match. A line can hold several values. Lines without a match are skipped, and so are empty matches. This lets you
compare blocks written in different formats:

```rust
// <block same-as="README.md:supported-env-vars" same-as-pattern="BLOCKWATCH_AI_[A-Z_]+">
const API_KEY: &str = "BLOCKWATCH_AI_API_KEY";
const API_URL: &str = "BLOCKWATCH_AI_API_URL";
// </block>
```

```markdown
[//]: # (<block name="supported-env-vars" same-as-pattern="BLOCKWATCH_AI_[A-Z_]+">)

- `BLOCKWATCH_AI_API_KEY`: API key.
- `BLOCKWATCH_AI_API_URL`: API URL.

[//]: # (</block>)
```

### Lines are not part of the comparison

With a pattern, only the values count, not how they are split into lines. These two blocks agree, even in `sequence`
mode:

```rust
// <block same-as="b.md:letters" same-as-pattern="[A-Z]+" same-as-mode="sequence">
A, B
C, D
// </block>
```

```markdown
[//]: # (<block name="letters" same-as-pattern="[A-Z]+">)

A, B, C D

[//]: # (</block>)
```

If the layout matters, leave the pattern off. Without one, the content is compared line by line.

## Whole files as targets

A target without `:` or `#` is a whole file. The block is compared against the file's full text, and nothing in the
file is parsed. So any file can be a target, even one that can't hold a block, such as `.env` or a lockfile:

```rust
// <block same-as=".nvmrc" same-as-pattern="\d+\.\d+\.\d+">
pub const NODE_VERSION: &str = "20.11.1";
// </block>
```

The file has no block of its own, so the referencing block's pattern reads both sides. For anything longer than one
line, you will want a pattern: without one, the block must equal the entire file.

## Symbols as targets

`file#/path` points at one value inside a file. That value is called a **symbol**. `#/path` points into the block's own
file. JSON files (`.json` and `.jsonc`) have symbols: every key and every array item.

```rust
// <block same-as="package.json#/dependencies/inngest" same-as-pattern="\d+\.\d+\.\d+">
pub const INNGEST_VERSION: &str = "4.18.1";
// </block>
```

This compares only `/dependencies/inngest`. With `same-as="package.json"`, the pattern would pick up every version in
the file.

- **What gets compared.** A string, number, `true`, `false` or `null` gives its value. A string loses its quotes. An
  object or array gives its text as written in the file. The referencing block's pattern applies, as for a whole file.
- **How to write a path.** Paths follow [RFC 6901](https://www.rfc-editor.org/rfc/rfc6901) (JSON Pointer):
  - `/` separates keys: `#/dependencies/inngest`.
  - An array item is its position, counted from 0: `#/files/0`.
  - Inside a key, write `/` as `~1` and `~` as `~0`: `#/dependencies/@types~1node`.
  - Inside a key, write `,` as `%2C`, `:` as `%3A` and `%` as `%25`.
- **A missing or repeated key is a violation.** For a missing key, the message suggests similar paths:
  `symbol not found; did you mean: /version`. For a key that appears twice, it shows where each copy is:
  `ambiguous symbol, defined at 2:3, 5:3`.
- **A file with a syntax error stops the run.** BlockWatch does not guess what a broken file meant. A trailing comma
  counts as an error, even in `.jsonc`.

## Comparison modes

| `same-as-mode`  | Passes when                                                       |
|-----------------|-------------------------------------------------------------------|
| `set` (default) | both sides hold the same values, in any order, ignoring repeats   |
| `sequence`      | both sides hold the same values in the same order                 |
| `single`        | each side holds exactly one value, and the two are equal          |
| `subset`        | every value in this block also appears in the target              |

`subset` checks one direction only. Use it when one side should hold part of the other, like a test that uses only
some of the environment variables:

```rust
// <block same-as="src/config.rs:env-vars" same-as-mode="subset" same-as-pattern="BLOCKWATCH_AI_[A-Z_]+">
const API_KEY: &str = "BLOCKWATCH_AI_API_KEY";
// </block>
```

## Numeric comparison

`same-as-format="numeric"` reads each value as a number, so `60` and `60.0` are equal.

This helps when one value lives in two languages that write it differently. Here a timeout lives in Rust and in
TypeScript. The tags sit right around the number, so each block holds only the number and needs no pattern:

**src/backend.rs**:

```rust
const TIMEOUT: Duration = Duration::from_secs_f64(/* <block same-as="app/config.ts:timeout" same-as-format="numeric"> */ 60.0 /* </block> */);
```

**app/config.ts**:

```typescript
export const timeout = /* <block name="timeout"> */ 60 /* </block> */; // seconds
```

Rust writes `60.0` and TypeScript writes `60`. As numbers they are equal. As text they would not be.

- Numbers are compared exactly, however long they are. Two large IDs never match because of rounding.
- A number can have a sign, a fraction and an exponent: `-1`, `.5`, `1e9`. `inf` and `NaN` are not numbers here.
- `_` can separate digits, so Rust's `1_000_000` equals JSON's `1000000`. It must sit between two digits: `_1`, `1_`,
  `1__0` and `1_.0` are not numbers.

## Notes

- **Which block's attributes count.** The referencing block's `same-as-mode` and `same-as-format` decide how to compare.
  Each block's `same-as-pattern` reads only that block. A whole file or a symbol has no block, so the referencing
  block's pattern reads it too.
- **Violations:** a missing block, a missing or repeated key, a value that is not a number under `numeric`, a `single`
  side without exactly one value, and a pattern that matches nothing on either side. Two empty sides do not count as
  equal.
- **Errors that stop the run:** an unknown `same-as-mode` or `same-as-format`, an invalid regex, a missing target file,
  and a symbol in a file with a syntax error or without symbols.
- **Run it on the whole tree now and then.** `same-as` needs no diff, so a plain `blockwatch` run catches drift that
  `--only-changed` misses. See [CI integration](../ci.md).
- **A rename alone goes unnoticed in a diff.** Renaming a target file without changing it (a plain `git mv`) gives
  `--only-changed` nothing to check. A full run catches it, because the old path no longer exists.
- **Targets are checked but not listed.** Under `--only-changed`, a target the diff did not touch is still compared, but
  it does not appear in the `--verbosity` report. See [Reports Under a Diff](../cli.md#reports-under-a-diff).

---

← [Validators](README.md) · [README](../../README.md)
