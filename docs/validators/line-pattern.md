# `line-pattern`

Requires every line in the block to match a regex. Catches typos in lists whose items have a strict
format — slugs, semver strings, env-var names.

## Syntax

| Attribute      | Value | Default |
|----------------|-------|---------|
| `line-pattern` | regex | —       |

## Example

```python
slugs = [
    # <block line-pattern="^[a-z0-9-]+$">
    "valid-slug",
    "another-one",
    # </block>
]
```

Adding `"Not A Slug"` to that list fails the run.

## Notes

- Matching is against the **trimmed** line, so leading indentation does not need to be in the regex.
- Blank lines are skipped.
- The pattern is used as a search, not an implicit full-line match — anchor it with `^` and `$` if
  you want the whole line to conform, as in the example above.
- Only the **first** non-matching line in a block is reported; fix it and re-run to see the next.
- An invalid regex is a hard error, not a violation.

---

← [Validators](README.md) · [README](../../README.md)
