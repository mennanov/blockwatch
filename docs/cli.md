# CLI Reference

For command-line flag documentation directly in your terminal, run `blockwatch --help`.

## Quick Options Reference

<!-- <block name="cli-docs" same-as="src/flags.rs:cli-flags" same-as-mode="subset"
     same-as-pattern='blockwatch --(?P<value>[a-z-]+)'> -->

- **Select Files**: `blockwatch src/main.rs docs "**/*.md"` checks only those files, directories and globs. An
  argument that selects no file to check fails the run.
- **Read a Diff**: `git diff --patch | blockwatch --diff` marks which blocks the diff changed.
- **Only Changed Blocks**: `git diff --patch | blockwatch --diff --only-changed` narrows the run to them, instead of
  every block in the repository.
- **List Blocks**: `blockwatch list` outputs a JSON report of all discovered blocks. It takes `--diff`,
  `--only-changed`, `--config`, `-E` and `--ignore`.
- **Print the Agent Skill**: `blockwatch skill` prints the [skill for AI agents](#the-skill-command), as a `SKILL.md`
- **Config File**: `blockwatch --config FILE` reads the project's settings from FILE instead of `blockwatch.toml`
- **Custom Extensions**: Map custom file extensions: `blockwatch -E cxx=cpp` (config key: `extensions`)
- **Disable Validators**: `blockwatch -d check-ai` (config key: `disable`)
- **Run Only Some Validators**: `blockwatch -e keep-sorted` runs only `keep-sorted` (config key: `enable`)
- **Check Only Some Blocks**: `blockwatch --only-block FILE:BLOCK_NAME` checks only that block (config key:
  `only-blocks`)
- **Skip Blocks**: `blockwatch --skip-block FILE:BLOCK_NAME` checks every block but that one (config key: `skip-blocks`)
- **Ignore Files**: `blockwatch --ignore "**/generated/**"` (config key: `ignore`)
- **Report What Ran**: `blockwatch --verbosity summary` (or `full` for JSON on stdout)
- **Suppress Violations**: `blockwatch --suppress FILE[:BLOCK[:VALIDATOR[:HASH]]]` reports them but stops them failing
  the run
- **Suppress From File**: `blockwatch --suppress-from FILE` reads suppressions from a text file (e.g., commit message)
- **Violation Format**: `blockwatch --format sarif` writes a SARIF log instead of the JSON diagnostics

<!-- </block> -->

## Selecting Files

By default, `blockwatch` scans every file in the repository, respecting `.gitignore`. Paths are reported relative to the
repository root, whichever directory you run from. Linked worktrees and submodules are supported too: a run inside one
checks that repository only.

VCS directories like `.git`, `.hg`, `.jj` and `.svn` are skipped.

```shell
# Check everything in the repository
blockwatch

# Check one file and one directory
blockwatch src/main.rs docs

# Restrict checks to specific glob patterns
blockwatch "src/**/*.rs" "**/*.md"

# Exclude specific paths
blockwatch "**/*.rs" --ignore "**/generated/**"
```

Note: Quote glob patterns to prevent shell expansion before passing arguments to `blockwatch`.

Each argument is a path or a glob:

- A path to a file checks that file, even when its name is also a glob, such as `app/[id].tsx`.
- A path to a directory checks every file under it. `.` checks the whole repository.
- Anything else is a glob.

Paths and globs start from the repository root, whichever directory you run from. So from `src/`, write `src/main.rs`,
not `main.rs`. An absolute path works too.

An argument that selects no file to check fails the run. A file is not checked if `.gitignore`, `--ignore` or the
`ignore` key in the config file leaves it out, or if its extension is not supported. With `--only-changed`, an
argument only has to select a file in the repository, not a changed one.

Exclusions that hold for the whole project belong in the `ignore` key of the [config file](#config-file).

Paths and globs **intersect** with whatever the run mode selected, in every mode. They only ever narrow a run:
passing `"src/**/*.rs"` alongside a diff checks the changed blocks under `src/`, and never adds an unchanged file back.

Paths and globs choose which blocks are **validated**, not which files a rule may **resolve a reference against**. A
rule such as [`affects`](validators/affects.md) still finds its target in a file they leave out, so narrowing a run
to one language does not turn every cross-language rule into a failure. An excluded file is read to answer the
reference and for nothing else: it is never validated, and never appears in a run report.

## Run Modes

Which files are parsed and which blocks are validated are two separate decisions, and each has its own flag.

| Invocation                         | Files parsed               | Blocks validated                                     |
|------------------------------------|----------------------------|------------------------------------------------------|
| `blockwatch`                       | Every file in scope        | Every block found; none counts as changed            |
| `blockwatch --diff`                | Every file in scope        | Every block found; the diff marks which ones changed |
| `blockwatch --diff --only-changed` | Only the files in the diff | Only the blocks the diff touched                     |

```shell
# Check every block in the repository
blockwatch

# Check every block, and enforce the rules that need a diff
git diff --patch | blockwatch --diff

# Check only the blocks the diff changed
git diff --patch --unified=0 | blockwatch --diff --only-changed

# The same, for staged and unstaged changes
git diff --patch --unified=0 HEAD | blockwatch --diff --only-changed

# Changed blocks under specific paths or globs only
git diff --patch | blockwatch --diff --only-changed src "**/*.md"
```

A block counts as changed when the diff overlaps its line range or its start tag. To inspect which blocks a diff
touches, use `blockwatch list --diff`.

The hooks and the GitHub Action run `--diff` on its own. It checks the whole repository and still enforces the rules that
fire only on changed content. `--only-changed` cuts a run down to the changed blocks, for when a full scan costs too
much — see [Checking Only Changed Blocks](ci.md#checking-only-changed-blocks).

### Rules of the Modes

- **stdin is never read without `--diff`.** A diff piped to a bare `blockwatch` is ignored and the whole tree is
  scanned.
- **`--only-changed` requires `--diff`.** Without a diff there is nothing to narrow the run down to.
- **`--diff` with a terminal on stdin is an error.** No diff is coming, and quietly scanning the tree instead would hide
  that.
- **Under `--diff`, an empty diff means nothing changed.** It is what `git diff` prints on a clean tree. `--diff` then
  checks every block, and `--diff --only-changed` checks none.
- **Under `--diff`, stdin that cannot be a diff is an error.** Input carrying ANSI color escapes (produce the diff with
  `--color=never`) and input with no unified-diff header are each reported by name rather than treated as "nothing
  changed".
- **A diff that resolves to nothing is an error.** Under `--only-changed` the diff is the scope, so any path in it that
  blockwatch would parse but cannot find fails the run. Under `--diff` alone such a path is passed over — unless *none*
  resolves, which means the diff was taken against a different root (`diff.relative=true`, a wrong `-p` level) and no
  rule that needs a diff could fire.
- **[`affects`](validators/affects.md) needs `--diff`.** It compares blocks that a diff has touched, so it reports
  nothing at all without one. `--verbosity summary` says how many blocks carry a rule that needs a diff — see
  [Run Reports](#run-reports).

### Supported Diff Input

[//]: # (<block name="diff-input" affects="src/repo_path.rs:diff-target-resolution">)

Each diff header points at a file, and `blockwatch` resolves that path against the repository:

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

A diff with a file that does not exist in the repository is an error too — but only for files blockwatch would parse.
Entries whose extension maps to no language, such as binary assets or lockfiles, contribute no blocks and are passed
over, so a diff carrying them alongside source changes still validates normally.

[//]: # (</block>)

In each case `blockwatch` stops rather than check the wrong file, reporting the diff target at fault. The messages
describe the input rather than the command that produced it, since a diff need not come from Git:

```console
$ git diff --no-prefix | blockwatch --diff
Error: diff target "rules.py" has no recognized path prefix (a/, b/, i/, w/, c/, o/).
```

With Git, that means `--no-prefix`, `diff.noprefix`, or a custom `diff.srcPrefix` / `diff.dstPrefix`. To produce
configuration-independent output in a repository that sets any of these globally:

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

The `extensions` table of the [config file](#config-file) holds the mappings a project always needs.

## Enabling and Disabling Validators

Control which validators run using `-e` (enable only) or `-d` (disable):

```shell
# Run all validators except check-ai
blockwatch -d check-ai

# Run only keep-sorted and keep-unique
blockwatch -e keep-sorted -e keep-unique
```

Note: `-e` and `-d` cannot be combined in a single invocation.

The `enable` and `disable` keys of the [config file](#config-file) make a selection the default for a project.

## Selecting Blocks

`--only-block` checks only the blocks you list, and `--skip-block` checks every block but those. Each takes the address
of one named block: `FILE:BLOCK_NAME`. A violation of a named block prints an address that starts with it, so you can
copy it from there.

```shell
# Check only the cli-docs block
blockwatch --only-block docs/cli.md:cli-docs

# Check every block but these two
blockwatch --skip-block docs/cli.md:cli-docs --skip-block src/flags.rs:cli-flags
```

- Repeat a flag to list more blocks. `--only-block` and `--skip-block` can't be combined.
- An unnamed block has no address. Give it a `name` to select it. To leave out a whole file, use a path argument or
  `--ignore`.
- An address that matches no block fails the run: a missing file, a file without a parser, or no block with that name
  in the file. So a renamed block can't quietly turn its checks on or off.
- The block doesn't have to be in this run. An address of a block that a path argument, `--ignore` or `--only-changed`
  leaves out is not an error.
- A block is checked only if every filter lets it through: paths and globs, `--ignore`, `-e` and `-d`, `--only-changed`,
  and these flags.
- A skipped block still exists. `affects` and `same-as` can still refer to it, and `blockwatch list` still shows it. No
  validator runs on it, and `--verbosity` doesn't count it.

The `only-blocks` and `skip-blocks` keys of the [config file](#config-file) make a selection the default for a project.
A flag replaces the config's selection. So a block that needs the network can be skipped in `blockwatch.toml`, and a
scheduled job can run just that block:

```toml
skip-blocks = ['src/models.rs:latest-model']
```

```shell
blockwatch --only-block src/models.rs:latest-model
```

## Config File

Settings that stay the same for a project can live in `blockwatch.toml` at the repository root. Then the pre-commit
hook, the CI job and local scripts no longer each repeat them:

```toml
ignore = ['**/generated/**', 'tests/testdata/**']
disable = ['check-ai']

[extensions]
cxx = 'cpp'
webmanifest = 'json'
```

- The file is read from the repository root, whichever directory you run from.
- Without the file, a run uses the flags alone.
- `--config FILE` reads another file instead. The path is relative to the working directory, not to the repository
  root, and the file must exist.
- The settings apply to `list` too, except `only-blocks` and `skip-blocks`. `list` shows every block.

Write globs in single quotes. TOML then takes them as written, backslashes included.

Each key matches a flag:

| Key           | Flag           | When the flag is given too                        |
|---------------|----------------|---------------------------------------------------|
| `ignore`      | `--ignore`     | Both lists apply.                                 |
| `extensions`  | `-E`           | Both apply. The flag wins for the same extension. |
| `enable`      | `--enable`     | The flags' selection replaces the config's.       |
| `disable`     | `--disable`    | The flags' selection replaces the config's.       |
| `only-blocks` | `--only-block` | The flags' selection replaces the config's.       |
| `skip-blocks` | `--skip-block` | The flags' selection replaces the config's.       |

So `blockwatch -e keep-sorted` runs only `keep-sorted`, whatever the config enables or disables. An extra `--ignore`
still skips the files the config skips.

A value is checked as the flag's would be. An unknown validator, a glob that does not compile, an extension mapped to an
unsupported language, a block address that is not `FILE:BLOCK_NAME`, or both `enable` and `disable` (or both
`only-blocks` and `skip-blocks`) in one file all fail the run. So does an unknown key, so a typo
cannot turn a setting off without anyone noticing. The error quotes the bad key or value. For an unknown key or a value
of the wrong type, it also shows the line and column:

```text
Error: invalid config file "blockwatch.toml"

Caused by:
    TOML parse error at line 1, column 1
      |
    1 | ignor = ['x']
      | ^^^^^
    unknown field `ignor`, expected one of `ignore`, `extensions`, `enable`, `disable`, `block`
```

Everything else stays out of the file:

- **The paths and globs, `--diff`, `--only-changed`, `--suppress` and `--suppress-from`** describe one run, not the
  project.
- **`--format` and `--verbosity`** depend on who reads the output.
- **The environment variables of `check-ai` and `check-lua`** stay in the environment. Anyone who can open a pull
  request can edit the config file. From there, unsafe Lua could run any code in CI, and an API URL could send the code
  and the API key to any server.

### Blocks in the Config File

A `[[block]]` entry in the config file declares a block around one [symbol](symbols.md), instead of tags in a comment.
Such a block is a **virtual block**. Use one when:

- The file can't hold a comment, such as `package.json`.
- The rule is about one value, such as a version. The block then sees just the value, without the key and the quotes.
- The file should stay free of tags, or the project keeps its rules in one place.

For example:

```toml
# When the React version changes, the install guide must change too.
[[block]]
target = 'package.json#/dependencies/react'
name = 'react-version'
affects = 'docs/install.md:react-version'

# A scalar is checked as its value, without the key and the quotes.
[[block]]
target = 'package.json#/version'
line-pattern = '^\d+\.\d+\.\d+$'

[[block]]
target = 'package.json#/keywords'
keep-sorted = true
keep-sorted-pattern = '^"(?P<value>[^"]+)",?$'
```

- `target` is required. It is a symbol in a file, such as `package.json#/version`. Any other target, such as a whole
  file or a named block, is an error.
- Every other key is an attribute, with the same name and meaning as in a tag. An unknown attribute is an error.
- A value is a string. `true` stands for an attribute without a value, so `keep-unique = true` is `keep-unique`. An
  integer counts as its digits, so `check-lua-timeout = 30` works. Any other value is an error.
- A symbol has at most one block. Put all its attributes in one entry: a second entry for the same symbol is an error.

A virtual block is a block of the file it wraps:

- **Its content is what a reference to the symbol reads.** A scalar gives its value: `1.2.3`, not
  `"version": "1.2.3"`. An object, a list or a table gives its text, key included.
- **Its violations are reported in that file.** A violation of the whole block points at the first line of the symbol.
  A violation inside a scalar points at the start of the value.
- **Its `name` is a block name in that file.** A name that another block in the file already has is an error. A
  reference such as `affects="package.json:react-version"` finds the block, as it finds a tag.
- **The run's filters apply to that file.** When the paths and globs, `--ignore` or `.gitignore` leave the file out,
  its virtual blocks are not checked. A reference to one of them still finds it.
- **It counts as changed when the diff touches the symbol or the entry.** A change elsewhere in either file does not
  count. The entry plays the part of the start tag. So when you edit an entry, `--diff --only-changed` checks its
  block, even if the diff does not touch the file the block wraps.

A target that does not resolve stops the run when its file is checked or a reference reads that file. That is a
missing file, a file without symbols or with a syntax error, a missing or ambiguous symbol, or a TOML table written in
several places. The error shows the entry's line:

```text
Error: invalid block at line 7 of "blockwatch.toml"

Caused by:
    0: target package.json#/versoin does not resolve
    1: symbol not found; did you mean: /version
```

[`blockwatch list`](#the-list-command) shows a virtual block under the file it wraps.

## Suppressing a Violation

`--suppress` takes the address of a violation and stops it failing the run. The violation is still reported, at its
declared severity, marked `"suppressed": true`; only the exit code changes.

```shell
blockwatch --suppress docs/cli.md:cli-docs:keep-sorted
```

Use it when a rule is wrong at one particular site and you do not want to edit the source or turn the validator off
everywhere with `-d`. Repeat the flag to suppress several violations.

### The Address of a Violation

```text
FILE[:BLOCK_NAME[:VALIDATOR[:HASH]]]
```

Every violation of a named block carries its full address in the diagnostics, so the usual way to write a `--suppress`
flag is to copy one from the output.

- `FILE:BLOCK_NAME` is the same `file:name` grammar [`affects`](validators/affects.md) and
  [`same-as`](validators/same-as.md) use, and identifies exactly one block. As there, a file path containing a `:`
  cannot be addressed.
- `VALIDATOR` is the rule that reported the violation, spelled as in `-d` and `-e`.
- `HASH` is an opaque hex string telling one violation of a block from its siblings, emitted by the five validators that
  can report several: `keep-sorted`, `keep-unique`, `line-pattern`, `affects` and `same-as`.

**Every length is valid, and the shorter it is, the more it covers.** Only `FILE` is required:

| Address                          | Suppresses                                          |
|----------------------------------|-----------------------------------------------------|
| `FILE`                           | every violation in that file, named blocks or not   |
| `FILE:BLOCK_NAME`                | every violation of that block                       |
| `FILE:BLOCK_NAME:VALIDATOR`      | every violation that validator reports on the block |
| `FILE:BLOCK_NAME:VALIDATOR:HASH` | exactly one violation                               |

**A block with no `name` can only be suppressed file-wide.** Its violations carry no `address` in the diagnostics,
because there is nothing narrower to point at, but a `FILE` address covers the whole file and so covers them too.

An invalid address that covers nothing is ignored.

### Suppressing from a File

`--suppress-from <FILE>` reads a text file and applies every matching line it finds as a `--suppress`:

```text
blockwatch-suppress: api.md:handler:affects
```

Matching is case-insensitive (e.g. `BLOCKWATCH-SUPPRESS:` is accepted). Any other line is ignored, so an ordinary commit
message is valid input. A `commit-msg` hook can pass the message file that Git gives it, as the
[plain `commit-msg` hook](ci.md#plain-git-hook) does. In CI, collect the messages of a pull request:

```shell
git log --format=%B "$BASE..$HEAD" > msgs
git diff --patch "$BASE...$HEAD" | blockwatch --diff --suppress-from msgs
```

Repeat the flag to read from multiple files.

The file may sit anywhere the run can read, inside the repository or not, because the path comes from the command line
rather than from any scanned file. Over a pull request range the trailers are written by whoever opened it — see
[Suppressing Violations From a Job](ci.md#suppressing-violations-from-a-job) for when to rely on that.

## SARIF Output

`--format sarif` writes the violations as a [SARIF 2.1.0](https://docs.oasis-open.org/sarif/sarif/v2.1.0/sarif-v2.1.0.html)
log instead of the JSON diagnostics. It goes to the same place they do, **stderr**, so the run report keeps stdout to
itself:

```shell
blockwatch --format sarif 2> blockwatch.sarif
```

Nothing else about the run changes: the same violations are found, and the exit code is decided the same way.

<!-- <block name="sarif-example" same-as-pattern='v?(?P<value>\d+\.\d+\.\d+)'> -->

```json
{
  "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
  "version": "2.1.0",
  "runs": [
    {
      "tool": {
        "driver": {
          "name": "blockwatch",
          "version": "0.10.0",
          "semanticVersion": "0.10.0",
          "informationUri": "https://github.com/mennanov/blockwatch",
          "rules": [
            {
              "id": "keep-sorted",
              "name": "keep-sorted",
              "shortDescription": { "text": "Requires the lines of a block to stay in order." },
              "helpUri": "https://github.com/mennanov/blockwatch/blob/v0.10.0/docs/validators/keep-sorted.md"
            }
          ]
        }
      },
      "columnKind": "unicodeCodePoints",
      "results": [
        {
          "ruleId": "keep-sorted",
          "ruleIndex": 0,
          "level": "error",
          "message": { "text": "Block fruits.py:fruits defined at line 2 has an out-of-order line 4 (asc)" },
          "locations": [
            {
              "physicalLocation": {
                "artifactLocation": { "uri": "fruits.py" },
                "region": { "startLine": 4, "startColumn": 5, "endLine": 4, "endColumn": 13 }
              }
            }
          ],
          "partialFingerprints": { "blockwatchAddress/v1": "fruits.py:fruits:keep-sorted:bbd61689" },
          "properties": {
            "address": "fruits.py:fruits:keep-sorted:bbd61689",
            "data": { "order_by": "asc" }
          }
        }
      ]
    }
  ]
}
```

<!-- </block> -->

What to expect from the log:

- **A clean run still writes one**, with an empty `results` array. A service that reads SARIF treats a missing log as a
  run that never happened, rather than as a run that found nothing.
- **Only the rules that fired are described.** Each carries a `helpUri` to its documentation, pinned to the version that
  produced the log.
- **`level` follows [severity](validators/README.md#severity)**: `error` and `warning` keep their names, and both `info`
  and `hint` become `note`, the weakest level SARIF consumers display.
- **A [suppressed](#suppressing-a-violation) violation is reported like any other**, with
  `"suppressions": [{"kind": "external"}]` alongside it — SARIF's own way of saying a reviewer accepted it. The
  justification is left out: it lives wherever the suppression itself is recorded.
- **`partialFingerprints` carries the violation's [address](#the-address-of-a-violation)**, so a service can match a
  violation against the same one in an earlier run. It is absent for a violation on an unnamed block, which has no
  address. The address is repeated in `properties`, where it is easier for a person to find and copy into a `--suppress`
  flag.
- **Paths are repository-relative**, the same paths the JSON diagnostics are keyed by, percent-encoded where a character
  would otherwise change how the path reads as a URI (a `#` or a space, say).
- **Lines and columns are 1-based, with an exclusive end column.** A column counts characters, which the run declares as
  `"columnKind": "unicodeCodePoints"` so that a consumer does not count UTF-16 code units instead.

`--format` cannot be combined with the `list` subcommand, which reports blocks rather than violations. To upload a log
to GitHub code scanning, see [CI Integration](ci.md#github-code-scanning).

## The `list` Command

The `list` command outputs details on all discovered blocks in JSON format without running validation. It mirrors the
[run modes](#run-modes) exactly, so `list` and the default command always agree on which blocks exist.

```shell
# List all blocks in the repository
blockwatch list

# Restrict block listing to specific paths or globs
blockwatch list src "**/*.md"

# List all blocks, marking the ones the diff changed
git diff --patch | blockwatch list --diff

# List only the blocks the diff changed
git diff --patch | blockwatch list --diff --only-changed
```

Like the default command, `blockwatch list` reads stdin only when `--diff` is explicitly provided, preventing blocking
during non-interactive scripts or pipeline commands (e.g. `blockwatch list "src/**/*.ts" | jq`).

Each block entry carries an `is_content_modified` boolean field. Without a diff nothing marks a block as changed, so it
is `false` throughout; under `--diff` it identifies the blocks the diff touched.

A [virtual block](#blocks-in-the-config-file) also has `config_line`: the line of the config file that declares it.
Its `line` and `column` are where the symbol starts.

Lines and columns are 1-based, with an exclusive end column. A column counts characters rather than bytes, so a
multi-byte character such as `é` or an emoji advances it by one. The `range` of a violation follows the same
convention: the start is inclusive and the end is exclusive (`[start, end)`).

### Output Example

[//]: # (<block name="list-output-example">)

```json
{
  "src/lib.rs": [
    {
      "attributes": {
        "affects": "README.md:supported-langs",
        "name": "languages-code"
      },
      "column": 4,
      "is_content_modified": false,
      "line": 12,
      "name": "languages-code"
    },
    {
      "attributes": {
        "keep-sorted": "asc"
      },
      "column": 8,
      "is_content_modified": false,
      "line": 40,
      "name": "(unnamed)"
    }
  ],
  "package.json": [
    {
      "attributes": {
        "line-pattern": "^\\d+\\.\\d+\\.\\d+$"
      },
      "column": 3,
      "config_line": 7,
      "is_content_modified": false,
      "line": 3,
      "name": "(unnamed)"
    }
  ]
}
```

[//]: # (</block>)

A block with no `name` attribute is reported as `(unnamed)`. Within a file, blocks appear in source order and each
block's attributes are sorted by name; the files themselves are not ordered, and two runs over an unchanged tree may
emit them differently. Sort by key downstream if you need to diff one run against another — or use
[`--verbosity full`](#full-reports), which does order its files.

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
blockwatch: mode=all, 34/240 files, 61 blocks (3 unchecked, 2 need --diff), 73 checks, 0 violations
```

Reading that line:

- `mode=all` — which [run mode](#run-modes) this was: `all`, `all+diff`, or `only-changed`. Each name is a single token,
  so the line stays tokenizable on whitespace.
- `34/240 files` — 240 files were read, and 34 of them contain blocks.
- `61 blocks (3 unchecked, 2 need --diff)` — 61 blocks were in scope, no validator checked 3 of them, and 2 carry a
  rule that cannot fire at all without a diff. Fixing that means supplying one, so the figure is a prompt to change how
  you invoked `blockwatch`.
- `73 checks` — validators ran 73 times in total, once per block they applied to.
- `0 violations` — nothing failed.

**`need --diff` appears only under `mode=all`.** Every other field is present in every mode. Once a diff is supplied
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
  nothing without `--diff`. These are the blocks the `need --diff` figure counts. Under a diff the same block goes
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

Reference targets are reported by the run's scope, even though they are not resolved by it. When a block declares
`affects` or `same-as`, its target is read from disk and compared wherever it lives — but under `--only-changed` the
target appears in the report only if the diff touched it as well. A diff that changes the source alone therefore reports
a single file, even though two were involved:

```shell
git diff --patch | blockwatch --diff --only-changed --verbosity summary
blockwatch: mode=only-changed, 1/1 files, 1 block (0 unchecked), 1 check, 1 violation
```

Nothing about the failure is hidden by this. Violations are printed to stderr as JSON, keyed by the file the violating
block lives in, and the message names both sides:

```json
{
  "fileA.py": [
    {
      "address": "fileA.py:a:affects:1d7a4c02",
      "code": "affects",
      "data": {
        "affected_block_file_path": "fileB.py",
        "affected_block_name": "b"
      },
      "message": "Block fileA.py:a at line 1 is modified, but fileB.py:b is not",
      "range": {
        "end": {
          "character": 40,
          "line": 1
        },
        "start": {
          "character": 3,
          "line": 1
        }
      },
      "severity": 1
    }
  ]
}
```

`severity` follows the [LSP numbering](validators/README.md#severity): `1` error, `2` warning, `3`
info, `4` hint. For a code-scanning service, ask for the same violations as SARIF instead — see
[SARIF Output](#sarif-output).

`address` is what a `--suppress` flag points at — see [Suppressing a Violation](#suppressing-a-violation). It is absent
when the block has no `name`, which leaves a file-wide address as the only way to suppress the violation. A suppressed
violation carries `"suppressed": true` alongside it; the key is absent otherwise. **A consumer that decides on
`severity` alone has to skip the suppressed violations**, which keep the severity their author declared.

The division of labour is deliberate — the report describes what the run examined, and the violation explains what went
wrong. Once the diff touches the target as well, it appears like any other block, with an empty `checks` array because a
block that carries nothing but a `name` declares no rule of its own:

```shell
git diff --patch | blockwatch --diff --only-changed --verbosity summary
blockwatch: mode=only-changed, 2/2 files, 2 blocks (1 unchecked), 1 check, 0 violations
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

## The `skill` Command

`blockwatch skill` prints the skill for AI coding agents, as a complete `SKILL.md`. The skill tells an agent where blocks
add value, how to write them, and how to check its work. The text is built into the binary, so it always matches the
installed version. The command works in any directory, even outside a repository.

Save it into your agent's skills directory:

```shell
blockwatch skill > .claude/skills/blockwatch/SKILL.md
```

The saved skill records the version that wrote it. When `blockwatch --version` prints a newer one, the skill tells the
agent to save it again. See [Annotating Codebases with AI Agents](agents.md).

## Exit Codes

| Code | Description                                                                                                                                                          |
|------|----------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `0`  | Success. No violations found, or every reported violation is either non-`error` [severity](validators/README.md#severity) or [suppressed](#suppressing-a-violation). |
| `1`  | Failure. At least one unsuppressed `error`-severity violation was detected.                                                                                          |

---

[← Return to README](../README.md)
