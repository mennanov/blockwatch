---
name: blockwatch
description: Use when a change touches something that another place must match, such as an enum and its docs, a constant and its config, a version written in two files, or a list that must stay sorted or unique. Also use it when a file you edit holds `<block ...>` tags, or `blockwatch.toml` points at it.
---

# blockwatch

This text is the output of `blockwatch skill`. If you read it from a saved copy instead, replace the copy with the
stub in [Installing this skill](#installing-this-skill). A saved copy goes stale when blockwatch is upgraded.

blockwatch enforces rules written in `<block ...>` comments or in `blockwatch.toml`. Add only high-value blocks that
catch real mistakes someone could plausibly make. Too many blocks create noise and get ignored.

## Workflows

### 1. Maintaining existing blocks (when editing annotated files)

1. **Keep tags intact.** Never delete `<block>` / `</block>` tags unless told to. Put new items inside the block.
   Deleting a block silently removes its rule.
2. **Check `blockwatch.toml` before editing.** A `[[block]]` entry can wrap a key, a regex match, or a whole file with
   no visible tags. Update the entry in the same change if you rename or move what it wraps.
3. **Honor every validator on touched blocks.** Keep `keep-sorted` lists ordered, avoid `keep-unique` duplicates, match
   `line-pattern`, stay within `line-count`, update `affects` targets, and keep `same-as` values aligned.
4. **Update renamed addresses.** If you rename or move a `name`d block, update `FILE:BLOCK_NAME` in all `affects`,
   `same-as`, `only-blocks`, and `skip-blocks` references.
5. **Verify.** Run `git diff --patch | blockwatch --diff`. Fix every violation your change caused. Report pre-existing
   unrelated ones to the user.

### 2. Annotating as you code (default)

When you write or change code matching **Where blocks add value** below, wrap it (or add a `[[block]]` entry) in the
same change. Then run `git diff --patch | blockwatch --diff`, plus `blockwatch explain FILE:NAME` on any new pattern,
glob, or `#/path` rule.

### 3. Annotating an existing project

1. Search code, docs, and config (`rg`) for copied values, code-to-doc mirrors, ordered or unique lists, and strict line
   formats.
2. Add minimal `<block>` tags in comments, or `[[block]]` entries in `blockwatch.toml` for files that can't hold
   comments (JSON) or must stay untouched (prompts, migrations).
3. Run `blockwatch list` to confirm every tag parses, `blockwatch explain FILE:NAME` on pattern/glob rules, and
   `blockwatch` on the clean tree.
4. Put project-wide ignores, extension maps, and validator defaults in `blockwatch.toml`.
5. Wire blockwatch into [hooks and CI](#wiring-into-hooks-and-ci).

## Where blocks add value

| You see...                                                      | Use            | Why                                                                 |
|-----------------------------------------------------------------|----------------|---------------------------------------------------------------------|
| Code and docs/config that must change together                  | `affects`      | Fails `--diff` if this block changes without its target (one-way)   |
| The same value copied across code, docs, config, or paths       | `same-as`      | Fails whenever copies disagree, even without a diff                 |
| A hand-maintained list, enum, or match that should stay ordered | `keep-sorted`  | Prevents out-of-order entries and review nits                       |
| A list of IDs, routes, or names that must not repeat            | `keep-unique`  | Catches accidental duplicates                                       |
| Items with a strict format, or text that must never appear      | `line-pattern` | Enforces a per-line regex or blocks forbidden text                  |
| A list or API surface that must stay within a line budget       | `line-count`   | Bounds the number of non-blank (or matching) lines                  |
| Domain logic too complex for regex                              | `check-lua`    | Runs a local Lua 5.4 script with optional `ctx.affects` targets     |
| Prose rules regex can't express ("must mention X", "no TODOs")  | `check-ai`     | LLM check; needs `BLOCKWATCH_AI_API_KEY` (can vary across runs)     |

Prefer the built-in validators over `check-ai`: they are offline, fast, and free. Prefer `same-as` over bare `affects`
when two places share a value, because `same-as` checks equality even without a diff. To catch a change to either side,
put `affects` on both blocks, each with a `name`.

## Tag syntax

- Place `<block ...>` and `</block>` inside the host language's comments. The lines between them are the block's content.
- Mention `<block>` in a comment inside backticks (``# checks `<block>` rules``); bare `<block` opens a tag.
- Quote values with `'...'` or `"..."`. Escape embedded quotes of the same kind (`\"` keeps the backslash in a regex),
  or switch to the other quote kind: `keep-unique='"(?P<value>[a-z]+)"'`.
- Keep each pattern or target reference on one line; line breaks after commas between targets are fine.
- In HTML and HTML parts of PHP, only `<!-- -->` comments count; tags inside `<script>` or `<style>` are ignored.

```python
DEPENDENCIES = [
    # <block keep-sorted keep-unique>
    "anyhow",
    "clap",
    # </block>
]
```

```rust
// <block name="langs" same-as="README.md:supported-langs" same-as-pattern="(?P<value>\w+),$">
pub enum Language {
    Rust,
    Python,
}
// </block>
```

## Validator reference

| Attribute                | Example / Values                                      | Behavior                                                                          |
|--------------------------|-------------------------------------------------------|-----------------------------------------------------------------------------------|
| `name`                   | `name="foo"`                                          | Names a block (`FILE:foo`) for `affects`, `same-as`, `list`, and `explain`.       |
| `affects`                | `affects="file:foo, :bar, file, file#/path, #/path"`  | Under `--diff`, changing this block requires each target to change (one-way).     |
| `same-as`                | `same-as="file:foo, :bar, file, file#/path, #/path"`  | Compares this block's values with each target; runs with or without a diff.       |
| `same-as-paths`          | `same-as-paths="services/*/"`                         | Compares values against sorted repo paths matching a glob (trailing `/` dirs).    |
| `same-as-pattern`        | `same-as-pattern="id: (?P<value>\d+)"`                | Per-line regex extracting `value` (or named groups as records, or full match).    |
| `same-as-mode`           | `set` / `sequence` / `single` / `subset`              | How values compare. Default: `sequence` without `same-as-pattern`, `set` with.    |
| `same-as-format`         | `numeric` / `identifier` / `flat` / `grp:fmt,...`     | Compares as numbers (`8080`==`8080.0`), name words, or letters and digits.        |
| `keep-sorted`            | `keep-sorted` / `"asc"` / `"desc"`                    | Checks text order on trimmed lines (default `asc`).                               |
| `keep-sorted-pattern`    | `keep-sorted-pattern="id: (?P<value>\d+)"`            | Sorts by `value` group (or full match) instead of the whole trimmed line.         |
| `keep-sorted-format`     | `keep-sorted-format="numeric"`                        | Compares extracted values as exact numbers (`2` < `10`).                          |
| `keep-unique`            | `keep-unique` / `keep-unique="ID:(?P<value>\d+)"`     | Rejects duplicate trimmed lines, or duplicate `value` captures.                   |
| `line-pattern`           | `line-pattern='^"[a-z0-9-]+",?$'`                     | Every trimmed non-blank line as written (quotes and commas included) must match.  |
| `line-pattern-forbidden` | `line-pattern-forbidden="TODO"`                       | No trimmed non-blank line may match; use instead of unsupported look-ahead.       |
| `line-count`             | `line-count="<=5"`                                    | Bounds non-blank line count using `<`, `<=`, `==`, `>=`, or `>`.                  |
| `line-count-pattern`     | `line-count-pattern='^"\w+",$'`                       | Counts only trimmed lines that match with a non-empty value.                      |
| `check-ai`               | `check-ai="Must mention 'Acme'"`                      | LLM (`BLOCKWATCH_AI_API_KEY`, `BLOCKWATCH_AI_MODEL`, `BLOCKWATCH_AI_API_URL`).    |
| `check-ai-pattern`       | `check-ai-pattern="\$(?P<value>\d+)"`                 | Sends only matching parts joined by newlines; zero matches is a violation.        |
| `check-lua`              | `check-lua="scripts/x.lua"`                           | Runs Lua 5.4 `validate(ctx, content)` returning `nil` or an error string.         |
| `check-lua-pattern`      | `check-lua-pattern='str = "(?P<value>[^"]+)"'`        | Runs regex over whole block; passes matches as a 1-based array `content`.         |
| `check-lua-timeout`      | `check-lua-timeout="60"`                              | Whole-second wall-clock budget (default `30`, min `1`); timeout fails the run.    |
| `severity`               | `error` (default) / `warning` / `info` / `hint`       | Only `error` exits `1`; the rest report violations and exit `0`.                  |

Key rules:

- Modifier attributes require their main attribute on the same block. Exception: `same-as-pattern` may sit on a `name`d
  target block.
- Under `--diff --only-changed`, `same-as` only runs when the block *carrying* `same-as` changed. Put `same-as` on both
  `name`d sides to catch edits to either side.
- `same-as-pattern` is per-block; for whole-file, `#/path`, or `same-as-paths` targets, the referencing block's pattern
  reads both sides.
- `same-as-paths`: `*` stops at `/`, `**` crosses it; use braces (`{a,b}/*/`) for multiple globs; `.gitignore`d files and
  symlinks are skipped; `ignore` does not apply; zero matches is a violation.
- `same-as-format="identifier"` compares the words of a name, and a run of capitals is one word: `HTTPServer` ==
  `http-server`, but `Signin` != `sign-in`. `flat` compares only the lower-cased letters and digits: `Signin` ==
  `sign-in`, but `v2` != `v`.
- `keep-sorted-format="numeric"` requires every value to parse as a number; pair it with `keep-sorted-pattern` when
  lines have quotes, commas, or labels.
- `check-lua` `ctx` provides `file` (repo-relative, `/`-separated), `line`, `attrs`, and `ctx.affects` (`{file, name,
  content}` list when `affects` is also set). Controlled by `BLOCKWATCH_LUA_MODE` (`sandboxed` default, `safe`, `unsafe`).

## Symbols

A symbol points to one value or definition inside a file (`file#/path`, or `#/path` in the same file) for `affects`,
`same-as`, `check-lua` (`ctx.affects`), or `[[block]]`.

### Files with symbols

`JSON`, `TOML`, `YAML`, `Markdown`, `MDX`, `XML`, `CSS`, `Go`, `C`, `C++`, `Python`, `Starlark`, `Rust`,
`TypeScript`, `JavaScript`, `Java`, `C#`, `Kotlin`, `Swift`, `Protocol Buffers`, `HCL`, `PHP`, `Bash`,
`Makefile`, `Dart`, `Dockerfile`.

### Variants and copies

- Files mapped via `-E` or `extensions` inherit that language's symbols.
- Copy suffixes (`.dist`, `.example`, `.in`, `.sample`, `.template`) read as the base name
  (`config.toml.example#/port`).
- Variants of `Dockerfile`, `Containerfile`, and `Makefile` with `.`, `-`, or `_` (such as `Dockerfile.ci`,
  `Dockerfile-dev`, `Makefile.am`) keep their symbols, except `Dockerfile.dockerignore` and `Makefile.{PL,SH,bat,cmd}`.
  No `.` may follow a `-` or `_`: `Dockerfile-logo.png` is not a variant. Zsh (`*.zsh`) and Go module files
  (`go.mod`, `go.sum`, `go.work`) have no symbols.

### Path syntax and discovery

- **RFC 6901 paths:** `/` separates keys; 0-based indices for arrays (`#/files/0`). Escape `/` as `~1`, `~` as `~0`,
  `,` as `%2C`, `%` as `%25` (for example, `#/dependencies/@types~1node`).
- **Value filters:** `~[key=value]` picks the unique item where `key` equals `value` (`#/services/~[name=web]/port`, or
  XML `#/Project/ItemGroup/~[@Include=Serilog]/@Version`). Zero or multiple matches fail.
- **How formats map:**
  - **JSON / TOML / YAML:** keys, tables, and list items; strings read without quotes, containers as raw file text.
  - **Markdown / MDX:** heading path from `#` down (`README.md#/My project/Install`); `same-as` reads the body without
    the heading line.
  - **XML / CSS:** element names and `@attr` (`pom.xml#/project/version`, `#/Project/@Sdk`); CSS selectors/at-rules and
    properties (`theme.css#/:root/--brand`).
  - **Code files:** top-level names (`consts.go#/MaxPorts`, `main.tf#/variable/region/default`). The members of a type
    are symbols too, under the type's name, in Java, C#, Kotlin, Swift, Protocol Buffers, PHP and Dart. Strings read
    without quotes; other values stay unevaluated as written.
- **Check paths with the CLI:** a missing key is a violation (exit `1`) that prints `"did you mean: ..."` suggestions,
  and `blockwatch explain FILE:NAME` prints the exact extracted text.
- **These stop the run (exit `2`):** a missing file, multi-document YAML, and a syntax error. In a code file, a syntax
  error stops it only for paths to names near the error.

## Config file

`blockwatch.toml` at the repository root applies to every run (override with `--config FILE`). Use single quotes for
globs and regexes. Unknown keys fail the run.

### Project settings

| Key           | Equivalent flag | Merge with CLI flags                 |
|---------------|-----------------|--------------------------------------|
| `ignore`      | `--ignore`      | Adds to CLI list                     |
| `extensions`  | `-E`            | Adds to CLI map; CLI wins on ties    |
| `enable`      | `--enable`      | CLI replaces config (never mix both) |
| `disable`     | `--disable`     | CLI replaces config (never mix both) |
| `only-blocks` | `--only-block`  | CLI replaces config                  |
| `skip-blocks` | `--skip-block`  | CLI replaces config                  |

Environment variables (`BLOCKWATCH_AI_*`, `BLOCKWATCH_LUA_MODE`) stay out of `blockwatch.toml`.

### Virtual blocks (`[[block]]`)

Declare blocks in `blockwatch.toml` when a file can't hold comments (JSON), must stay unchanged (prompts, migrations),
needs rules on a single scalar without key/quote punctuation, or repeats across a glob:

```toml
[[block]]
target = 'package.json#/version'               # symbol, file, or glob (plugins/*/package.json#/version)
line-pattern = '^\d+\.\d+\.\d+$'

[[block]]
target = 'settings.py'
target-pattern = '^MODEL = "(?P<value>[^"]+)"' # needs a file target; must match once; (?s) spans lines
name = 'model'
same-as = 'config/models.json#/default'
```

- One entry per target. Use `true` for valueless attributes (`keep-unique = true`) and integers for
  `check-lua-timeout`. `affects` and `same-as` also take a list: `same-as = ['a.md:x', 'b.json#/y']`.
- A missing target, a `target-pattern` without a unique match, or an empty target glob stops the run.
- `target-if = { symbols = ['#/a'], patterns = ['^b$'] }` on a glob target keeps only the matched files with every
  symbol and a match for every regex. Those must still have the target. A full run fails when none passes.
- Counts as changed when the diff touches the wrapped content, the `[[block]]` entry, or what `target-if` checks.

## Running and verifying

```bash
blockwatch                                                # check every block in the repository
git diff --patch | blockwatch --diff                      # full scan + diff-gated rules (affects)
git diff --patch HEAD | blockwatch --diff --only-changed  # only blocks touched by diff
blockwatch list                                           # JSON dump of all parsed blocks
blockwatch explain FILE:BLOCK_NAME                        # JSON of exact extracted values and targets
blockwatch --verbosity summary                            # count of files scanned and checked
blockwatch --suppress FILE:BLOCK_NAME:VALIDATOR:HASH      # suppress a known violation (still reported)
```

- **Stdin is read only with `--diff`.** `--only-changed` requires `--diff`.
- **Always run `blockwatch explain FILE:NAME`** after adding a pattern, glob, or `#/path` rule to check what it
  extracted. A rule can pass by matching the same wrong text on both sides.
- If git sets `diff.noprefix` or `diff.relative`, run
  `git diff --patch --default-prefix --no-relative | blockwatch --diff`.
- Exit codes: `0` pass, `1` `error` violation, `2` fatal error (malformed tag, bad config, missing target file).
- Run `blockwatch --help` for all flags (`-d`, `-e`, `-E`, `--only-block`, `--skip-block`, `--format sarif`).

## Wiring into hooks and CI

**pre-commit** (`.pre-commit-config.yaml`):

```yaml
- repo: https://github.com/mennanov/blockwatch
  rev: v{{version}}
  hooks:
    - id: blockwatch  # or blockwatch-commit-msg (reads `blockwatch-suppress:` trailers)
```

- `blockwatch-commit-msg` runs at the `commit-msg` stage. Plain `pre-commit install` doesn't install that stage, and the
  hook then never runs. Run `pre-commit install --hook-type commit-msg`, or add
  `default_install_hook_types: [ pre-commit, commit-msg ]` to `.pre-commit-config.yaml`.
- With `check-ai` blocks, add `args: [ --only-changed ]` to the hook. Without it, every commit sends every `check-ai`
  block to the model.

**Plain `.git/hooks/pre-commit`** (`chmod +x`; checks files on disk, so stop if unstaged changes exist):

```sh
#!/bin/sh
if ! git diff --quiet; then
  echo 'blockwatch checks files on disk; stage or stash unstaged changes first' >&2
  exit 1
fi
diff=$(git diff --patch --cached --unified=0) || exit
if [ -z "$diff" ]; then exec blockwatch; fi
printf '%s\n' "$diff" | blockwatch --diff
```

With `check-ai` blocks, add `--only-changed` after `--diff`.

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
        # With check-ai: add `with: { only_changed: 'true' }` and `env: { BLOCKWATCH_AI_API_KEY: ... }`
```

## Installing this skill

Install the skill as a short stub, not as a copy of this text. The stub runs `blockwatch skill` each time an agent
loads it, so the skill always matches the installed version. Save the stub as `blockwatch/SKILL.md` in the project's
skills directory, such as `.claude/skills/` for Claude Code:

```markdown
---
name: blockwatch
description: Use when a change touches something that another place must match, such as an enum and its docs, a constant and its config, a version written in two files, or a list that must stay sorted or unique. Also use it when a file you edit holds `<block ...>` tags, or `blockwatch.toml` points at it.
allowed-tools: Bash(blockwatch skill)
---

# blockwatch skill

`blockwatch skill` prints the skill for the installed version:

!`blockwatch skill`

If its output is not shown above, run `blockwatch skill` and follow what it prints. If `blockwatch` is not installed,
run `uvx --from blockwatch-cli blockwatch skill` instead.
```

Some agents read their instructions from one file instead, such as `AGENTS.md`. There, add the description from the
stub and the sentence "Then run `blockwatch skill` and follow what it prints."
