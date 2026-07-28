---
name: blockwatch
description: Use when writing or modifying code in a project that uses BlockWatch — proactively link co-dependent code with `<block affects=...>`/`<block name=...>` so it catches drift when one side changes without the other (an enum and its docs, a constant and its config), or assert two places hold the same value with `<block same-as=...>`. Also for lists that must stay sorted/unique or values with a strict format/size, and when editing files that contain `<block ...>` tags (affects, same-as, keep-sorted, keep-unique, line-pattern, line-count, check-ai, check-lua).
---

# BlockWatch

BlockWatch is a language-agnostic linter that enforces rules declared inside HTML-like `<block ...>` tags placed in
source-file comments. It works across Rust, Python, JS/TS, Go, Java, Markdown, YAML, TOML, HTML, and more, and can run
on the whole tree or only the changed lines of a `git diff`.

Use this skill in three situations:

- **As you write code (the default):** the moment you write something a block would guard, add the block in the same
  change — don't wait for a separate pass.
- **First-time / bulk pass:** annotating an existing project that has no blocks yet.
- **Maintaining blocks:** keeping existing blocks valid when you edit files that already contain them.

Two rules apply to all three:

- **High-value only.** A block must catch a real mistake someone could plausibly make, not decorate. Too many blocks
  create noise and get ignored. When in doubt, leave it out.
- **Only in projects that use BlockWatch.** This skill being installed, or existing `<block>` tags in the tree, means
  the project opted in. Don't add blocks to a project that doesn't use the tool.

## Annotate as you write code

This is the primary way blocks should get added: incrementally, as part of normal coding. Whenever you write or change
code matching a row in **Where blocks add value** (below), add the tag right then, using the **Validator reference**
for the syntax.

Introduce a list that should stay ordered → wrap it in `keep-sorted` in the same edit. Add a fact that also lives in
the docs or config → add `affects` in the same edit. Retrofitting later is exactly the cost this avoids.

Then run `git diff --patch | blockwatch` to confirm the new tags pass (see *Running and verifying*).

## Annotating a new project

1. Survey the repo for the patterns in the catalog below. Read the code *and* the docs/config; use `rg`/grep to find
   lists, enums, match arms, tables, and constants.
2. For each candidate, add the minimal block tag using the comment syntax of that file's language.
3. Run `blockwatch list` to confirm every new tag parses and is recognized, then run `blockwatch` to confirm all blocks
   pass on the current (clean) tree. Fix any tag you placed on already-inconsistent content.
4. Commit, then wire BlockWatch into hooks/CI (see below) so the rules are enforced from now on.

### Where blocks add value (catalog)

