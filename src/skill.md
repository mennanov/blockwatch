---
name: blockwatch
description: Use whenever you write or change code. As you write it, add blockwatch `<block>` tags in comments, so that `blockwatch` fails when related code falls out of sync. Use `affects` for code that must change together (an enum and its docs, a constant and its config), `same-as` for two places that must hold the same value, `keep-sorted` and `keep-unique` for lists, and `line-pattern` or `line-count` for strict formats and sizes. Also use it for rules on one key of a JSON, TOML or YAML file (such as `version` in `package.json`) through `[[block]]` entries in `blockwatch.toml`, and when you edit files that hold `<block ...>` tags or that `blockwatch.toml` points at.
---

# blockwatch

`blockwatch skill` wrote this text for blockwatch {{version}}. Before you follow it, run `blockwatch --version`. If that
prints a newer version, save the output of `blockwatch skill` over this file, and read the file again.

blockwatch is a language-agnostic linter that enforces rules declared inside HTML-like `<block ...>` tags placed in
source-file comments. It works across Rust, Python, JS/TS, Go, Java, Markdown, YAML, TOML, HTML, and more. By default it
checks the whole tree; given a `git diff` on stdin plus `--diff --only-changed` it checks only the blocks that diff
changed. Project-wide settings, and blocks declared around a key instead of with tags, live in `blockwatch.toml` (see
[The config file](#the-config-file-blockwatchtoml)).

Use this skill in three situations:

- **As you write code (the default):** the moment you write something a block would guard, add the block in the same
  change, even if the project has no blocks yet. Don't wait for a separate pass.
- **First-time / bulk pass:** when the user asks you to annotate an existing project.
- **Maintaining blocks:** keeping existing blocks valid when you edit files that already contain them.

In all three, add only high-value blocks. A block must catch a real mistake someone could plausibly make, not decorate.
Too many blocks create noise and get ignored. When in doubt, leave it out.

## Annotate as you write code

This is the primary way blocks should get added: incrementally, as part of normal coding. Whenever you write or change
code matching a row in **Where blocks add value** (below), add the tag right then, using the **Validator reference**
for the syntax.

Introduce a list that should stay ordered → wrap it in `keep-sorted` in the same edit. Add a fact that also lives in the
docs or config → add `affects` in the same edit. Retrofitting later is exactly the cost this avoids.

Then run `git diff --patch | blockwatch --diff` to confirm the new tags pass (see *Running and verifying*).

## Annotating a new project

1. Survey the repo for the patterns in the catalog below. Read the code *and* the docs/config; use `rg`/grep to find
   lists, enums, match arms, tables, and constants.
2. For each candidate, add the minimal block tag using the comment syntax of that file's language. For one key of a
   JSON, TOML or YAML file, a `[[block]]` entry in `blockwatch.toml` is often simpler. A JSON file needs one, since it
   can't hold a tag.
3. Run `blockwatch list` to confirm every new tag parses and is recognized, then run `blockwatch` to confirm all blocks
   pass on the current (clean) tree. Fix any tag you placed on already-inconsistent content.
4. Put the flags every run needs, such as `--ignore` for generated code, in `blockwatch.toml`.
5. Commit, then wire blockwatch into hooks/CI (see below) so the rules are enforced from now on.

### Where blocks add value (catalog)

[//]: # (<block name="validator-catalog">)

| You see...                                                                                                                                   | Add                      | Why                                                                        |
|----------------------------------------------------------------------------------------------------------------------------------------------|--------------------------|----------------------------------------------------------------------------|
| A hand-maintained list/enum/match that should stay ordered (dependencies, CLI flags, feature lists, route tables)                            | `keep-sorted`            | Eliminates "please sort this" review nits                                  |
| A list that must not repeat (allowlists, IDs, registered names)                                                                              | `keep-unique`            | Prevents accidental duplicates                                             |
| The same fact in two places — an enum and its docs, a version constant and a changelog row, a config key and its README table                | `affects` + `name`       | Forces docs/config to be updated alongside code                            |
| The same **value** duplicated across places — a constant and its docs, a port in code and in a manifest, an env-var set and its README table | `same-as` + `name`       | Fails when the copies actually disagree, not just when one side is touched |
| A list whose items have a strict format (slugs, semver, env-var names)                                                                       | `line-pattern="<regex>"` | Catches typos at the source                                                |
| A block that must not grow past N lines (public API surface, a switch mapped to a fixed enum)                                                | `line-count="<=N"`       | Flags unbounded growth                                                     |
| Prose or config with a natural-language rule ("must mention X", "no TODOs left")                                                             | `check-ai="..."`         | Rules regex can't express                                                  |
| Domain logic too complex for regex                                                                                                           | `check-lua="script.lua"` | Custom programmable checks                                                 |

[//]: # (</block>)

Prefer the deterministic validators (`keep-sorted`, `keep-unique`, `affects`, `same-as`, `line-pattern`, `line-count`)
first — they are free, fast, and need no API keys. Reserve `check-ai` for rules the cheaper validators genuinely can't
express.

When two blocks should hold the same value, prefer `same-as` over a bare `affects`: `affects` only notices that one side
was edited, while `same-as` fails when the copies actually disagree. Put reciprocal blocks on both sides (each `name`d),
and — because `same-as` also fires without a diff — a periodic bare `blockwatch` run over the whole tree (see CI below)
catches drift that a changed-blocks-only check would miss.

### Placing tags

- Tags live **inside comments**, using the host language's comment syntax. Open with `<block ...>`, close with
  `</block>`.
- The block's *content* is the lines between the two tags.
- A block around one key of a JSON, TOML or YAML file can be a `[[block]]` entry in `blockwatch.toml` instead of
  tags (see [The config file](#the-config-file-blockwatchtoml)). A JSON file can't hold a tag, so it needs one.
- Under `--diff --only-changed` a block is only validated when its content (or its start tag) is touched by the diff, so
  annotating is safe to do incrementally — adding a tag never retroactively fails unrelated code. A bare
  `blockwatch` run checks every block in the tree, so use it to find the tags you placed on already-inconsistent
  content.

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

| Attribute             | Syntax                                                                                                                                | Notes                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
|-----------------------|---------------------------------------------------------------------------------------------------------------------------------------|----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `name`                | `name="foo"`                                                                                                                          | Gives the block a name, so `affects` and `same-as` can point at it; shown by `blockwatch list`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| `affects`             | `affects="file:foo"`, `":foo"` (same file), `"file"` (whole file) or `"file#/path"` (one [symbol](#symbols)); comma-separate multiple | If this block's content changes in a diff, each target must change too, else a violation. Only fires under `--diff`. One-way: put `affects` on **both** blocks, each with a `name`, to catch both directions. A whole-file target is satisfied by *any* change to the file; use it for files that cannot hold a comment (`.env`, lockfiles). If the file has symbols, point at one instead (`package.json#/version`): only a change to that key or its value counts. A missing target file fails the run; a missing block or key is a violation.                                                                                       |
| `same-as`             | `same-as="file:foo"`, `":foo"` (same file), `"file"` (whole file) or `"file#/path"` (one [symbol](#symbols)); comma-separate multiple | This block and each target must hold the same **value**. Unlike `affects`, it also runs on a full-tree scan. Under `--only-changed`, only a changed block that *carries* `same-as` starts the check, so put `same-as` on both blocks if a change to either side must be caught. Compares the whole trimmed content by default. A whole-file or `#/path` target has no block of its own, so **this** block's `same-as-pattern` reads it. A `#/path` target is one value: a string compares as its unquoted text, any other value as written, and a value that holds other values (an object, a table, a mapping or a list) as its text. |
| `same-as-pattern`     | `same-as-pattern="id: (?P<value>\d+)"`                                                                                                | Per line, compare the `value` capture group (or the whole match); **every** match on a line counts, and all lines flatten into one list — values are compared, not their layout. Each side reads *itself*, so put a pattern on both blocks when the two are in different formats.                                                                                                                                                                                                                                                                                                                                                      |
| `same-as-mode`        | `same-as-mode="set"` (default) `/ sequence / single / subset`                                                                         | `set` order/duplicate-insensitive; `sequence` ordered; `single` exactly one token per side; `subset` this block's tokens must all appear in the target (directional). Governed by the source block.                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| `same-as-format`      | `same-as-format="numeric"`                                                                                                            | Parse tokens as numbers before comparing, so `8080` == `8080.0`. Governed by the source block.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| `keep-sorted`         | `keep-sorted` / `keep-sorted="asc"` / `keep-sorted="desc"`                                                                            | Default `asc`, compared lexicographically.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| `keep-sorted-pattern` | `keep-sorted-pattern="id: (?P<value>\d+)"`                                                                                            | Sort by the regex capture group named `value` instead of the whole line.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| `keep-sorted-format`  | `keep-sorted-format="numeric"`                                                                                                        | Compare the value numerically rather than as text (`"10"` after `"2"`). Every line must then *be* a number, and a trailing comma is part of the trimmed line — on a real list literal pair this with `keep-sorted-pattern` to lift the number out, or the run is a hard error.                                                                                                                                                                                                                                                                                                                                                         |
| `keep-unique`         | `keep-unique` / `keep-unique="^ID:(?P<value>\d+)"`                                                                                    | Uniqueness on the whole line, or on the `value` capture group.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| `line-pattern`        | `line-pattern='^"[a-z0-9-]+",?$'`                                                                                                     | Every line in the block must match, against the **trimmed** line as the file spells it — quotes, trailing commas and all. A pattern written for the bare value (`^[a-z0-9-]+$`) rejects every line of a quoted list, valid entries included.                                                                                                                                                                                                                                                                                                                                                                                           |
| `line-count`          | `line-count="<=5"`                                                                                                                    | Operators: `<`, `>`, `<=`, `>=`, `==`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| `check-ai`            | `check-ai="Must mention 'Acme'"`                                                                                                      | LLM validation. Requires `BLOCKWATCH_AI_API_KEY` (plus optional `BLOCKWATCH_AI_MODEL`, `BLOCKWATCH_AI_API_URL`). One network call per block, and the answer is a guess: the same block can pass one run and fail the next.                                                                                                                                                                                                                                                                                                                                                                                                             |
| `check-ai-pattern`    | `check-ai-pattern="\$(?P<value>\d+)"`                                                                                                 | Send only the matching parts of the block to the model instead of the whole block, which keeps the prompt focused and the token cost down. **Every** match is sent, joined by newlines; a pattern matching nothing is a violation, not a silent pass.                                                                                                                                                                                                                                                                                                                                                                                  |
| `check-lua`           | `check-lua="scripts/x.lua"`                                                                                                           | Script defines `validate(ctx, content)` returning `nil` (pass) or an error string. `ctx` has `file` (repository-relative, always `/`-separated, in every run mode), `line`, `attrs`; if the block also has `affects`, `ctx.affects` is a list of the affected targets (`{ file, name, content }`, same path format) for IO-free cross-block checks — a whole-file target carries the file's text with `name == nil`.                                                                                                                                                                                                                   |
| `check-lua-pattern`   | `check-lua-pattern='str = "(?P<value>[^"]+)"'`                                                                                        | Pass only the extracted values to the script instead of the whole block. **Every** match contributes its value, and `content` becomes a 1-based array (read `content[1]` for a single value); the regex runs against the entire block (not per line), so it may span several lines. Empty array when nothing matches.                                                                                                                                                                                                                                                                                                                  |
| `check-lua-timeout`   | `check-lua-timeout="60"`                                                                                                              | Wall-clock budget for the script, in whole seconds; default `30`. A script that runs past it fails the run with an error, not a violation. A value below `1` is a hard error.                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| `severity`            | `severity="error"` (default) `/ warning / info / hint`                                                                                | Only `error` fails the run (exit 1); the others are reported but exit 0.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |

## Symbols

A symbol is one value inside a file, such as the `version` key in `package.json`. `affects` and `same-as` point at one
with `file#/path`, or with `#/path` for the block's own file. Use a symbol when only one key in a file matters, or when
the file cannot hold a comment. To put rules on the value itself, wrap it in a `[[block]]` entry in `blockwatch.toml`.

<!-- <block name="extensions-with-symbols" same-as-pattern="`\.(?P<value>[a-z]+)`"> -->

Only these files have symbols: JSON (`.json`, `.jsonc`), TOML (`.toml`) and YAML (`.yaml`, `.yml`). A path into any
other file fails the run.

<!-- </block> -->

- **Paths follow RFC 6901.** `/` separates keys. An array item is its position, counted from 0: `#/files/0`.
- **Some characters in a key must be escaped:** `/` as `~1`, `~` as `~0`, `,` as `%2C`, `:` as `%3A` and `%` as
  `%25`. The key `@types/node` is `#/dependencies/@types~1node`.
- **A missing key is a violation.** The message suggests similar paths.
- **A missing or broken file fails the run.** blockwatch does not guess what a broken file meant.

<!-- <block name="json-paths"> -->

**JSON.** Every key and every array item is a symbol.

- A comment is not an array item, so it does not shift the positions after it.
- A trailing comma breaks the file, even in `.jsonc`.
- A key written twice is a violation, because blockwatch can't tell which copy you mean.

<!-- </block> -->

<!-- <block name="toml-paths"> -->

**TOML.** Every key, every table and every array item is a symbol.

- A path follows the keys, however the file writes them. `[tool.ruff]` with `line-length = 88` below it is
  `#/tool/ruff/line-length`, and so is `tool.ruff.line-length = 88`.
- A quoted key is one part of the path, dots and all. Under `[a]`, `"b.c" = 1` is `#/a/b.c`.
- `[[bin]]` entries count from 0: `#/bin/0/name`. A sub-table belongs to the entry above it.
- A table covers every place it is written. `affects` counts a change to any of them. `same-as` compares the text of
  each place, in order.
- A key written twice breaks the file.

<!-- </block> -->

<!-- <block name="yaml-paths"> -->

**YAML.** Every key and every list item is a symbol. Block style and flow style give the same paths.

- A string counts as the text YAML reads, `|` and `>` blocks included. Tags and anchors are not part of a value.
  An empty value (`key:`) is an empty string.
- An alias (`*name`) is not a symbol, so a path to it or through it is not found. In a list, it still takes its
  position. `<<` is an ordinary key.
- A key that is a list or a mapping (`? [a, b]`) is skipped with everything under it.
- A key written twice is a violation.
- A file with several documents fails the run. Use a named block there instead.

<!-- </block> -->

## The config file (`blockwatch.toml`)

Every run reads `blockwatch.toml` from the repository root, from any directory: the hook, CI and your own runs.
`--config FILE` reads another file instead.

### Project-wide settings

Put the flags that every run repeats in the config:

```toml
ignore = ['**/generated/**', 'vendor/**'] # like --ignore
disable = ['check-ai']                    # like --disable; `enable` is like --enable, never set both

[extensions]
cxx = 'cpp'                               # like -E cxx=cpp
```

- Write globs and regexes in single quotes. TOML then takes them as written, backslashes included.
- `ignore` and `extensions` add to the flags. `--enable` or `--disable` on the command line replaces the config's
  selection.
- An unknown key or a bad value fails the run, so a typo can't turn a setting off.
- The settings of `check-ai` and `check-lua` stay in environment variables. The config can't hold them.

### Blocks around a key

A `[[block]]` entry declares a block around one [symbol](#symbols), with the attributes a tag would have. Prefer it to
tags when:

- The file can't hold a comment, such as `package.json`.
- The rule is about one value, such as a version. The block's content is then just the value, so a rule needs no
  pattern to skip the key and the quotes.
- The file should stay free of tags, or the project keeps its rules in one place.

```toml
# The oldest Rust that CI tests must be the one Cargo.toml promises.
[[block]]
target = 'Cargo.toml#/package/rust-version'
same-as = '.github/workflows/ci.yml#/jobs/test/strategy/matrix/rust/0'

# Bumping React must update the install guide.
[[block]]
target = 'package.json#/dependencies/react'
name = 'react-version'
affects = 'docs/install.md:react-version'

[[block]]
target = 'package.json#/version'
line-pattern = '^\d+\.\d+\.\d+$'

[[block]]
target = 'package.json#/keywords'
keep-sorted = true
keep-sorted-pattern = '^"(?P<value>[^"]+)",?$'
```

- `target` is required, and it must be `file#/path`. A whole file or a named block is not a valid target.
- `true` stands for an attribute without a value: `keep-unique = true`. An integer counts as its digits:
  `check-lua-timeout = 30`.
- A symbol has at most one entry. Put all its rules in one.
- **The content is what a reference to the symbol reads.** A string is its value without quotes, so the
  `line-pattern` above checks `1.2.3`. A list, an object or a table is its text, key line included. That is why the
  `keep-sorted-pattern` above takes the value out of each quoted item and skips the `"keywords": [` and `]` lines.
- A `name` works like a tag's. A tag in another file can point at it: `same-as="package.json:react-version"`.
- Violations are reported in the wrapped file, at the symbol. The message shows the entry's line in `blockwatch.toml`.
- The block counts as changed when the diff touches the symbol or the entry. So `--only-changed` checks a block whose
  entry you edited, even if the wrapped file did not change.
- A target that does not resolve stops the run: a missing file or key, or a file that does not parse.

To only point *at* a key from a tag, a `file#/path` target is enough. An entry is for putting rules *on* the key.

## Maintaining blocks (editing annotated files)

When you change code in a file that contains blocks, you **MUST**:

1. **Never delete `<block>` / `</block>` tags** unless explicitly told to. Place new content inside the appropriate
   block boundaries.
2. **Respect each block's directives** as you edit: keep `keep-sorted` lists ordered, never introduce a `keep-unique`
   duplicate, make every new line match `line-pattern`, stay within `line-count`, and satisfy `check-ai` / `check-lua`
   rules.
3. **Honor `affects`:** if you change a block carrying `affects="file:name"`, you must also update the referenced
   `<block name="name">` in `file` — they are meant to move together. A target without `:` or `#` is a whole file, so
   that file has to change too. A `file#/path` target is one [symbol](#symbols), so its key or its value has to change.
4. **Check `blockwatch.toml` before editing a JSON, TOML or YAML file.** A `[[block]]` entry can put rules on a key
   there, though the file shows no tag. If you rename or move that key, update the entry's `target` in the same
   change, or the run stops.
5. **Verify** before claiming the change is done (see below).

## Running and verifying

You can run the `blockwatch` command directly in the shell:

```bash
blockwatch                                                   # validate every block in the tree
git diff --patch | blockwatch --diff                         # every block, with `affects` enforced on your changes
git diff --patch HEAD | blockwatch --diff                    # the same, for staged and unstaged changes
git diff --patch | blockwatch --diff --only-changed          # only the blocks your changes touched
blockwatch list                                              # JSON dump of every block found (audit / debug)
blockwatch src/main.rs "**/*.md"                             # restrict to paths or globs (quote globs)
blockwatch --ignore "**/generated/**"                        # exclude paths for this run
```

Every run also applies the settings in `blockwatch.toml`. Put an exclusion the project always needs in its `ignore` key,
not in each command.

Stdin is read **only** with `--diff`; piping a diff without it is silently ignored and the whole tree is scanned
instead. `--only-changed` narrows the run to the blocks the diff touched and requires `--diff`.

After editing annotated files, run `git diff --patch | blockwatch --diff`. If it fails, read the message, fix the
sorting/duplication/pattern/sync issue, and re-run until it passes. A violation in a block you didn't edit is yours to
fix only if your change caused it, for example when you changed one side of a `same-as` or renamed a block that another
one points at. Otherwise leave it, and tell the user about it. Use `blockwatch list` to confirm a tag you
just added is parsed and seen.

The piped diff must carry Git's standard path prefixes, which a plain `git diff` produces. If blockwatch reports that a
diff target has no recognized prefix or does not exist, the repository sets `diff.noprefix`, a custom `diff.srcPrefix`,
or `diff.relative`; re-run as
`git diff --patch --default-prefix --no-relative | blockwatch --diff`. Under `--diff`, an empty diff
means nothing changed, but stdin that is ANSI-colorized or not a diff is an error.

## Wiring into hooks and CI (do this once, after annotating)

The hook and the GitHub Action below check every block in the repository, and enforce `affects` on the diff. A file
without a block tag is not parsed, so this stays fast.

**pre-commit** (`.pre-commit-config.yaml`):

```yaml
- repo: local
  hooks:
    - id: blockwatch
      name: blockwatch
      entry: bash -c 'diff=$(git diff --patch --cached --unified=0) || exit; if [ -z "$diff" ]; then exec blockwatch; fi; printf "%s\n" "$diff" | blockwatch --diff'
      language: system
      stages: [ pre-commit ]
      pass_filenames: false
```

Without the pre-commit framework, write this to `.git/hooks/pre-commit` and `chmod +x` it:

```sh
#!/bin/sh
if ! git diff --quiet; then
  echo 'blockwatch checks the files on disk, so stage or stash the unstaged changes first' >&2
  exit 1
fi
diff=$(git diff --patch --cached --unified=0) || exit
if [ -z "$diff" ]; then exec blockwatch; fi
printf '%s\n' "$diff" | blockwatch --diff
```

blockwatch reads the files on disk, not the staged content. The pre-commit framework sets unstaged changes aside before
it runs a hook, but a plain hook doesn't. So this hook stops when there are unstaged changes, instead of checking them
in place of the commit.

Both hooks run `blockwatch` without `--diff` when nothing is staged, because releases up to 0.8.1 reject an empty diff.

If the project has `check-ai` blocks, add `--only-changed` after `--diff`: a full scan sends every one of them to the
model on every commit.

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

Validating the PR diff is enough for `affects`/drift checks. A periodic bare `blockwatch` run over the whole tree on
`main` is a good extra safety net for the deterministic validators — but note it cannot check `affects`, which needs a
diff to compare against.
