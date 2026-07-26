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
