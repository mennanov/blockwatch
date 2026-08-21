# `same-as`

Asserts that two or more blocks hold the **same value**. Where [`affects`](affects.md) only checks that linked blocks
were co-edited, `same-as` compares their contents — catching duplicated constants, lists, and versions that silently
drift apart.

Unlike `affects`, it also runs on a full-tree scan, not only on a diff.

## Syntax

| Attribute         | Value                                                                   | Default    |
|-------------------|-------------------------------------------------------------------------|------------|
| `same-as`         | `file:name`, or `:name` for the same file; comma-separated for multiple | —          |
| `same-as-pattern` | regex; the `(?P<value>…)` group, or the whole match                     | whole line |
| `same-as-mode`    | `set`, `sequence`, `single`, `subset`                                   | `set`      |
| `same-as-format`  | `numeric`                                                               | text       |

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

Each side extracts one token per line via its own regex — the `(?P<value>…)` capture group, or the whole match if there
is none. Lines that do not match are skipped. Because each block self-describes, blocks in different formats can still
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

## Notes

- **Which block governs what.** The source block's `same-as-mode` and `same-as-format` govern the comparison. Each
  block's own `same-as-pattern` governs only how *that* block is read.
- **Violations:** a missing target block, a non-numeric token under `numeric`, a `single` /
  `subset` side with the wrong number of tokens, or a `same-as-pattern` that matches nothing on
  both sides — an empty match on both blocks is not treated as trivially equal.
- **Hard errors** (not violations): an unrecognized `same-as-mode` or `same-as-format` value, or an invalid regex.
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
