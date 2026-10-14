# `line-pattern`

Checks every line in the block against a regex:

- `line-pattern` requires each line to match. It catches typos in lists whose items have a strict format — slugs,
  semver strings, env-var names.
- `line-pattern-forbidden` rejects each line that matches. It keeps text out of a block, such as `localhost` in a
  production config.

## Syntax

| Attribute                | Value | Default |
|--------------------------|-------|---------|
| `line-pattern`           | regex | —       |
| `line-pattern-forbidden` | regex | —       |

A block can have either attribute, or both.

## Example

```python
slugs = [
    # <block line-pattern='^"[a-z0-9-]+",?$'>
    "valid-slug",
    "another-one",
    # </block>
]
```

Adding `"Not A Slug"` to that list fails the run.

The pattern has to describe the line as the file actually spells it, punctuation included — here the quotes and the
trailing comma the language requires. A bare `^[a-z0-9-]+$` would reject every line in the block, the valid ones too.

## Forbidden text

```python
# <block line-pattern-forbidden="localhost|127\.0\.0\.1">
API_URL = "https://api.example.com"
DB_HOST = "db.example.com"
# </block>
```

Changing `DB_HOST` to `"localhost"` fails the run. The violation points at the matched text, `localhost`.

The regex syntax has no look-ahead, so a `line-pattern` such as `^(?!.*localhost)` is an error. Use
`line-pattern-forbidden` instead.

## Notes

- Matching is against the **trimmed** line, so leading indentation does not need to be in the regex.
- Blank lines are skipped.
- The pattern is used as a search, not an implicit full-line match — anchor it with `^` and `$` if you want the whole
  line to conform, as in the example above. A `line-pattern-forbidden` regex without anchors rejects a line that has
  the text anywhere.
- Only the **first** failing line in a block is reported; fix it and re-run to see the next. A line that fails both
  attributes is reported for `line-pattern`.
- Both attributes report their violations as `line-pattern`, so `--disable line-pattern` turns off both.
- An invalid regex is a hard error, not a violation.

---

← [Validators](README.md) · [README](../../README.md)
