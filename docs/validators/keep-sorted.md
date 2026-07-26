# `keep-sorted`

Keeps a list ordered. Eliminates "please sort this" review nits.

## Syntax

| Attribute             | Value                                              | Default |
|-----------------------|----------------------------------------------------|---------|
| `keep-sorted`         | `asc`, `desc`                                       | `asc`   |
| `keep-sorted-pattern` | regex; the `(?P<value>…)` group, or the whole match | whole line |
| `keep-sorted-format`  | `numeric`                                           | text    |

## Example

```python
# <block keep-sorted>
"apple",
"banana",
"cherry",
# </block>
```

Reorder any of those lines and the run fails.

## Sort by regex

Sort on part of the line rather than the whole thing, using a capture group named `value`:

```python
items = [
    # <block keep-sorted="asc" keep-sorted-pattern="id: (?P<value>\d+)">
    "id: 1  apple",
    "id: 2  banana",
    "id: 10 orange",
    # </block>
]
```

Lines that do not match the pattern are skipped.

## Numeric sort

Values are compared lexicographically by default, so `"10"` sorts before `"2"` — character by
character, `"1" < "2"`. `keep-sorted-format="numeric"` compares them as numbers instead:

```python
numbers = [
    # <block keep-sorted keep-sorted-format="numeric">
    2
    10
    20
    # </block>
]
```

It combines with `keep-sorted-pattern` to pull numbers out of mixed content:

```python
items = [
    # <block keep-sorted keep-sorted-format="numeric" keep-sorted-pattern="id: (?P<value>\d+)">
    "id: 2  banana",
    "id: 10 orange",
    "id: 20 apple",
    # </block>
]
```

Without `keep-sorted-format="numeric"` that block would fail, since `"10"` is lexicographically less
than `"2"`.

## Notes

- Blank lines and lines the pattern does not match are ignored, not treated as out of order.
- Comparison is on the trimmed line, so indentation does not affect ordering.
- An unrecognized `keep-sorted` or `keep-sorted-format` value is a hard error, not a violation.
- Pairs naturally with [`keep-unique`](keep-unique.md) on the same block.

---

← [Validators](README.md) · [README](../../README.md)
