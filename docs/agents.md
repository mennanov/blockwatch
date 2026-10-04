# Annotating Codebases with AI Agents

Annotating an existing repository with `<block>` tags by hand can be repetitive. AI coding tools can automate this
process by scanning your codebase, adding `<block>` comments in the appropriate language syntax, and running
`blockwatch` to verify their changes.

This repository includes a skill definition
([`.agents/skills/blockwatch/SKILL.md`](../.agents/skills/blockwatch/SKILL.md)) that guides agents on where blocks add
value, how tag parameters work, and how to test the resulting blocks.

## 1. Install the CLI

Make sure the `blockwatch` binary is installed locally so your agent can verify its work.
See [Installation](../README.md#installation).

## 2. Run the Agent

Paste this prompt into your agent. The agent installs the skill in your project and uses it to add blocks:

<!-- <block same-as="README.md:agent-prompt"> -->

```text
Install the BlockWatch skill from
https://raw.githubusercontent.com/mennanov/blockwatch/main/.agents/skills/blockwatch/SKILL.md
in this project. Then use it to annotate the project and add `blockwatch.toml` if needed. List each block you
added and the mistake it catches.
```

<!-- </block> -->

The skill stays in the project, so agents that work on it later know the rules too. Always inspect the generated diff
before committing to ensure the added blocks are necessary and accurate.

## 3. Enable Automated Checks

Set up pre-commit hooks or CI workflows to enforce rules on future changes. See [CI Integration](ci.md).

## Other Ways to Install the Skill

Install the skill in one of these ways, then leave the first sentence out of the prompt.

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

---

[← Return to README](../README.md)
