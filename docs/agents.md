# Annotating Codebases with AI Agents

Annotating an existing repository with `<block>` tags by hand can be repetitive. AI coding tools can automate this
process by scanning your codebase, adding `<block>` comments in the appropriate language syntax, and running
`blockwatch` to verify their changes.

blockwatch comes with a skill that guides agents on where blocks add value, how tag parameters work, and how to test
the resulting blocks. `blockwatch skill` prints it, so the text always matches the installed version. Its source is
[`src/skill.md`](../src/skill.md).

## 1. Install the CLI

Make sure the `blockwatch` binary is installed locally so your agent can verify its work.
See [Installation](../README.md#installation).

## 2. Run the Agent

Paste this prompt into your agent. The agent installs the skill in your project and uses it to add blocks:

<!-- <block same-as="README.md:agent-prompt"> -->

```text
Run `blockwatch skill` and install the skill in this project as its "Installing this skill" section says. Then use the
skill to annotate the project and add `blockwatch.toml` if needed. List each block you added and the mistake it
catches.
```

<!-- </block> -->

The agent saves a short stub, not the whole skill. The stub runs `blockwatch skill` each time it loads, so it always
matches the installed version. It stays in the project, so agents that work on it later know the rules too. Always
inspect the generated diff before committing to ensure the added blocks are necessary and accurate.

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

The plugin's skill runs `blockwatch skill` each time it loads, so it always matches the installed version.

### Other AI Tools

To set up a project by hand, save the stub from
[Installing this skill](../src/skill.md#installing-this-skill) where your agent reads it:

| Agent               | Location                                               |
|---------------------|--------------------------------------------------------|
| **Claude Code**     | Plugin (above) or `.claude/skills/blockwatch/SKILL.md` |
| **Cursor**          | `.cursor/rules/blockwatch.mdc`                         |
| **GitHub Copilot**  | `.github/copilot-instructions.md`                      |
| **Codex / generic** | `AGENTS.md`                                            |

`.github/copilot-instructions.md` and `AGENTS.md` hold other instructions too. Add only the stub's description there,
and the sentence "Then run `blockwatch skill` and follow what it prints."

Don't save the output of `blockwatch skill` itself. A saved copy goes stale when blockwatch is upgraded.

---

[← Return to README](../README.md)
