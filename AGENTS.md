# AGENTS.md

blockwatch is a linter written in Rust. It works with any language. Rules are `<block ...>` tags inside code comments.
`CLAUDE.md` is a symlink to this file, because Claude Code only reads `CLAUDE.md`.

Every rule below says why it exists. Use the reason to decide the cases the rule doesn't cover. When a rule gets in
your way, ask the human. Don't work around it.

## Commands

```shell
pre-commit run --all-files         # fmt, clippy, docs, tests, markdownlint, and this repository's own blocks
cargo llvm-cov                     # tests with coverage
cargo run                          # check this repository's own blocks
cargo run -- list '<path or glob>' # print the blocks parsed from matching files, as JSON
```

## Ask the human

- Ask when the task is unclear, and before a change that affects UX, security or performance. Describe the problem,
  list the options, and let the human choose. These are the human's decisions.
- Stop and ask as soon as a fix starts to look complicated. A complicated workaround usually means the plan is wrong.
- Never commit on your own. A human reviews every change first. When asked to commit, commit on the current branch.
  Write a title and one to three sentences. Don't list what the diff changed. The diff already shows that.

## Design

- Write the simplest code that does the job. Don't add a helper, an accessor, a type alias or a test whose only payoff
  is slightly nicer output or a rare edge case. Reword the message to fit the simple code instead. For example, join a
  list with `", "` rather than write a helper that prints "a, b or c". Every extra line is code someone must read and
  keep working.
- Make failures loud. When a value can't be computed, or an input can't be handled as asked, return an error that says
  the cause and, where possible, the fix. Never fall back to a default, and never skip a bad input with only a
  warning. Users don't notice warnings, so a silent fallback gives a wrong result that looks right. Output may be empty
  or zero when that is the true result.
- Handle an enum with an exhaustive `match`, never with `let ... else`, `if let` or `matches!`. Then the compiler
  makes you decide what each new variant does. Clippy's `wildcard_enum_match_arm` fails a `_` arm, but it doesn't
  see these three. `Option` and `Result` can't get new variants, so they are fine.
- A `match` on another crate's enum may end with `_` when no new variant could need another arm. Say why in an
  `#[expect(clippy::wildcard_enum_match_arm, reason = "...")]` on the function or statement that holds the `match`.
  Clippy ignores one on the arm.
- Never add a dependency loop, between the modules in `src/` or between the files of one module. If `a` uses `b`, then
  `b` must not use `a`, directly or through other files. Tests count too: a test may use only what its file's code may
  use. A `mod.rs` doesn't use an item it only re-exports. To break a loop, move the item that both sides need into its
  own file.
- Give each item the narrowest visibility that compiles: private, then `pub(super)`, then `pub(crate)`. blockwatch is
  a program, not a library.
- Rename or move Rust symbols with a tool that understands Rust, such as rust-analyzer. Never use `sed`, `perl` or a
  bulk find-and-replace. Text replacement misses references and changes unrelated text.

## Tests

The goal is a test suite that fails when behavior changes. Running every line is not the goal.

- Write a failing test first. Check that it fails for the reason you expect.
- Find the inputs whose result your change changes, and test the edges of that group. A change can be right for the
  obvious input and wrong just next to it.
- Mutation-test each new test: flip each comparison or boundary the change touched, and check that a test fails. Do it
  in a scratch copy with its own `target/`, and delete both right after. A `target/` takes gigabytes.
- Read the coverage report as questions. An uncovered branch is either a case worth testing or code worth deleting.
  Never add a test only to cover a line.
- Name tests `state_action_expected_result`. Put the most important tests first.
- Integration tests run the binary on `tests/testdata/`. Follow `tests/<validator>.rs`, and build the command with
  `common::cargo_bin_cmd!`. It passes an empty config, because `blockwatch.toml` skips `tests/testdata/`.

## Comments

- Explain why the code is written this way. Never describe what it does. The code shows that.
- Write for a reader who doesn't know today's task. Never refer to a plan, a stage or an issue.
- A doc comment says what the symbol is, what it takes and returns, the order of results, when a value can be missing,
  and when it errors or panics. It doesn't say how callers use it. Implementation details go in the body, next to the
  code.

## Writing style

This applies to everything you write: comments, `--help`, error messages, docs, `CHANGELOG.md`, commit messages and
chat.

- Use short sentences, one idea each, and everyday words. Each sentence must make sense on the first read.
- Say the concrete thing: the file, flag or value. Say what passes or fails, not a property like "order-insensitive".
- No jargon or formal framing, such as "hold for", "as opposed to" or "govern".
- Use an example or a table when it is shorter than the explanation.

| Hard to follow                                                                          | Easy to follow                                                                                   |
|-----------------------------------------------------------------------------------------|--------------------------------------------------------------------------------------------------|
| A file whose extension is a key is parsed as if its extension were the value.           | Maps an extension to a supported one. For example, `cu` to `cpp` makes `.cu` files parse as C++. |
| Validates the arguments that describe one run, as opposed to the project-wide settings. | Validates the flags that are not settings, such as `--format` and `--suppress`.                  |

## Traps in this repository

- **The repository lints itself.** Tag text in a source comment becomes a block, or fails the run as a malformed tag,
  even inside backticks. Describe a tag in words instead, such as "the start tag". Tag text is safe in a Rust string
  literal, and in Markdown outside `<!-- -->` and `[//]: #` comments. `cargo test` doesn't check these blocks, so run
  `cargo run`. When the commit hook fails, read the violation. It is usually a rule your change broke.
- **A block counts as changed only when the diff touches its content or its start tag.** If a rule such as `affects`
  didn't fire, check that first.
- **`src/skill.md`, `.agents/skills/blockwatch/SKILL.md` and `.claude-plugin/` ship to users.** They are not guidance
  for working here. `src/skill.md` must not hold block tags, because each project that saves the skill would parse
  them. Point a block at its headings instead, such as `src/skill.md#/blockwatch/Validator reference`.
- **A user-facing change gets a `CHANGELOG.md` entry** under `## [Unreleased]`, in the same commit. It becomes the
  release notes. Refactors, tests and CI changes get no entry.
- **`.github/workflows/release.yml` is generated.** Change `dist-workspace.toml`, then run `dist generate`. After
  changing any workflow, run `gh actions-lock`, or GitHub refuses to start it.
