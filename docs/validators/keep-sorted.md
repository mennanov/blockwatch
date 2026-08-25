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

Values are compared as exact decimals of any length, so identifiers far beyond the range of a 64-bit
float still order correctly. A value may carry a sign, a fractional part and an exponent
(`-1`, `.5`, `1e9`); `inf` and `NaN` are not numbers a source file can hold and are rejected.

`_` is accepted as a digit separator, so long literals keep the spelling their language gives them:

```rust
const LIMITS: [u64; 3] = [
    // <block keep-sorted keep-sorted-format="numeric">
    1_000,
    1_000_000,
    1_000_000_000,
    // </block>
];
```

A separator must sit between two digits — `_1`, `1_`, `1__0` and `1_.0` are typos, not numbers.

## Notes

- Blank lines and lines the pattern does not match are ignored, not treated as out of order. A line
  whose match is empty is ignored as well.
- Comparison is on the trimmed line, so indentation does not affect ordering. A pattern is applied
  to the trimmed line too, so `^` and `$` anchor to the entry rather than to the indentation.
- An unrecognized `keep-sorted` or `keep-sorted-format` value is a hard error, not a violation.
- Under `numeric`, a line whose value is not a number is a hard error too.
- Pairs naturally with [`keep-unique`](keep-unique.md) on the same block.

---

← [Validators](README.md) · [README](../../README.md)
