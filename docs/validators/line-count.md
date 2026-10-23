# `line-count`

Constrains how many lines a block may contain. Flags unbounded growth in a public API surface, or pins a switch to the
size of the enum it maps.

## Syntax

| Attribute            | Value                                      | Default                  |
|----------------------|--------------------------------------------|--------------------------|
| `line-count`         | `<N`, `<=N`, `==N`, `>=N`, `>N`            | —                        |
| `line-count-pattern` | regex; a line counts when it gives a value | every line that has text |

`line-count-pattern` works only next to `line-count`. On a block without it, it stops the run, because it would do
nothing there.

## Example

```python
# <block line-count="<=5">
"a",
"b",
"c"
# </block>
```

## Counting only some lines

`line-count-pattern` counts only the lines it matches:

```python
# <block line-count="<=2" line-count-pattern='^"\w+",$'>
"a",
# a comment
"b",
# </block>
```

The block above counts 2: the comment line doesn't match.

Lines that do not match are skipped, as are lines whose match is empty. So is a line that matches a branch without the
`value` group: `^(?:none|(?P<value>\w+))$` skips the line `none`. The pattern runs on the trimmed line, so `^` and `$`
match at the text, not at the indentation.

## Notes

- **Blank lines are not counted.** The block above counts 3.
- A malformed comparator is a hard error, not a violation.

---

← [Validators](README.md) · [README](../../README.md)
