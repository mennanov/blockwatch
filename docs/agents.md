# Annotating your project with an AI agent

Adding the first `<block>` tags by hand is the tedious part of picking up BlockWatch. Coding agents are good at it: an
agent can read through the repo, pick reasonable spots, write the tags in the right comment syntax for each language,
and run `blockwatch` to check its own work.

This repo ships a skill for exactly that —
[`.agents/skills/blockwatch/SKILL.md`](../.agents/skills/blockwatch/SKILL.md). It tells the agent where blocks are worth
adding, documents the tag syntax, and explains how to verify the result.

## 1. Install the binary

The agent needs to be able to run it. See [Install](../README.md#install).

## 2. Give the skill to your agent

**Claude Code** — install the plugin once and the skill is available in every project, with no per-project setup:

```text
/plugin marketplace add mennanov/blockwatch
/plugin install blockwatch@blockwatch
```

For other agents, or if you prefer a project-local copy, place `SKILL.md` where your tool looks for instructions:

| Agent              | Where to put the skill                                                                                                  |
|--------------------|-------------------------------------------------------------------------------------------------------------------------|
| **Claude Code**    | Use the plugin above (recommended), or `.claude/skills/blockwatch/SKILL.md` (project) / `~/.claude/skills/...` (global) |
| **Cursor**         | `.cursor/rules/blockwatch.mdc`                                                                                          |
| **GitHub Copilot** | append to `.github/copilot-instructions.md`                                                                             |
| **Codex / others** | append to `AGENTS.md`                                                                                                   |

Pull the file straight from this repo:

```shell
mkdir -p .claude/skills/blockwatch
curl -sL https://raw.githubusercontent.com/mennanov/blockwatch/main/.agents/skills/blockwatch/SKILL.md \
  -o .claude/skills/blockwatch/SKILL.md
```

## 3. Ask the agent to annotate the project

For example:

> Using the BlockWatch skill, annotate this repository with `<block>` tags. Focus on lists that
> should stay sorted/unique and on code that must stay in sync with docs or config. Add only
> high-value blocks, then run `blockwatch` to confirm they all pass.

Review the diff before committing. The agent's choices are a starting point, and a block that does not catch a real
mistake is just noise — the skill tells the agent to be selective, but the judgment call is yours.

## 4. Turn on enforcement

Wire up the pre-commit hook and GitHub Action so the rules stay in place. See
[CI integration](ci.md).

---

← [README](../README.md)
