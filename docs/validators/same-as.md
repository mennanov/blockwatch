# `same-as`

Checks that two or more places hold the **same value**. [`affects`](affects.md) only checks that linked blocks were
edited together. `same-as` compares what they contain. Use it for constants, lists and versions that are copied in
several places.

It runs on every scan, with or without a diff.

## Syntax

| Attribute         | Value                                               | Default           |
|-------------------|-----------------------------------------------------|-------------------|
| `same-as`         | One or more [targets](#targets), comma-separated    | —                 |
| `same-as-paths`   | A [glob](#paths-as-targets) of paths to compare     | —                 |
| `same-as-pattern` | A regex that picks the values to compare            | the whole content |
| `same-as-mode`    | `set`, `sequence`, `single` or `subset`             | `set`             |
| `same-as-format`  | `numeric`, `identifier` or `flat`                   | text              |

## Targets

| Target       | Compares against                                             |
|--------------|--------------------------------------------------------------|
| `file:name`  | the block called `name` in `file`                            |
| `:name`      | the block called `name` in the same file                     |
| `file`       | all of `file` (see [Whole files](#whole-files-as-targets))   |
| `file#/path` | one value inside `file` (see [Symbols](#symbols-as-targets)) |
| `#/path`     | one value inside the same file                               |

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
match. A line can hold several values. Lines without a match are skipped, and so are empty matches. So is a match of a
branch without the `value` group: `^(?:none|(?P<value>\w+))$` skips the line `none`. This lets you compare blocks
written in different formats:

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
file. [Symbols](../symbols.md) lists the files that have symbols and explains how to write a path.

```rust
// <block same-as="package.json#/dependencies/inngest" same-as-pattern="\d+\.\d+\.\d+">
pub const INNGEST_VERSION: &str = "4.18.1";
// </block>
```

This compares only `/dependencies/inngest`. With `same-as="package.json"`, the pattern would pick up every version in
the file.

What a symbol compares as:

- **A string** compares as the text it holds, without its quotes.
- **Any other plain value** compares as written, so `1_000` stays `1_000`.
- **A value that holds other values**, such as an object, a table, a mapping or a list, compares as its text in the
  file.
- **A TOML table written in several places** compares as the text of each place, headers included, in the order they
  appear.

The referencing block's pattern reads the symbol, as it does a whole file.

## Paths as targets

`same-as-paths` compares the block with the paths in the repository that match a glob. Use it for a list that must
show every entry of a directory, such as the services in a README:

```markdown
<!-- <block same-as-paths="services/*/" same-as-pattern="services/(?P<value>[a-z-]+)"> -->
- [billing](services/billing)
- [search](services/search)
<!-- </block> -->
```

The glob lists `services/auth/`, `services/billing/` and `services/search/`. The pattern picks `auth`, `billing` and
`search` from them, and `billing` and `search` from the block. So the run reports that `auth` is missing.

- **`same-as-pattern` is required.** It reads both the block and the paths. To compare whole lines, use
  `same-as-pattern=".+"`.
- **The paths are sorted, and each one is listed once.** So `same-as-mode="sequence"` also checks that the block
  lists them in that order.
- **A trailing `/` lists directories.** A directory is listed when it holds a file, and `services/billing/` is listed
  once however many files it holds. Without the `/`, the glob lists files: `crates/*/AGENTS.md`.
- **`*` stops at `/`.** `services/*/` lists `services/billing/`, but not `services/billing/src/`. `**` matches any
  number of directories: `docs/**/*.md`. This differs from path arguments and the `ignore` setting, where `*` also
  matches `/`.
- **It takes one glob.** To list several directories, use braces: `{services,workers}/*/`.
- **Files that `.gitignore` leaves out are not listed,** and neither are symlinks. The `ignore` setting does not apply:
  it chooses which blocks to check, not what a target holds.
- **A glob that matches nothing is a violation,** so a typo in it can't pass.

A block can have both `same-as` and `same-as-paths`. Each target is compared with the block on its own.

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

## Identifier comparison

`same-as-format="identifier"` splits each value into words and compares the words. So `PreExistingFailure`,
`pre-existing-failure`, `pre_existing_failure` and `PRE_EXISTING_FAILURE` are equal.

This helps when one name is written in a different style on each side. Here a Rust enum is sent as kebab-case, and the
docs list the values it sends:

**src/report.rs**:

```rust
#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Category {
    // <block same-as="docs/report.md:categories" same-as-format="identifier" same-as-pattern="^(?P<value>\w+),$">
    PreExistingFailure,
    ToolBlocked,
    // </block>
}
```

**docs/report.md**:

```markdown
<!-- <block name="categories" same-as-pattern="`(?P<value>[a-z-]+)`"> -->
- `pre-existing-failure`
- `tool-blocked`
<!-- </block> -->
```

- **Words break at a change from lower case to upper case,** and at any character that is not a letter or a digit,
  such as `_`, `-`, `.` or a space.
- **A run of capitals is one word.** `HTTPServer` is `http server`, so it equals `http-server`. serde writes
  `HTTPServer` as `h-t-t-p-server`, and that does not equal `HTTPServer`. Write `HttpServer` instead.
- **A digit stays in the word before it.** `Version2` equals `version2`, but not `version_2`.
- **Where the words break counts.** `Signin` and `sign-in` are not equal. To ignore it, use
  [`flat`](#flat-comparison).
- **The style itself is not checked.** If the docs said `pre_existing_failure`, the words would still match. To catch
  that, let the pattern match only one style. Above, `[a-z-]+` doesn't match `_`, so that value would be missing.
- **Messages show each value in snake_case,** such as `pre_existing_failure`, because that is what is compared.

## Flat comparison

`same-as-format="flat"` lower-cases each value and drops every character that is not a letter or a digit. So
`CronTab`, `cron-tab`, `CRON_TAB` and `crontab` all compare as `crontab`.

This helps when one side writes a name with no word breaks at all. Here a list of Kubernetes kinds is in CamelCase, and
the manifest files that define them use the lower-case plural:

**src/kinds.rs**:

```rust
pub const KINDS: &[&str] = &[
    // <block same-as="deploy/kustomization.yaml:crds" same-as-format="flat" same-as-pattern='"(?P<value>\w+)"'>
    "CronTab",
    "DataSource",
    // </block>
];
```

**deploy/kustomization.yaml**:

```yaml
resources:
  # <block name="crds" same-as-pattern="crds/(?P<value>[a-z]+?)s?\.yaml">
  - crds/crontabs.yaml
  - crds/datasources.yaml
  # </block>
```

The YAML pattern drops the plural `s`: `[a-z]+?` takes as few letters as it can, so `s?` gets the `s`. It only drops a
plain `s`, so `policies.yaml` gives `policie`, which doesn't equal `Policy`.

- **Where the words break doesn't count.** `Signin` equals `sign-in`. When both sides write the word breaks, use
  [`identifier`](#identifier-comparison), which catches `Signin` against `sign-in`.
- **Digits and accents count.** `Version2` equals `version-2`, but not `version`. `CAFÉ` equals `café`, but not `cafe`.
- **Messages show each value as it is compared,** such as `crontab`.

## Notes

- **Which block's attributes count.** The referencing block's `same-as-mode` and `same-as-format` decide how to compare.
  Each block's `same-as-pattern` reads only that block. A whole file, a symbol or a list of paths has no block, so the
  referencing block's pattern reads it too.
- **Violations:** a missing block, a missing or repeated key, a value that is not a number under `numeric`, a `single`
  side without exactly one value, a pattern that matches nothing on either side, and a `same-as-paths` glob that
  matches no path. Two empty sides do not count as equal.
- **Errors that stop the run:** an unknown `same-as-mode` or `same-as-format`, an invalid regex or glob,
  `same-as-paths` without `same-as-pattern`, a missing target file, and a symbol in a file with a syntax error, without
  symbols, or with several YAML documents.
- **Run it on the whole tree now and then.** `same-as` needs no diff, so a plain `blockwatch` run catches drift that
  `--only-changed` misses. See [CI integration](../ci.md).
- **A rename alone goes unnoticed in a diff.** Renaming a target file without changing it (a plain `git mv`) gives
  `--only-changed` nothing to check. A full run catches it, because the old path no longer exists.
- **A new path alone goes unnoticed in a diff.** Adding `services/payments/` doesn't touch the block that lists the
  services, so `--only-changed` doesn't check it. A full run catches it.
- **Targets are checked but not listed.** Under `--only-changed`, a target the diff did not touch is still compared, but
  it does not appear in the `--verbosity` report. See [Reports Under a Diff](../cli.md#reports-under-a-diff).

---

← [Validators](README.md) · [README](../../README.md)
