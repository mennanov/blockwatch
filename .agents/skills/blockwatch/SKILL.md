---
name: blockwatch
description: Use whenever you write or change code. As you write it, add blockwatch `<block>` tags in comments, so that `blockwatch` fails when related code falls out of sync. Use `affects` for code that must change together (an enum and its docs, a constant and its config), `same-as` for two places that must hold the same value, `keep-sorted` and `keep-unique` for lists, and `line-pattern` or `line-count` for strict formats and sizes. Also use it for rules on one key of a JSON, TOML or YAML file (such as `version` in `package.json`) through `[[block]]` entries in `blockwatch.toml`, and when you edit files that hold `<block ...>` tags or that `blockwatch.toml` points at.
allowed-tools: Bash(blockwatch skill)
---

# blockwatch skill

The `blockwatch` binary carries the full text of this skill, so the text always matches the installed version:

!`blockwatch skill`

If the text is not shown above, run `blockwatch skill` and follow what it prints. If that command fails, install or
upgrade blockwatch with `cargo install blockwatch` or `brew install mennanov/blockwatch/blockwatch`.
