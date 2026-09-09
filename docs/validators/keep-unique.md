# `keep-unique`

Prevents duplicate entries in a list — allowlists, registered IDs, route names.

## Syntax

| Attribute     | Value                                                    | Default    |
|---------------|----------------------------------------------------------|------------|
| `keep-unique` | empty, or a regex whose `(?P<value>…)` group is compared | whole line |

Unlike `keep-sorted`, the regex goes directly in the attribute value — there is no separate
`keep-unique-pattern`.

## Example

```python
# <block keep-unique>
"user_1",
"user_2",
"user_3",
# </block>
```

## Uniqueness by regex

Compare only part of each line:

```python
ids = [
    # <block keep-unique="ID:(?P<value>\d+)">
    "ID:1 Alice",
    "ID:2 Bob",
    "ID:1 Carol",  # Violation: ID:1 is already used
    # </block>
]
```

Without a `value` group the whole match is compared. Lines that do not match are skipped, as are
lines whose match is empty.

## Notes

- Blank lines are ignored.
- Comparison is on the trimmed line, so indentation does not create false uniqueness. A regex is
  applied to the trimmed line too, so `^` and `$` anchor to the entry rather than to the
  indentation.
- Commonly combined with [`keep-sorted`](keep-sorted.md) on the same block.

---

← [Validators](README.md) · [README](../../README.md)
