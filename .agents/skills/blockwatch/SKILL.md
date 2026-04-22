---
name: blockwatch
description: Instructions for using BlockWatch to maintain code and documentation sync, format lists, and validate block rules.
---

# BlockWatch Skill

This repository uses **BlockWatch** to strictly align code, documentation, and configuration formats using simple tags inside comments.

## What is BlockWatch?

BlockWatch links parts of your codebase and enforces rules directly via inline comments. Block boundaries are typically marked by `<block ...>` and `</block>`.

Supported languages include Rust, Python, Markdown, Go, HTML, YAML, etc.

## Guidelines For AI

When generating or modifying code in this repository, you **MUST** follow these rules:

1. **Preserve Block Tags:** Never delete existing `<block ...>` and `</block>` tags unless explicitly told so. Place any new additions within the appropriate block boundaries.
2. **Follow Block Directives:** When modifying content inside a block, respect its attributes:
   - `keep-sorted` / `keep-sorted="asc"`: You MUST keep the list items alphabetically sorted.
   - `keep-sorted-format="numeric"`: Sort items numerically.
   - `keep-unique`: You MUST NOT introduce duplicate lines in the block.
   - `line-pattern="<regex>"`: Ensure all your new lines match the provided regex.
   - `line-count="<operator><number>"`: Ensure the line count satisfies the condition (e.g. `line-count="<=5"`).
   - `affects="<file>:<block-name>"`: If you change this block, you **MUST** also modify the corresponding `<block name="<block-name>">` in `<file>`.
   - `check-ai`: Validates using an LLM. Ensure your changes respect the natural language rule provided.
3. **Verify Changes:** Whenever you alter files that might contain BlockWatch blocks, or if you modify lists/configs, you **MUST** run the `blockwatch` command to verify that no rules were broken.

## Running BlockWatch

You have the ability to run the `blockwatch` command directly in the Bash shell.

To run BlockWatch on all files:

```bash
blockwatch
```

To run BlockWatch specifically on your currently unstaged git changes (very fast and efficient):

```bash
git diff --patch | blockwatch
```

If it fails, read the error message, fix the sorting, duplication, or synchronization issue, and re-run the command until it passes.

To list all recognized blocks and audit them without running validation:

```bash
blockwatch list
```
