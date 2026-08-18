# CLI Reference

For command-line flag documentation directly in your terminal, run `blockwatch --help`.

## Quick Options Reference

[//]: # (<block name="cli-docs">)

- **Read a Diff**: `git diff --patch | blockwatch --diff` marks which blocks the diff changed.
- **Only Changed Blocks**: `git diff --patch | blockwatch --diff --only-changed` narrows the run to them, instead of
  every block in the repository.
- **List Blocks**: `blockwatch list` outputs a JSON report of all discovered blocks.
- **Custom Extensions**: Map custom file extensions: `blockwatch -E cxx=cpp`
- **Disable Validators**: `blockwatch -d check-ai`
- **Enable Validators**: `blockwatch -e keep-sorted`
- **Ignore Files**: `blockwatch --ignore "**/generated/**"`
- **Report What Ran**: `blockwatch --verbosity summary` (or `full` for JSON on stdout)

[//]: # (</block>)

## Selecting Files

By default, `blockwatch` scans all files in the current working directory, respecting `.gitignore`.

```shell
# Check everything in the repository
blockwatch

# Restrict checks to specific glob patterns
blockwatch "src/**/*.rs" "**/*.md"

# Exclude specific paths
blockwatch "**/*.rs" --ignore "**/generated/**"
```

Note: Quote glob patterns to prevent shell expansion before passing arguments to `blockwatch`.

Globs **intersect** with whatever the run mode selected, in every mode. They only ever narrow a run: passing
`"src/**/*.rs"` alongside a diff checks the changed blocks under `src/`, and never adds an unchanged file back.

## Run Modes

Which files are parsed and which blocks are validated are two separate decisions, and each has its own flag.

| Invocation                         | Files parsed                 | Blocks validated                                     |
|------------------------------------|------------------------------|------------------------------------------------------|
| `blockwatch`                       | Every file in scope          | Every block found; none counts as changed            |
| `blockwatch --diff`                | Every file in scope          | Every block found; the diff marks which ones changed |
| `blockwatch --diff --only-changed` | Only files named by the diff | Only the blocks the diff touched                     |

```shell
# Check every block in the repository
blockwatch

# Check every block, and enforce the rules that need a diff
git diff --patch | blockwatch --diff

# Check only the blocks the diff changed
git diff --patch --unified=0 | blockwatch --diff --only-changed

# The same, for staged changes
git diff --cached --patch --unified=0 | blockwatch --diff --only-changed

# Changed blocks under specific globs only
git diff --patch | blockwatch --diff --only-changed "src/**/*.rs" "**/*.md"
```

A block counts as changed when the diff overlaps its line range or its start tag. To inspect which blocks a diff
touches, use `blockwatch list --diff`.

`--only-changed` is what keeps pre-commit hooks and per-pull-request CI runs fast (see [CI Integration](ci.md)).
`--diff` on its own audits the whole repository while still enforcing the rules that fire only on changed content.

### Rules of the Modes

- **stdin is never read without `--diff`.** A diff piped to a bare `blockwatch` is ignored and the whole tree is
  scanned.
- **`--only-changed` requires `--diff`.** Without a diff there is nothing to narrow the run down to.
- **`--diff` with a terminal on stdin is an error.** No diff is coming, and quietly scanning the tree instead would hide
  that.
- **Under `--diff`, stdin that cannot be a diff is an error.** Empty input, input carrying ANSI color escapes (produce
  the diff with `--color=never`), and input with no unified-diff header are each reported by name rather than treated as
  "nothing changed".
- **[`affects`](validators/affects.md) needs `--diff`.** It compares blocks that a diff has touched, so it reports
  nothing at all without one. `--verbosity summary` says how many blocks carry a rule that needs a diff — see
  [Run Reports](#run-reports).

### Supported Diff Input

[//]: # (<block name="diff-input" affects="src/repo_path.rs:diff-target-resolution">)

Each diff header names a file, and `blockwatch` resolves that name against the repository:

- **Path prefixes are required.** Git writes each diff path behind a one-component prefix — `a/`
  and `b/` by default, or `i/`, `w/`, `c/` and `o/` under `diff.mnemonicPrefix`. Exactly one is removed, so a repository
  directory that happens to share a prefix's name (a top-level `b/`, for example) is preserved.
- **Quoted paths are decoded.** Git's default `core.quotePath` writes a name containing non-ASCII characters, tabs,
  quotes or backslashes in an escaped form such as `"b/caf\303\251.py"`; the real name is recovered from it.

Diffs written *without* prefixes — `git diff --no-prefix`, `diff.noprefix=true` — or with a custom
`diff.srcPrefix` / `diff.dstPrefix` are rejected. They cannot be distinguished from prefixed paths whose repository
directory shares the prefix's name, and guessing would risk validating a file the diff never mentioned while the changed
one went unchecked. `diff.relative=true` is likewise unsupported: it writes paths relative to the current directory, and
nothing in the diff records that it did.

Two details make the detection reliable. Git draws the two prefixes from opposite sides of the comparison — `a/` against
`b/`, or `i/`, `c/` and `o/` against `w/` — so a header repeating one prefix on both sides is recognised as unprefixed
rather than stripped. An added file is the exception: its source is `/dev/null`, which says nothing either way, so both
readings are checked against the working tree and a target where both name a real file is reported as ambiguous.

A diff naming a file that does not exist in the repository is an error too — but only for files BlockWatch would parse.
Entries whose extension maps to no language, such as binary assets or lockfiles, contribute no blocks and are passed
over, so a diff carrying them alongside source changes still validates normally.

[//]: # (</block>)

In each case `blockwatch` stops with the flag that fixes it rather than checking the wrong file:

```console
$ git diff --no-prefix | blockwatch --diff
Error: diff target "rules.py" has no recognized Git path prefix.
BlockWatch reads diffs written with the prefixes Git produces by default. This one looks like the
output of --no-prefix, diff.noprefix, or a custom diff.srcPrefix/diff.dstPrefix. Re-run with:
    git diff --default-prefix
```

To produce configuration-independent output in a repository that sets these options globally:

```shell
git diff --patch --default-prefix --no-relative | blockwatch --diff
```

`--default-prefix` requires Git 2.41 or newer. On older versions, use
`git -c diff.mnemonicPrefix=false -c diff.noprefix=false diff --patch`.

## Custom File Extension Mappings

Language detection relies on file extensions. Use `-E` to map unrecognized or custom extensions to a supported grammar:

```shell
blockwatch -E cxx=cpp -E c++=cpp
```

Files with extensions that do not map to any supported grammar are ignored.

## Enabling and Disabling Validators

Control which validators run using `-e` (enable only) or `-d` (disable):

```shell
# Run all validators except check-ai
blockwatch -d check-ai

# Run only keep-sorted and keep-unique
blockwatch -e keep-sorted -e keep-unique
```

Note: `-e` and `-d` cannot be combined in a single invocation.

## The `list` Command

The `list` command outputs details on all discovered blocks in JSON format without running validation. It mirrors the
[run modes](#run-modes) exactly, so `list` and the default command always agree on which blocks exist.

```shell
# List all blocks under the current directory
blockwatch list

# Restrict block listing to specific globs
blockwatch list "src/**/*.rs" "**/*.md"

# List all blocks, marking the ones the diff changed
git diff --patch | blockwatch list --diff

# List only the blocks the diff changed
git diff --patch | blockwatch list --diff --only-changed
```

Like the default command, `blockwatch list` reads stdin only when `--diff` is explicitly provided, preventing blocking
during non-interactive scripts or pipeline commands (e.g. `blockwatch list "src/**/*.ts" | jq`).

Each block entry carries an `is_content_modified` boolean field. Without a diff nothing marks a block as changed, so it
is `false` throughout; under `--diff` it identifies the blocks the diff touched.

### Output Example

[//]: # (<block name="list-output-example">)

```json
{
  "README.md": [
    {
      "name": "available-validators",
      "line": 18,
      "column": 10,
      "is_content_modified": false,
      "attributes": {
        "name": "available-validators"
      }
    }
  ]
}
```

[//]: # (</block>)

## Run Reports

By default `blockwatch` prints nothing when a run succeeds. That makes a check that passed look exactly like a check
that never ran. Use `--verbosity` to see what was actually checked.

| Level            | Output                                                           |
|------------------|------------------------------------------------------------------|
| `none` (default) | Nothing.                                                         |
| `summary`        | One line of counts.                                              |
| `full`           | A JSON report of every block and the validators that checked it. |

The report goes to **stdout**. Violations go to **stderr**. A run can print both, and each one can be piped and parsed
on its own.

```shell
blockwatch --verbosity summary
blockwatch: mode=all, 34/240 files, 61 blocks (3 unchecked, 2 needs --diff), 73 checks, 0 violations
```

Reading that line:

- `mode=all` — which [run mode](#run-modes) this was: `all`, `all+diff`, or `only-changed`. Each name is a single token,
  so the line stays tokenizable on whitespace.
- `34/240 files` — 240 files were read, and 34 of them contain blocks.
- `61 blocks (3 unchecked, 2 needs --diff)` — 61 blocks were in scope, no validator checked 3 of them, and 2 carry a
  rule that cannot fire at all without a diff. Fixing that means supplying one, so the figure is a prompt to change how
  you invoked `blockwatch`.
- `73 checks` — validators ran 73 times in total, once per block they applied to.
- `0 violations` — nothing failed.

**`needs --diff` appears only under `mode=all`.** Every other field is present in every mode. Once a diff is supplied
those rules *can* fire, so the question the figure answers no longer arises — and the obvious substitute, counting the
blocks the diff did not happen to reach, would just measure the size of your change. On a repository with fifty
`affects` blocks, a one-line commit would report forty-nine, every time, with nothing wrong. So the clause is left out
rather than reported as zero or as noise.

A parser should therefore read `mode=` first and expect the clause only for `all`; the remaining fields keep a fixed
shape in every mode.

A block goes unchecked for one of three reasons:

- **It is only a reference target.** A block that carries nothing but a `name` exists so that other blocks can point at
  it with `affects` or `same-as`. It declares no rule of its own, so nothing checks it. This is normal and needs no
  fixing.
- **The validator does not apply to this run.** `affects` only compares blocks that a diff has touched, so it checks
  nothing without `--diff`. These are the blocks the `needs --diff` figure counts. Under a diff the same block goes
  unchecked whenever the diff did not reach it, which is normal for an incremental run and is not counted.
- **The attributes do not add up to a rule.** A modifier such as `keep-sorted-pattern` only refines the validator it
  belongs to; on a block with no `keep-sorted`, it has nothing to modify and no validator claims the block. A `full`
  report lists every attribute as it was written, which is usually enough to see what is missing.

### Reports Under a Diff

Under `--diff --only-changed` the diff scopes the report exactly as it scopes the run: only the blocks the diff touched
are described. A block the diff never reached is *absent* from the report rather than listed with an empty `checks`
array, so `blocks_unchecked` counts only blocks that were in scope and that nothing checked. This is the same rule
`blockwatch list --diff --only-changed` follows, so the two commands always agree on which blocks exist. Under `--diff`
alone the report covers the whole tree, exactly as a run without a diff does.

Reference targets follow the run's scope too. When a block declares `affects` or `same-as`, its target is read from disk
and compared — but under `--only-changed` the target appears in the report only if the diff touched it as well. A diff
that changes the source alone therefore reports a single file, even though two were involved:

```shell
git diff --patch | blockwatch --diff --only-changed --verbosity summary
blockwatch: mode=only-changed, 1/1 files, 1 blocks (0 unchecked), 1 checks, 1 violations
```

Nothing about the failure is hidden by this. The violation on stderr names both sides:

```console
Block fileA.py:a at line 1 is modified, but fileB.py:b is not
```

The division of labour is deliberate — the report describes what the run examined, and the violation explains what went
wrong. Once the diff touches the target as well, it appears like any other block, with an empty `checks` array because a
block that carries nothing but a `name` declares no rule of its own:

```shell
git diff --patch | blockwatch --diff --only-changed --verbosity summary
blockwatch: mode=only-changed, 2/2 files, 2 blocks (1 unchecked), 1 checks, 0 violations
```

### Full Reports

`--verbosity full` describes every block the same way `blockwatch list` does, and adds a `checks` array naming the
validators that ran on it. A block checked by several validators lists all of them. The `summary` object carries the
same counts as the one-line report and follows the same rule: `blocks_needing_diff` is present only under `mode=all`,
and is absent — not zero — in the other two modes.

```json
{
  "summary": {
    "mode": "all",
    "files_scanned": 240,
    "files_with_blocks": 34,
    "files_skipped": 179,
    "blocks": 61,
    "blocks_unchecked": 3,
    "blocks_needing_diff": 2,
    "checks": 73,
    "violations": 0,
    "validators": {
      "affects": 41,
      "check-lua": 12,
      "keep-sorted": 20
    }
  },
  "files": {
    "src/validators/check_ai.rs": [
      {
        "attributes": {
          "affects": "docs/validators/check-ai.md:check-ai-env-vars",
          "name": "check-ai-env-vars",
          "same-as": "docs/validators/check-ai.md:check-ai-env-vars",
          "same-as-pattern": "BLOCKWATCH_AI_[A-Z_]+"
        },
        "checks": [
          "affects",
          "same-as"
        ],
        "column": 4,
        "is_content_modified": true,
        "line": 31,
        "name": "check-ai-env-vars"
      }
    ]
  }
}
```

Files are sorted by path, and each block's checks by validator name, so two runs over an unchanged tree print the same
bytes.

The report says which validators looked at a block, not what each one concluded. Violations are not repeated here; they
stay on stderr, under the same file paths and line numbers.

`--verbosity` cannot be combined with the `list` subcommand, because `list` already prints its own JSON to stdout.

## Exit Codes

| Code | Description                                                                                                                  |
|------|------------------------------------------------------------------------------------------------------------------------------|
| `0`  | Success. No violations found, or all reported violations have a non-`error` [severity level](validators/README.md#severity). |
| `1`  | Failure. At least one `error`-severity violation was detected.                                                               |

---

[← Return to README](../README.md)
