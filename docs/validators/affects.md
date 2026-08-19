# `affects`

Forces linked blocks to be edited together. If you change this block and leave the block it points at untouched, the run
fails.

## Syntax

| Attribute | Value                                     | Default |
|-----------|-------------------------------------------|---------|
| `affects` | `file:name`, or `:name` for the same file | —       |

Separate multiple targets with commas:

```rust
// <block affects="README.md:supported-langs, docs/api.md:languages">
```

## Example

**src/lib.rs**:

```rust
// <block affects="README.html:supported-langs">
pub enum Language {
    Rust,
    Python,
}
// </block>
```

**README.html**:

```html
<!-- <block name="supported-langs"> -->
<ul>
    <li>Rust</li>
    <li>Python</li>
</ul>
<!-- </block> -->
```

Modify the enum and BlockWatch fails until you also touch `supported-langs` in `README.html`.

## Direction

`affects` is one-way. The example above catches "code changed, docs didn't" — but not the reverse. For two-way drift
detection, name both blocks and point each at the other:

```rust
// <block name="languages-code" affects="README.html:supported-langs">
```

```html
<!-- <block name="supported-langs" affects="src/lib.rs:languages-code"> -->
```

## Notes

- **Needs a diff.** Without one every block counts as unmodified, so `affects` reports nothing at all — not a pass, but
  no check. `blockwatch` on its own is therefore blind to it; run `git diff --patch | blockwatch --diff` to audit the
  whole tree with `affects` enforced, or add `--only-changed` to check just the changed blocks. On a run without a diff,
  `blockwatch --verbosity summary` prints a `needs --diff` count naming how many blocks were skipped for this reason;
  under a diff those rules can fire, so the count is not reported. If you want a check that also works on a bare
  full-tree run, use [`same-as`](same-as.md).
- **Co-editing, not agreement.** `affects` only checks that both sides were touched — it does not compare their
  contents. Touching the target with an unrelated edit satisfies it. When the two blocks should hold the same *value*, [
  `same-as`](same-as.md) is the stronger check.
- **Missing targets are violations.** A reference to a `name` that does not exist is reported.
- **Targets are read, not reported.** Under `--only-changed`, a target the diff did not touch is still resolved and
  compared, but it does not appear in a `--verbosity` run report. See
  [Reports Under a Diff](../cli.md#reports-under-a-diff).
- **Globs do not narrow target resolution.** `blockwatch --diff --only-changed "src/**/*.rs"` still resolves a target
  living under `docs/`, so narrowing a run to one language does not report every rule that spans two of them as
  unsatisfied. An excluded target file is read to answer the reference and is never validated.
- Combining `affects` with [`check-lua`](check-lua.md) gives a script access to the affected blocks' contents through
  `ctx.affects`, which is a way to compare them without file IO.

---

← [Validators](README.md) · [README](../../README.md)