[//]: # (<block name="validator-catalog" affects="docs/validators/README.md:validators-index">)

| You see...                                                                                                                    | Add                      | Why                                             |
|-------------------------------------------------------------------------------------------------------------------------------|--------------------------|-------------------------------------------------|
| A hand-maintained list/enum/match that should stay ordered (dependencies, CLI flags, feature lists, route tables)             | `keep-sorted`            | Eliminates "please sort this" review nits       |
| A list that must not repeat (allowlists, IDs, registered names)                                                               | `keep-unique`            | Prevents accidental duplicates                  |
| The same fact in two places — an enum and its docs, a version constant and a changelog row, a config key and its README table | `affects` + `name`       | Forces docs/config to be updated alongside code |
| The same **value** duplicated across places — a constant and its docs, a port in code and in a manifest, an env-var set and its README table | `same-as` + `name`       | Fails when the copies actually disagree, not just when one side is touched |
| A list whose items have a strict format (slugs, semver, env-var names)                                                        | `line-pattern="<regex>"` | Catches typos at the source                     |
| A block that must not grow past N lines (public API surface, a switch mapped to a fixed enum)                                 | `line-count="<=N"`       | Flags unbounded growth                          |
| Prose or config with a natural-language rule ("must mention X", "no TODOs left")                                              | `check-ai="..."`         | Rules regex can't express                       |
| Domain logic too complex for regex                                                                                            | `check-lua="script.lua"` | Custom programmable checks                      |

[//]: # (</block>)

Prefer the deterministic validators (`keep-sorted`, `keep-unique`, `affects`, `same-as`, `line-pattern`, `line-count`)
first — they are free, fast, and need no API keys. Reserve `check-ai` for rules the cheaper validators genuinely can't
express.

When two blocks should hold the same value, prefer `same-as` over a bare `affects`: `affects` only notices that one side
was edited, while `same-as` fails when the copies actually disagree. Put reciprocal blocks on both sides (each `name`d),
and — because `same-as` also fires without a diff — a periodic full-tree `blockwatch` run (see CI below) catches drift
that a diff-only check would miss.

### Placing tags

- Tags live **inside comments**, using the host language's comment syntax. Open with `<block ...>`, close with
  `</block>`.
- The block's *content* is the lines between the two tags.
- A block is only validated when its content (or its start tag) is touched by the diff, so annotating is safe to do
  incrementally — adding a tag never retroactively fails unrelated code.

```python
DEPENDENCIES = [
    # <block keep-sorted keep-unique>
    "anyhow",
    "clap",
    "serde",
    # </block>
]
```

```rust
// <block affects="README.md:supported-langs">
pub enum Language { Rust, Python }
// </block>
```

```markdown
<!-- <block name="supported-langs"> -->

- Rust
- Python

<!-- </block> -->
```

(Editing the enum now forces you to touch the `supported-langs` block in `README.md`.)

## Validator reference

| Attribute             | Syntax                                                                            | Notes                                                                                                                                                                                                                                                             |
|-----------------------|-----------------------------------------------------------------------------------|-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `name`                | `name="foo"`                                                                      | Names a block; the target of `affects`; shown by `blockwatch list`.                                                                                                                                                                                               |
| `affects`             | `affects="file:foo"` or `affects=":foo"` (same file); comma-separate multiple     | If this block's content changes in a diff, the referenced `name="foo"` block's content must change too, else a violation. One-way by default; put `affects` on **both** blocks (each `name`d) for two-way drift detection. Only fires in diff mode.               |
| `same-as`             | `same-as="file:foo"` or `same-as=":foo"` (same file); comma-separate multiple     | This block and each referenced `name="foo"` block must hold the same **value**. Symmetric, and fires in full-tree mode too (unlike `affects`). Whole trimmed content by default.                                                                                    |
| `same-as-pattern`     | `same-as-pattern="id: (?P<value>\d+)"`                                            | Per line, compare the `value` capture group (or the whole match). Each side reads *itself*, so put a pattern on both blocks when the two are in different formats.                                                                                                  |
| `same-as-mode`        | `same-as-mode="set"` (default) `/ sequence / single / subset`                     | `set` order/duplicate-insensitive; `sequence` ordered; `single` exactly one token per side; `subset` this block's tokens must all appear in the target (directional). Governed by the source block.                                                                 |
| `same-as-format`      | `same-as-format="numeric"`                                                        | Parse tokens as numbers before comparing, so `8080` == `8080.0`. Governed by the source block.                                                                                                                                                                     |
| `keep-sorted`         | `keep-sorted` / `keep-sorted="asc"` / `keep-sorted="desc"`                        | Default `asc`, compared lexicographically.                                                                                                                                                                                                                        |
| `keep-sorted-pattern` | `keep-sorted-pattern="id: (?P<value>\d+)"`                                        | Sort by the regex capture group named `value` instead of the whole line.                                                                                                                                                                                          |
| `keep-sorted-format`  | `keep-sorted-format="numeric"`                                                    | Compare the value numerically rather than as text (`"10"` after `"2"`).                                                                                                                                                                                           |
| `keep-unique`         | `keep-unique` / `keep-unique="^ID:(?P<value>\d+)"`                                | Uniqueness on the whole line, or on the `value` capture group.                                                                                                                                                                                                    |
| `line-pattern`        | `line-pattern="^[a-z0-9-]+$"`                                                     | Every line in the block must match.                                                                                                                                                                                                                               |
| `line-count`          | `line-count="<=5"`                                                                | Operators: `<`, `>`, `<=`, `>=`, `==`.                                                                                                                                                                                                                            |
| `check-ai`            | `check-ai="Must mention 'Acme'"` + optional `check-ai-pattern="\$(?P<value>\d+)"` | LLM validation. Requires `BLOCKWATCH_AI_API_KEY` (plus optional `BLOCKWATCH_AI_MODEL`, `BLOCKWATCH_AI_API_URL`).                                                                                                                                                  |
| `check-lua`           | `check-lua="scripts/x.lua"`                                                       | Script defines `validate(ctx, content)` returning `nil` (pass) or an error string. `ctx` has `file` (repository-relative, always `/`-separated, in every run mode), `line`, `attrs`; if the block also has `affects`, `ctx.affects` is a list of the affected blocks (`{ file, name, content }`, same path format) for IO-free cross-block checks. |
| `check-lua-pattern`   | `check-lua-pattern='str = "(?P<value>[^"]+)"'`                                    | Pass only the extracted value to the script instead of the whole block. Matches **once against the entire block** (not per line); `content` is `""` when nothing matches.                                                                                          |
| `severity`            | `severity="error"` (default) `/ warning / info / hint`                            | Only `error` fails the run (exit 1); the others are reported but exit 0.                                                                                                                                                                                          |

## Maintaining blocks (editing annotated files)

When you change code in a file that contains blocks, you **MUST**:

1. **Never delete `<block>` / `</block>` tags** unless explicitly told to. Place new content inside the appropriate
   block boundaries.
2. **Respect each block's directives** as you edit: keep `keep-sorted` lists ordered, never introduce a `keep-unique`
   duplicate, make every new line match `line-pattern`, stay within `line-count`, and satisfy `check-ai` / `check-lua`
   rules.
3. **Honor `affects`:** if you change a block carrying `affects="file:name"`, you must also update the referenced
   `<block name="name">` in `file` — they are meant to move together.
4. **Verify** before claiming the change is done (see below).

## Running and verifying

You can run the `blockwatch` command directly in the shell:

```bash
blockwatch                                # validate every block in the tree
git diff --patch | blockwatch             # validate only blocks your changes touched (fast)
git diff --cached --patch | blockwatch    # staged changes only
blockwatch list                           # JSON dump of every block found (audit / debug)
blockwatch "src/**/*.rs" "**/*.md"        # restrict to globs (quote them)
blockwatch --ignore "**/generated/**"     # exclude paths
```

After editing annotated files, run `git diff --patch | blockwatch`. If it fails, read the message, fix the
sorting/duplication/pattern/sync issue, and re-run until it passes. Use `blockwatch list` to confirm a tag you just
added is parsed and seen.

The piped diff must carry Git's standard path prefixes, which a plain `git diff` produces. If BlockWatch reports that a
diff target has no recognized prefix or does not exist, the repository sets `diff.noprefix`, a custom `diff.srcPrefix`,
or `diff.relative`; re-run as `git diff --patch --default-prefix --no-relative | blockwatch`.

If `blockwatch` is not on `PATH`, install it with `cargo install blockwatch` or
`brew install mennanov/blockwatch/blockwatch`.

## Wiring into hooks and CI (do this once, after annotating)

Validating only the diff keeps these near-instant.

**pre-commit** (`.pre-commit-config.yaml`):

```yaml
- repo: local
  hooks:
    - id: blockwatch
      name: blockwatch
      entry: bash -c 'git diff --patch --cached --unified=0 | blockwatch'
      language: system
      stages: [ pre-commit ]
      pass_filenames: false
```

Without the pre-commit framework, put the same `entry` command in `.git/hooks/pre-commit` and `chmod +x` it.

**GitHub Actions** (`.github/workflows/blockwatch.yml`):

```yaml
name: blockwatch
on:
  pull_request: { branches: [ main ] }
  push: { branches: [ main ] }
permissions: { contents: read }
jobs:
  blockwatch:
    runs-on: ubuntu-latest
    steps:
      - uses: mennanov/blockwatch-action@v1
        # Only needed if you use check-ai:
        # env: { BLOCKWATCH_AI_API_KEY: ${{ secrets.BLOCKWATCH_AI_API_KEY }} }
```

Validating the PR diff is enough for `affects`/drift checks. A periodic full-tree `blockwatch` run (no diff) on `main`
is a good extra safety net for the deterministic validators.
