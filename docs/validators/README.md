# Validators

Every rule is declared as an attribute on a `<block>` tag inside a comment. One block can carry several attributes at
once.

## Which validator do I want?

| I want to...                                                  | Use                               |
|---------------------------------------------------------------|-----------------------------------|
| Force docs or config to be updated whenever some code changes | [`affects`](affects.md)           |
| Assert two places still hold the **same value**               | [`same-as`](same-as.md)           |
| Keep a list alphabetized or numerically ordered               | [`keep-sorted`](keep-sorted.md)   |
| Prevent duplicate entries in a list                           | [`keep-unique`](keep-unique.md)   |
| Require every line to match a format                          | [`line-pattern`](line-pattern.md) |
| Cap or fix the number of lines in a block                     | [`line-count`](line-count.md)     |
| Enforce a rule stated in plain English                        | [`check-ai`](check-ai.md)         |
| Run arbitrary validation logic                                | [`check-lua`](check-lua.md)       |

Prefer the deterministic validators — `affects`, `same-as`, `keep-sorted`, `keep-unique`,
`line-pattern`, `line-count`. They are fast, offline, and need no API key. Reserve `check-ai` for rules the others
genuinely cannot express.

## Universal attributes

These apply to any block regardless of which validator it uses.

### `name`

Names the block so other blocks can point at it with `affects` or `same-as`. Names also show up in
[`blockwatch list`](../cli.md#the-list-command).

```python
# <block name="allowed-colors">
"red",
"green",
# </block>
```

### `severity`

Controls how a violation is reported. Mirrors
[LSP diagnostic severities](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#diagnostic).

| Value             | Reported | Exit code |
|-------------------|----------|-----------|
| `error` (default) | yes      | 1         |
| `warning`         | yes      | 0         |
| `info`            | yes      | 0         |
| `hint`            | yes      | 0         |

Only `error` fails the run. This makes `severity` the way to introduce a rule without breaking CI on day one — land the
block as a `warning`, clean up the existing violations, then promote it:

```python
# <block keep-sorted severity="warning">
"cherry",
"apple",
# </block>
```

## When a block is checked

With a diff on stdin, a block is validated only if the diff touched its content or its start tag. Without a diff, every
block in scope is validated.

`affects` is the exception in the other direction: it is inert without a diff, since it asks "was this edited without
its counterpart?" — a question a full-tree run cannot answer. Every other validator runs in both modes.

---

← [README](../../README.md)
