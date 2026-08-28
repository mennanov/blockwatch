# `check-lua`

Runs custom validation logic written in Lua (5.4). The escape hatch for domain rules the other validators cannot
express.

The script defines a global `validate(ctx, content)` that returns `nil` when validation passes, or an error message
string when it fails.

## Syntax

| Attribute           | Value                                                     | Default     |
|---------------------|-----------------------------------------------------------|-------------|
| `check-lua`         | path to a `.lua` script, relative to the project root     | —           |
| `check-lua-pattern` | regex; every match's `(?P<value>…)` group, or whole match | whole block |
| `check-lua-timeout` | whole number of seconds (≥ 1) the script may run          | `30`        |

The script path must point to a file inside the repository. Paths that escape it — absolute paths outside the project,
`../` traversal, or symlinks pointing outward — are rejected.

A leading `#!` line and a UTF-8 byte order mark are skipped, as the `lua` interpreter skips them, so a script that runs
standalone runs here unchanged.

## Example

```python
colors = [
    # <block check-lua="scripts/validate_colors.lua">
    'red',
    'green',
    'blue',
    # </block>
]
```

**scripts/validate_colors.lua**:

```lua
function validate(ctx, content)
    if content:find("purple") then
        return "purple is not an allowed color"
    end
    return nil
end
```

## The `validate` arguments

- `ctx` — a table with:
    - `ctx.file` — the source file path, relative to the repository root and always separated by
      `/`, on every platform and in every run mode. A script may compare or pattern-match it without normalizing first.
    - `ctx.line` — the line number of the block's start tag.
    - `ctx.attrs` — the block's own attributes, keyed by attribute name, each value exactly as it was written in the
      tag. A block tagged `<block check-lua="…" name="limits" severity="warning">` gives the script
      `ctx.attrs["check-lua"]`, `ctx.attrs["name"]` and `ctx.attrs["severity"]`. Only BlockWatch's own attributes can
      appear here.
    - `ctx.affects` — present only when the block also has an [`affects`](affects.md) attribute. A 1-based array of the
      targets this block affects, each a table with `file`, `name`, and (trimmed)
      `content`. `file` uses the same format as `ctx.file`. References that do not resolve are skipped. A whole-file
      target (an `affects` entry written without a `:`) carries the file's entire text as `content` and no `name`, so
      `affected.name == nil` is how a script tells the two kinds apart.
- `content` — a **string** holding the trimmed text content of the block, or, when
  `check-lua-pattern` is set, a **1-based array** of the values the pattern extracted.

## Narrowing the input with `check-lua-pattern`

`check-lua-pattern` extracts values from the block and passes those to the script instead of the whole content. The
`content` in the Lua script becomes a 1-based array to accommodate all the matches.

```python
prices = [
    # <block check-lua="scripts/check_prices.lua" check-lua-pattern="\$(?P<value>\d+)">
    "Item A: $50",
    "Item B: $150",
    # </block>
]
```

```lua
function validate(ctx, content)
    for _, price in ipairs(content) do   -- content is { "50", "150" }
        if tonumber(price) >= 100 then
            return "price $" .. price .. " is not under $100"
        end
    end
    return nil
end
```

A script that wants a single value reads `content[1]`:

```rust
// <block check-lua="scripts/check_latest_gpt_nano_model.lua" check-lua-pattern='str = "(?P<value>[^"]+)"'>
const DEFAULT_MODEL_NAME: &str = "gpt-5-nano";
// </block>
```

Unlike `same-as-pattern`, the regex runs against the entire block rather than line by line, so a pattern may span
several lines.

## Checking affected blocks

Combining `affects` with `check-lua` lets a script inspect the blocks it affects through
`ctx.affects` — with no file IO, so it works in the default sandboxed mode. This keeps two blocks in sync
deterministically:

```python
allowed_colors = [
    # <block check-lua="scripts/in_sync.lua" affects=":allowed-colors-docs">
    'blue',
    'green',
    'red',
    # </block>
]

docs = [
    # <block name="allowed-colors-docs">
    'blue',
    'green',
    'red',
    # </block>
]
```

**scripts/in_sync.lua**:

```lua
function validate(ctx, content)
    for _, affected in ipairs(ctx.affects) do
        if affected.content ~= content then
            return "block '" .. affected.name .. "' in " .. affected.file .. " is out of sync"
        end
    end
    return nil
end
```

For a plain value comparison, [`same-as`](same-as.md) does this without a script.

A whole-file `affects` target reaches the script the same way, which is how a sandboxed script can read a file it could
not otherwise open:

```lua
function validate(ctx, content)
    for _, affected in ipairs(ctx.affects) do
        if affected.name == nil and not affected.content:find(content, 1, true) then
            return "the value is missing from " .. affected.file
        end
    end
    return nil
end
```

<!-- <block name="lua-safety-modes"> -->

## Safety modes

By default, Lua scripts run **sandboxed** with only the `coroutine`, `table`, `string`, `utf8`, and
`math` standard libraries available. The `io`, `os`, and `package` libraries are **not** loaded, preventing file system
access, command execution, and loading of external modules.

Set `BLOCKWATCH_LUA_MODE` to change the security level:

```shell
# Allow IO and OS libraries (memory-safe, but with file/system access)
BLOCKWATCH_LUA_MODE=safe blockwatch

# Allow all libraries including C module loading (unsafe)
BLOCKWATCH_LUA_MODE=unsafe blockwatch
```

| `BLOCKWATCH_LUA_MODE` | Libraries available                                                   | Security Level                      |
|-----------------------|-----------------------------------------------------------------------|-------------------------------------|
| `sandboxed` (default) | `coroutine`, `table`, `string`, `utf8`, `math`                        | Most secure - No file/OS access     |
| `safe`                | All memory-safe libraries (including `io`, `os`, `package`)           | Memory-safe - Allows file/OS access |
| `unsafe`              | All Lua standard libraries with no restrictions (including C modules) | Unsafe - Full system access         |

<!-- </block> -->

---

← [Validators](README.md) · [README](../../README.md)
