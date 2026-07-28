# CLI Reference

For command-line flag documentation directly in your terminal, run `blockwatch --help`.

## Quick Options Reference

[//]: # (<block name="cli-docs">)

- **List Blocks**: `blockwatch list` outputs a JSON report of all discovered blocks.
- **Custom Extensions**: Map custom file extensions: `blockwatch -E cxx=cpp`
- **Disable Validators**: `blockwatch -d check-ai`
- **Enable Validators**: `blockwatch -e keep-sorted`
- **Ignore Files**: `blockwatch --ignore "**/generated/**"`

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

## Diff Validation

When given a unified diff via stdin, `blockwatch` limits validation to blocks modified by the diff. This keeps execution
fast during pre-commit hooks and CI runs (see [CI Integration](ci.md)).

```shell
# Validate unstaged changes
git diff --patch | blockwatch

# Validate staged changes
git diff --cached --patch | blockwatch

# Validate changes in a specific file
git diff --patch path/to/file | blockwatch

# Validate diff changes alongside explicit globs
git diff --patch | blockwatch "src/always_checked.rs" "**/*.md"
```

A block is validated if the diff overlaps its line range or start tag. To inspect which blocks a diff touches, use
`blockwatch list --diff`.

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

In each case `blockwatch` stops with the flag that fixes it rather than checking the wrong file:

```console
$ git diff --no-prefix | blockwatch
Error: diff target "rules.py" has no recognized Git path prefix.
BlockWatch reads diffs written with the prefixes Git produces by default. This one looks like the
output of --no-prefix, diff.noprefix, or a custom diff.srcPrefix/diff.dstPrefix. Re-run with:
    git diff --default-prefix
```

A diff naming a file that does not exist in the repository is an error too — but only for files BlockWatch would parse.
Entries whose extension maps to no language, such as binary assets or lockfiles, contribute no blocks and are passed
over, so a diff carrying them alongside source changes still validates normally.

To produce configuration-independent output in a repository that sets these options globally:

```shell
git diff --patch --default-prefix --no-relative | blockwatch
```

`--default-prefix` requires Git 2.41 or newer. On older versions, use
`git -c diff.mnemonicPrefix=false -c diff.noprefix=false diff --patch`.

[//]: # (</block>)

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

The `list` command outputs details on all discovered blocks in JSON format without running validation.

```shell
# List all blocks under the current directory
blockwatch list

# Restrict block listing to specific globs
blockwatch list "src/**/*.rs" "**/*.md"

# Annotate output with diff status
git diff | blockwatch list --diff
```

`blockwatch list` reads stdin only when `--diff` is explicitly provided, preventing blocking during non-interactive
scripts or pipeline commands (e.g. `blockwatch list "src/**/*.ts" | jq`).

With `--diff`, each block entry includes the `is_content_modified` boolean field.

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

## Exit Codes

| Code | Description                                                                                                                  |
|------|------------------------------------------------------------------------------------------------------------------------------|
| `0`  | Success. No violations found, or all reported violations have a non-`error` [severity level](validators/README.md#severity). |
| `1`  | Failure. At least one `error`-severity violation was detected.                                                               |

---

[← Return to README](../README.md)
