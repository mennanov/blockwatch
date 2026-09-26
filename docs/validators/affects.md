# `affects`

Makes sure linked blocks are edited together. If you change this block but not the target it points at, the run fails.

## Syntax

| Attribute | Value                                            | Default |
|-----------|--------------------------------------------------|---------|
| `affects` | One or more [targets](#targets), comma-separated | —       |

```rust
// <block affects="README.md:supported-langs, docs/api.md:languages, locales/en.json">
```

## Targets

| Target       | Counts as changed when the diff touches              |
|--------------|------------------------------------------------------|
| `file:name`  | the block called `name` in `file`                    |
| `:name`      | the block called `name` in the same file             |
| `file`       | any part of `file` (see [Whole files](#whole-files)) |
| `file#/path` | one value inside `file` (see [Symbols](#symbols))    |
| `#/path`     | one value inside the same file                       |

## Example

**src/lib.rs**:

```rust
// <block affects="README.md:supported-langs">
pub enum Language {
    Rust,
    Python,
}
// </block>
```

**README.md**:

```markdown
<!-- <block name="supported-langs"> -->

- Rust
- Python

<!-- </block> -->
```

Change the enum, and the run fails until you also change `supported-langs` in `README.md`.

## Whole Files

A target without `:` or `#` is a whole file:

```rust
// <block affects="config/schema.json">
```

A block has to live in a comment, and some files can't have comments: plain JSON, `.env`, lockfiles, CSV, text
fixtures. A whole-file target works for all of them. Change the block, and the run fails until that file changes too.

Things to know:

- **Any edit counts.** Reformatting or a new comment satisfies the target. That is fine for a small file, but noisy for
  a big one. If the file has symbols, point at one of them instead: see [Symbols](#symbols).
- **Moving the file is not an edit.** A rename that changes no content still fails. Creating the file, even empty,
  counts.
- **It works one way only.** A file without comments can't hold an `affects` of its own, so "the JSON changed but the
  code didn't" goes unnoticed.
- **Deleting the target fails the run**, like a missing file behind a block target.
- **A file path with a `:` or `#` can't be used**, because those characters separate the file from the rest of the
  target.

## Symbols

`file#/path` points at one value inside a file. That value is called a **symbol**. [Symbols](../symbols.md) lists the
files that have symbols and explains how to write a path.

```rust
// <block affects="package.json#/version">
pub const VERSION: &str = "1.4.2";
// </block>
```

The target counts as changed only when the diff touches the key or its value. For an object or array, that means any
line inside it. Edits elsewhere in `package.json` don't count.

A TOML table counts as changed when the diff touches any place it is written. `affects="Cargo.toml#/package"` counts
a change under `[package.metadata.docs]`, but not one under `[dependencies]`.

A missing key is a violation, even without a diff. For the other cases, see
[When a path does not resolve](../symbols.md#when-a-path-does-not-resolve).

## Direction

`affects` works one way. The example above catches "code changed, docs didn't", but not the reverse. To catch both,
give both blocks a name and point each one at the other:

```rust
// <block name="languages-code" affects="README.md:supported-langs">
```

```markdown
[//]: # (<block name="supported-langs" affects="src/lib.rs:languages-code">)
```

## Notes

- **The edited-together check needs a diff.** Without one, no block counts as changed, so the check never runs. It
  does not pass; it just finds nothing. Run `git diff --patch | blockwatch --diff` to check the whole tree, or add
  `--only-changed` to check only the changed blocks. Without a diff, `blockwatch --verbosity summary` shows a
  `needs --diff` count of the blocks it skipped. Missing targets (below) are checked either way. To compare values
  without a diff, use [`same-as`](same-as.md).
- **It checks edits, not values.** Any edit to the target satisfies it, even an unrelated one. When both places must
  hold the same value, [`same-as`](same-as.md) is the stronger check.
- **Missing targets are violations**, even without a diff. That covers a block that was renamed or deleted, and a
  missing or repeated key. A target *file* that doesn't exist stops the run.
- **Targets are checked but not listed.** Under `--only-changed`, a target the diff did not touch is still checked, but
  it does not appear in the `--verbosity` report. See [Reports Under a Diff](../cli.md#reports-under-a-diff).
- **Globs don't limit targets.** `blockwatch --diff --only-changed "src/**/*.rs"` still finds a target under `docs/`,
  so narrowing a run to one language doesn't break links to other files. A target outside the globs is read to check
  the link, but it is never validated itself.
- With [`check-lua`](check-lua.md), a script can read its targets' contents through `ctx.affects`, without opening
  files.

---

← [Validators](README.md) · [README](../../README.md)
