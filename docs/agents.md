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

Paste this prompt into your agent. The agent reads the skill and adds blocks to your project:

<!-- <block same-as="README.md:agent-prompt"> -->

```text
Run `blockwatch skill` and follow it to add `<block>` tags, plus `blockwatch.toml` if needed.
Verify your blocks with `blockwatch`, then list each block you added and the mistake it catches.
```

<!-- </block> -->

The agent does not install the skill or edit your hooks or CI. Inspect the generated diff before committing to make
sure every added block is necessary and accurate.

## 3. Enable Automated Checks

Set up pre-commit hooks or CI workflows to enforce rules on future changes. See [CI Integration](ci.md).

## Installing the Skill for Future Runs

To make agents follow blockwatch rules in future sessions without pasting the prompt again, install the skill once:

### Claude Code

Install the plugin once to make the skill available across all projects:

<!-- <block same-as="README.md:plugin-install"> -->

```text
/plugin marketplace add mennanov/blockwatch
/plugin install blockwatch@blockwatch
```

<!-- </block> -->

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
