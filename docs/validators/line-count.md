# `line-count`

Constrains how many lines a block may contain. Flags unbounded growth in a public API surface, or pins a switch to the
size of the enum it maps.

## Syntax

| Attribute    | Value                           | Default |
|--------------|---------------------------------|---------|
| `line-count` | `<N`, `<=N`, `==N`, `>=N`, `>N` | —       |

## Example

```python
# <block line-count="<=5">
"a",
"b",
"c"
# </block>
```

## Notes

- **Blank lines are not counted.** The block above counts 3.
- A malformed comparator is a hard error, not a violation.

---

← [Validators](README.md) · [README](../../README.md)
