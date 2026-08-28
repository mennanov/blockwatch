# `same-as`

Asserts that two or more blocks hold the **same value**. Where [`affects`](affects.md) only checks that linked blocks
were co-edited, `same-as` compares their contents — catching duplicated constants, lists, and versions that silently
drift apart.

Unlike `affects`, it also runs on a full-tree scan, not only on a diff.

## Syntax

| Attribute         | Value                                                                                            | Default    |
|-------------------|--------------------------------------------------------------------------------------------------|------------|
| `same-as`         | `file:name`, `:name` for the same file, or `file` for a whole file; comma-separated for multiple | —          |
| `same-as-pattern` | regex; every match's `(?P<value>…)` group, or the whole match                                    | whole line |
| `same-as-mode`    | `set`, `sequence`, `single`, `subset`                                                            | `set`      |
| `same-as-format`  | `numeric`                                                                                        | text       |

## Example

With no extra attributes the whole trimmed content is compared as text, so both blocks must be **identical**. This fits
content you cannot factor into a shared symbol — here, a command documented in two places:

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

Most couplings, though, do not share verbatim text. For those, each block describes **how to read itself**.

## Extract values with `same-as-pattern`

Each side extracts tokens via its own regex — the `(?P<value>…)` capture group, or the whole match if there is none.
Every match on a line counts, so a line listing several values contributes all of them; lines that do not match are
skipped, as are matches whose value is empty. Because each block self-describes, blocks in different formats can still
be compared:

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

A pattern compares the **values** a block yields, not the lines they are written on: every line's matches flow into one
flat list. That is what makes the example above work, and it means regrouping the same values across lines is not a
disagreement; these two blocks agree, in `sequence` mode as much as in `set`:

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

If a block's line structure is itself meaningful, leave the pattern off. Without one, the whole content is compared
newline by newline, so the layout has to match too.

## Whole files as targets

A target written without a `:` names a file rather than a block, and the file's **entire** content is what the block is
compared against. Nothing is parsed out of it, so a format that cannot declare a block — JSON, `.env`, a lockfile — can
still be a target:

```rust
// <block same-as="package.json" same-as-pattern="\d+\.\d+\.\d+">
pub const VERSION: &str = "1.4.2";
// </block>
```

The file has no block of its own to carry a `same-as-pattern`, so the referencing block's pattern reads both sides. On a
file of any size a pattern is usually what you want: without one, the block's content has to equal the whole file.

## Comparison modes

| `same-as-mode`  | Meaning                                                            |
|-----------------|--------------------------------------------------------------------|
| `set` (default) | Order- and duplicate-insensitive; the two token sets must be equal |
| `sequence`      | Order-sensitive list equality                                      |
| `single`        | Exactly one token per side — "there is exactly one version"        |
| `subset`        | Directional: every token here must also appear in the target       |

`subset` is the one directional mode. It fits cases where one side is a legitimate subset of the other — a test fixture
exercising only some of the declared environment variables, say:

```rust
// <block same-as="src/config.rs:env-vars" same-as-mode="subset" same-as-pattern="BLOCKWATCH_AI_[A-Z_]+">
const API_KEY: &str = "BLOCKWATCH_AI_API_KEY";
// </block>
```

## Numeric comparison

`same-as-format="numeric"` parses each token as a number before comparing, so the same quantity written in different
numeric forms still agrees.

A timeout shared between a Rust backend and a TypeScript frontend is a good case: the value cannot be imported across
the language boundary, and the two sides spell it differently. Wrapping the tag *inline* around just the literal keeps
the block content down to the number itself, so no
`same-as-pattern` is needed:

**src/backend.rs**:

```rust
const TIMEOUT: Duration = Duration::from_secs_f64(/* <block same-as="app/config.ts:timeout" same-as-format="numeric"> */ 60.0 /* </block> */);
```

**app/config.ts**:

```typescript
export const timeout = /* <block name="timeout"> */ 60 /* </block> */; // seconds
```

Rust's `from_secs_f64` takes a float (`60.0`) while TypeScript uses a plain `60`; `numeric` parses both and they compare
equal. Under text comparison, `"60.0" != "60"` would fail.

Values are compared as exact decimals of any length, so two identifiers far beyond the range of a 64-bit float never
compare equal by accident. A value may carry a sign, a fractional part and an exponent (`-1`, `.5`, `1e9`); `inf` and
`NaN` are not numbers a source file can hold and are reported like any other non-numeric token.

`_` is accepted as a digit separator, so each side keeps the spelling its language gives it — a Rust `1_000_000` and a
JSON `1000000` are the same value. A separator must sit between two digits: `_1`, `1_`, `1__0` and `1_.0` are typos, not
numbers.

## Notes

- **Which block governs what.** The source block's `same-as-mode` and `same-as-format` govern the comparison. Each
  block's own `same-as-pattern` governs only how *that* block is read — except for a whole-file target, which has no
  block of its own and is read under the referencing block's pattern.
- **Not a violation:** the same values regrouped across lines, when a `same-as-pattern` is set. See
  [Lines are not part of the comparison](#lines-are-not-part-of-the-comparison).
- **Violations:** a missing target block, a non-numeric token under `numeric`, a `single` /
  `subset` side with the wrong number of tokens, or a `same-as-pattern` that matches nothing on
  both sides — an empty match on both blocks is not treated as trivially equal.
- **Hard errors** (not violations): an unrecognized `same-as-mode` or `same-as-format` value, an invalid regex, or a
  target file that does not exist.
- Because `same-as` fires without a diff, a periodic bare `blockwatch` run — which scans the whole tree — catches drift
  that an `--only-changed` check would miss. See [CI integration](../ci.md).
- **A pure rename is invisible to diff input.** Renaming a `same-as` target with no content change (a plain `git mv`)
  produces no content hunk, so `--only-changed` has nothing to check and exits `0`. A full-tree run does catch it,
  because the old path no longer resolves — another reason to schedule the periodic run above.
- **Targets are read, not reported.** Under `--only-changed`, a target the diff did not touch is still resolved and
  compared, but it does not appear in a `--verbosity` run report. See
  [Reports Under a Diff](../cli.md#reports-under-a-diff).

---

← [Validators](README.md) · [README](../../README.md)
