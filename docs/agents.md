# Annotating Codebases with AI Agents

Annotating an existing repository with `<block>` tags by hand can be repetitive. AI coding tools can automate this
process by scanning your codebase, adding `<block>` comments in the appropriate language syntax, and running
`blockwatch` to verify their changes.

This repository includes a skill definition ([
`.agents/skills/blockwatch/SKILL.md`](../.agents/skills/blockwatch/SKILL.md)) that guides agents on where blocks add
value, how tag parameters work, and how to test the resulting blocks.

## 1. Install the CLI

Make sure the `blockwatch` binary is installed locally so your agent can verify its work.
See [Installation](../README.md#installation).

## 2. Install the Skill

### Claude Code

Install the plugin once to make the skill available across all projects:

```text
/plugin marketplace add mennanov/blockwatch
/plugin install blockwatch@blockwatch
```

### Other AI Tools

For other environments or project-local setups, copy `SKILL.md` to the expected skill location:

| Agent               | Location                                               |
|---------------------|--------------------------------------------------------|
| **Claude Code**     | Plugin (above) or `.claude/skills/blockwatch/SKILL.md` |
| **Cursor**          | `.cursor/rules/blockwatch.mdc`                         |
| **GitHub Copilot**  | Append to `.github/copilot-instructions.md`            |
| **Codex / generic** | Append to `AGENTS.md`                                  |

To download `SKILL.md` directly:

```shell
mkdir -p .claude/skills/blockwatch
curl -sL https://raw.githubusercontent.com/mennanov/blockwatch/main/.agents/skills/blockwatch/SKILL.md \
  -o .claude/skills/blockwatch/SKILL.md
```

## 3. Run the Agent

Prompt your agent to scan the repository and add rules:

> Using the BlockWatch skill, annotate this repository with `<block>` tags. Focus on lists that must remain sorted or
> unique, and on code that should stay in sync with docs or config. Only add high-value blocks, then run `blockwatch` to
> verify everything passes.

Always inspect the generated diff before committing to ensure the added blocks are necessary and accurate.

## 4. Enable Automated Checks

Set up pre-commit hooks or CI workflows to enforce rules on future changes. See [CI Integration](ci.md).

---

[← Return to README](../README.md)
