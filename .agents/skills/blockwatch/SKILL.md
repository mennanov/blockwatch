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
