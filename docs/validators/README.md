# Validators

Every rule is declared as an attribute on a `<block>` tag inside a comment. One block can carry several attributes at
once.

A start or end tag that fails to parse — most commonly a missing closing `>` — fails the whole run with `Malformed
block tag at line N, column N`, rather than being silently ignored.

## Which validator do I want?

<!-- <block name="validators-index" affects=".agents/skills/blockwatch/SKILL.md:validator-catalog"
     same-as="src/validators/mod.rs:validator-registry, README.md:available-validators"
     same-as-pattern='\[`(?P<value>[a-z-]+)`\]\('> -->

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

<!-- </block> -->

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

A name must be unique within its file — a second block reusing one is a hard error, since every
`affects`/`same-as` reference to it would be ambiguous.

Naming a block also makes its violations suppressible one at a time: only a named block's violations carry the address
that [`--suppress`](../cli.md#suppressing-a-violation) points at. An unnamed block's violations can only be suppressed
along with the rest of their file.

### `severity`

Controls how a violation is reported. Mirrors
[LSP diagnostic severities](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#diagnostic).

<!-- <block name="severity-levels" affects="src/blocks.rs:block-severity"> -->

| Value             | Reported | Exit code |
|-------------------|----------|-----------|
| `error` (default) | yes      | 1         |
| `warning`         | yes      | 0         |
| `info`            | yes      | 0         |
| `hint`            | yes      | 0         |

<!-- </block> -->

Only `error` fails the run. This makes `severity` the way to introduce a rule without breaking CI on day one — land the
block as a `warning`, clean up the existing violations, then promote it:

```python
# <block keep-sorted severity="warning">
"cherry",
"apple",
# </block>
```

`severity` is declared beside the code and applies to every violation the block will ever produce.
[`--suppress`](../cli.md#suppressing-a-violation) is passed on the command line and applies to violations that were
already reported. Lower the severity when the rule is genuinely advisory here; suppress when the rule is right in
general and wrong about this one case.

## When a block is checked

With a diff on stdin, a block is validated only if the diff touched its content or its start tag. Without a diff, every
block in scope is validated.

`affects` is the exception in the other direction: it is inert without a diff, since it asks "was this edited without
its counterpart?" — a question a full-tree run cannot answer. Every other validator runs in both modes.

---

← [README](../../README.md)
