# CLI reference

Run `blockwatch --help` for the generated version of this.

## Options

[//]: # (<block name="cli-docs">)

- **List Blocks**: `blockwatch list` outputs a JSON report of all found blocks.
- **Extensions**: Map custom extensions: `blockwatch -E cxx=cpp`
- **Disable Validators**: `blockwatch -d check-ai`
- **Enable Validators**: `blockwatch -e keep-sorted`
- **Ignore Files**: `blockwatch --ignore "**/generated/**"`

[//]: # (</block>)

## Selecting files

```shell
# Everything under the current directory, honoring .gitignore
blockwatch

# Only these globs
blockwatch "src/**/*.rs" "**/*.md"

# Exclude paths
blockwatch "**/*.rs" --ignore "**/generated/**"
```

Quote glob patterns so the shell does not expand them first.

## Checking only what changed

Pipe a unified diff on stdin and only the blocks it touched are validated. This is what makes
BlockWatch cheap enough for a pre-commit hook — see [CI integration](ci.md).

```shell
# Unstaged changes
git diff --patch | blockwatch

# Staged changes
git diff --cached --patch | blockwatch

# A single file's changes
git diff --patch path/to/file | blockwatch

# Changed blocks, plus some files that are always checked
git diff --patch | blockwatch "src/always_checked.rs" "**/*.md"
```

A block is validated when the diff intersects its content or its start tag. If a rule is not firing
when you expect it to, check that the diff actually hit the block's line range — `blockwatch list
--diff` shows this directly.

## Extensions

Language is resolved by file extension. Map an unrecognized extension onto a supported grammar:

```shell
blockwatch -E cxx=cpp -E c++=cpp
```

Files whose extension resolves to no grammar are skipped.

## Enabling and disabling validators

```shell
blockwatch -d check-ai                 # everything except check-ai
blockwatch -e keep-sorted -e keep-unique   # only these two
```

`-e` and `-d` cannot be combined in one invocation.

## `list`

Dumps every block found, without validating anything. Useful for auditing annotations or working out
why a rule did not fire.

```shell
# All blocks under the current directory
blockwatch list

# Restricted to globs
blockwatch list "src/**/*.rs" "**/*.md"

# Mark blocks touched by a diff
git diff | blockwatch list --diff
```

`list` does **not** read stdin unless you pass `--diff`, so it never blocks waiting for input. That
makes it safe to run non-interactively — in CI, or when invoked by an AI agent — and to pipe
elsewhere: `blockwatch list "src/**/*.ts" | jq`.

With `--diff`, the `is_content_modified` field reports which blocks the diff touched.

### Output

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

## Exit codes

| Code | Meaning                                                         |
|------|-----------------------------------------------------------------|
| 0    | No violations, or only violations with a non-`error` [severity](validators/README.md#severity) |
| 1    | At least one `error`-severity violation                          |

---

← [README](../README.md)
