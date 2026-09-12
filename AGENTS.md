# AGENTS.md

This file provides guidance to AI coding agents working in this repository. It is the single source of truth,
regardless of which agent reads it. `CLAUDE.md` is a symlink to this file, because Claude Code only discovers
`CLAUDE.md`.

## Project

BlockWatch is a language-agnostic linter (Rust CLI, published on crates.io) that enforces rules declared inside
HTML-like `<block ...>` tags found in source-file comments. It can run across the whole tree or only on the changed
lines of a piped unified diff.

## Common commands

```shell
cargo build                                # build CLI
cargo run -- [args]                        # run locally
cargo run -- list '<glob>'                 # print the blocks parsed from matching files, as JSON
cargo test                                 # run all unit + integration tests
cargo test --test keep_sorted              # run one integration test file (tests/<name>.rs)
cargo test <pattern>                       # run tests whose name matches
cargo test -- --nocapture                  # show stdout/stderr from tests
cargo llvm-cov                             # run the tests and report coverage
cargo fmt                                  # format
cargo clippy --all-targets -- -D warnings  # lint
markdownlint-cli2                          # lint Markdown (config: .markdownlint-cli2.yaml)
pre-commit run --all-files                 # run every hook the repository installs
```

Fuzzing (nightly toolchain + `cargo-afl` required) lives in `fuzz/`; see `fuzz/README.md`.

Integration tests use `assert_cmd` to invoke the compiled binary against fixtures in `tests/testdata/`. Many of them
also pipe synthetic diffs into stdin — when adding a test, mirror the existing pattern in `tests/<validator>.rs`.

## Agentic coding rules

Strict adherence to the following rules is *absolutely required* when working with this project.

### Navigating and editing code

- *Always* navigate and edit Rust through a tool that understands the language semantically — an LSP client, an IDE
  integration, or an equivalent MCP server. Use it to find references, jump to definitions and rename symbols.
- *Never* rename or restructure Rust symbols with text substitution (`sed`, `perl`, bulk find-and-replace). Either use a
  semantic rename, or list every reference first and then edit them one at a time.
- *Prefer* the editing tools the harness provides over one-off scripts.
- *Always* dispatch on an enum with an exhaustive `match`. A `let ... else`, or an `if` that peels off one variant and
  funnels the rest into a single branch, silently gives a variant added later whatever the catch-all happened to do. A
  `match` makes the compiler demand a decision at every place that has to make one.

### Escalating to a human

- *Always* escalate on any ambiguity, and on any UX, security or performance concern. State the problem, lay out the
  options, and let the human choose.
- *Never* work around a blocker by going down a rabbit hole. As soon as a solution starts to look complicated, stop and
  escalate instead.

### Writing comments

These rules apply to doc comments, inline comments and prose alike.

- *Always* explain *why* the code is written the way it is. *Never* narrate what it does — the code already says that.
  If the reason is too exotic to explain, escalate to a human.
- *Always* write in plain English, in short sentences. A comment that has to be read twice to be understood is too
  dense: split it up.
- *Never* write a comment that only makes sense to someone doing today's task. Assume a future reader who knows neither
  that task nor this project.
- *Always* keep a doc comment on the symbol it belongs to, and describe that symbol's contract: what it is, what a
  function takes and returns, and what a caller has to know — ordering guarantees, when a value is absent, when it
  panics or errors.
- *Always* put the explanation of an implementation detail inside the function body, next to the code it explains. A
  caller should not have to read about internals to use the symbol.
- *Never* describe how other modules or callers use a symbol. Describe the symbol on its own terms.
- *Always* analyze all the tests in the module and make sure that they are structurally consistent: the most important
  tests come first. All tests should have a consistent naming pattern: "state_action_expected_result", the "action" can
  be omitted if it is obvious from the context.
- *Always* escalate to a human when the tests need to be refactored to keep them well-structured and consistent.

### Writing and running tests

The goal is a suite that fails when behavior changes, not one that touches every line.

- *Always* start from a test that fails, and fails for the reason you expect.
- *Always* re-derive the reasoning for each code path a change touches, and give each path its own test.
- *Always* work out which inputs the change newly changes the answer for, and test the edges of that set. A change that
  is right for the obvious input and wrong just outside it can be overlooked at review.
- *Always* mutation-check a new test before calling the work done. E.g. flip each comparison or boundary the change
  touched, run the suite, confirm a test fails, then restore. A suite that stays green either way is weak.
- *Always* run tests with coverage as a final check before calling the work done.
- *Prefer* reading a coverage report (`cargo llvm-cov`) as a list of questions rather than a number to raise. An
  uncovered branch asks what nobody needed it for, and the answer is either a case worth testing or code worth deleting.
  *Never* add a test whose only purpose is to turn a coverage report line green.

### Committing

- *Never* commit anything automatically. Every change is reviewed by a human first.
- *Always* prefer committing on the current branch (likely `main`) instead of creating feature branches. If a branch is
  strongly recommended, escalate.
- *Always* keep the commit message short: a summary title and a description of one to three sentences.
- *Never* restate the diff in a commit message — which files changed, and what changed in them. The diff already shows
  that.

## The repository lints itself

Source files here carry real `<block ...>` tags, and the `commit-msg` hook runs the working tree's build of the linter
over the staged diff. Two consequences:

- A comment that spells out a block tag *becomes* a block. To write about the syntax in a comment or in Markdown prose,
  keep it inside a code span so it is not mistaken for a rule.
- A failing commit is often the linter reporting a rule the change broke, not the hook itself being broken. Read the
  violation before working around it.

## Architecture

Pipeline, end-to-end, lives in `src/main.rs`:

1. Parse CLI flags (`flags.rs`, clap-derived). Flags are `global`, so they may be written before or after a subcommand.
2. Resolve the repository root and build a `fs::FileSystemImpl` confined to it. Every read goes through it, so a path
   named by a block attribute cannot reach outside the repository.
3. Decide the run's inputs. With `--diff`, a unified diff is read from stdin and parsed by
   `diff_parser::line_changes_from_diff` → `HashMap<RepoPath, Vec<LineChange>>`; `--only-changed` additionally narrows
   the run to the files that diff touched. Without `--diff` stdin is never read and no block counts as changed.
4. Walk the repo (using the `ignore` crate, honoring `.gitignore`) filtered by globs from args and `--ignore`. Language
   is resolved by file extension (with `-E ext=lang` overrides).
5. For each file, the appropriate `language_parsers::<lang>` (tree-sitter grammar) extracts comments; `tag_parser` +
   `block_parser` turn comment text into `Block` values (attributes + byte/position ranges). Result:
   `blocks::FileBlocks`, held in a `validators::ValidationContext`.
6. `validators::detect_validators` dispatches each block through `ValidatorDetector`s (one per validator type). A
   detector returns either `ValidatorType::Sync` or `ValidatorType::Async`. Async validators (e.g. `check-ai`) run on
   Tokio; the runtime is only started if at least one async validator is detected.
7. Validators produce `Violation`s with `ViolationRange` + `BlockSeverity` (error/warning). Addresses passed to
   `--suppress` / `--suppress-from` mark the violations they cover as suppressed: those are still reported, but no
   longer fail the run.
8. Violations go to stderr, as JSON diagnostics or as a SARIF 2.1.0 log depending on `--format`. The `--verbosity` run
   report goes to stdout. The exit code comes from the error-severity violations that remain.

The `list` subcommand stops after step 5 and writes the parsed blocks to stdout as JSON.

Key module boundaries:

- `src/blocks.rs` — `Block`, `FileBlocks`, and the parse that produces them.
- `src/fs.rs` — `FileSystem` / `PathChecker` traits plus the real implementations. These traits are the seam that tests
  use to inject fakes (see `FakeFileSystem`, `FakePathChecker` in `fs::test_utils`).
- `src/repo_path.rs` — `RepoPath`, the one spelling of a repository-relative path. A diff header (`b/src/main.rs`), a
  walk entry and a `file:name` attribute all normalize here, so the same file is always the same map key.
- `src/tag_parser.rs` — winnow-based parser for the `<block ...>` / `</block>` tag syntax.
- `src/block_parser.rs` + `src/language_parsers/` — one tree-sitter grammar per language, each with a `parser()`
  returning a `BlocksParser` that knows which tree-sitter node kinds are comments.
  `language_parsers/mod.rs::language_parsers()` returns the extension→parser map; **adding a new language means adding a
  module here and registering it in that function**.
- `src/validators/` — one file per validator (`affects`, `check_ai`, `check_lua`, `keep_sorted`, `keep_unique`,
  `line_count`, `line_pattern`, `same_as`), each exporting a `*ValidatorDetector`. All detectors are wired up in
  `validators/mod.rs`.
- `src/violation_address.rs` — `ViolationAddress`, the `FILE[:BLOCK_NAME[:VALIDATOR[:HASH]]]` address a suppression
  names. The shorter the address, the more it covers.
- `src/diff_parser.rs` — unidiff wrapper producing `LineChange`s used to decide whether a block's content (or start tag)
  was modified. `is_content_modified` / `intersects_with_any` on `Block` drive the "only check touched blocks" behavior.
- `src/report.rs` — the run report `--verbosity` prints: files scanned, blocks found, validators that checked them.
- `src/sarif.rs` — the SARIF 2.1.0 log `--format sarif` writes.

Only blocks whose content or start-tag range intersects a `LineChange` are validated when a diff is provided; this is
the primary source of subtlety — when debugging "why didn't my rule fire," check whether the diff actually hit the
block's line range.

The `check-ai` validator calls an OpenAI-compatible API configured via `BLOCKWATCH_AI_API_KEY` / `BLOCKWATCH_AI_MODEL` /
`BLOCKWATCH_AI_API_URL`. The `check-lua` validator embeds `mlua` (Lua 5.4); the `BLOCKWATCH_LUA_MODE` env var
(`sandboxed` default / `safe` / `unsafe`) controls which stdlibs are exposed.

## What is shipped versus what is guidance

`.agents/skills/blockwatch/SKILL.md` and `.claude-plugin/` are *product*: the skill this project distributes so agents
can annotate **other** repositories. They are not instructions for working on BlockWatch itself, and changes to them are
user-facing. This file is the guidance for working on BlockWatch.

Per-agent local configuration directories (`.claude/`, `.cursor/`, `.gemini/`, `.windsurf/`, `.aider*`, and others
listed in `.gitignore`) are deliberately untracked. Do not check in agent-specific settings; anything a future
contributor needs belongs in this file.

## Release

Releases are produced by `cargo-dist` (see `dist-workspace.toml`) and the GitHub Actions workflow in
`.github/workflows/`. Version bumps happen in `Cargo.toml` and land via a `chore: Release blockwatch version X.Y.Z`
commit.

User-facing changes get a `CHANGELOG.md` entry under `## [Unreleased]`, written in the same commit that makes the
change; refactors, tests and CI work get none. `cargo release` renames that heading to the version being released (see
the replacements in `release.toml`) and `dist` publishes the renamed section as the GitHub release notes, so an empty
`Unreleased` section at release time means a release with no notes.
