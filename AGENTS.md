# AGENTS.md

This file tells AI coding agents how to work in this repository. Every agent reads the same file. `CLAUDE.md` is a
symlink to it, because Claude Code only looks for `CLAUDE.md`.

## Project

blockwatch is a linter written in Rust and published on crates.io. It works with any language. Rules are written as
HTML-like `<block ...>` tags inside code comments, and blockwatch checks them. It can check the whole repository, or
only the lines changed in a unified diff piped into it.

## Common commands

```shell
cargo run -- list '<path or glob>'         # print the blocks parsed from matching files, as JSON
cargo llvm-cov                             # run the tests and report coverage
cargo clippy --all-targets -- -D warnings  # lint
markdownlint-cli2                          # lint Markdown (config: .markdownlint-cli2.yaml)
pre-commit run --all-files                 # run every hook the repository installs
```

Fuzzing lives in `fuzz/`. It needs the nightly toolchain and `cargo-afl`. See `fuzz/README.md`.

Integration tests use `assert_cmd` to run the built binary on the fixtures in `tests/testdata/`. Many of them also pipe
a made-up diff into stdin. When you add a test, follow the pattern in `tests/<validator>.rs`. Build the command with
`common::cargo_bin_cmd!`. It passes an empty config, because this repository's `blockwatch.toml` skips
`tests/testdata/`. Clippy rejects `assert_cmd::cargo_bin_cmd!`, so a test can't read that file by mistake.

## Rules for agents

You *must* follow every rule below, at all times.

### Writing style

These rules apply to everything you write, with no exceptions: code comments, doc comments, `--help` text, error
messages, docs, `CHANGELOG.md`, commit messages, and your replies to the human in chat.

- *Always* write so that each sentence is understood on the first read. If a sentence needs a second read, split it or
  say it more simply.
- *Always* use short sentences, one idea each, and everyday words.
- *Always* say the concrete thing. Name the file, flag or value instead of describing it in the abstract.
- *Always* say what happens, such as what passes or fails, rather than naming a property like "order-insensitive".
- *Never* use jargon or formal framing, such as "hold for", "as opposed to", "govern" or "self-describes". Use the plain
  word instead.
- *Prefer* an example when it is shorter than the explanation.
- *Prefer* a short list or a table to a long sentence with several clauses.

For example:

| Hard to follow                                                                             | Easy to follow                                                                                     |
|--------------------------------------------------------------------------------------------|----------------------------------------------------------------------------------------------------|
| The settings that hold for a whole project, as opposed to the flags that describe one run. | The validated settings for a run: the config file merged with the flags.                           |
| A file whose extension is a key is parsed as if its extension were the value.              | Maps an extension to a supported one. For example, `cxx` to `cpp` makes `.cxx` files parse as C++. |
| Validates the arguments that describe one run, as opposed to the project-wide settings.    | Validates the flags that are not settings, such as `--format` and `--suppress`.                    |

### Reading and editing code

- *Always* read and edit Rust with a tool that understands the language, such as an LSP client, an IDE plugin or an MCP
  server. Use it to find references, jump to definitions and rename symbols.
- *Never* rename or move Rust symbols with text replacement, such as `sed`, `perl` or a bulk find-and-replace. Use the
  tool's rename. If you can't, list every reference first, then edit them one at a time.
- *Prefer* the harness's editing tools to one-off scripts.
- *Always* handle the variants of an enum with an exhaustive `match`. A `let ... else`, or an `if` that handles one
  variant and sends the rest to a single branch, quietly gives any new variant whatever that branch does. With a
  `match`, the compiler makes you decide what to do with each new variant.
- *Always* give an item the narrowest visibility that compiles: private, then `pub(super)`, then `pub(crate)`.
  blockwatch is a program, not a library. Only `run` and `count_rust_blocks` in `lib.rs` are `pub`, for `main.rs` and
  the fuzz target. The `unreachable_pub` lint makes `cargo clippy -- -D warnings` fail on any other `pub`.

### Asking the human

- *Always* ask the human when something is unclear, and when a change affects UX, security or performance. Describe
  the problem, list the options, and let the human choose.
- *Never* dig deeper and deeper to get around a blocker. As soon as a solution starts to look complicated, stop and ask.

### Writing comments

These rules apply to doc comments, inline comments and docs.

- *Always* explain *why* the code is written this way. *Never* describe what it does. The code already shows that. If
  the reason is too unusual to explain, ask the human.
- *Never* write a comment that only makes sense to someone working on today's task. Write for a future reader who
  knows neither the task nor this project.
- *Always* put a doc comment on the symbol it describes. Say what the symbol is, what a function takes and returns, and
  what a caller needs to know: the order of results, when a value can be missing, and when it panics or returns an
  error.
- *Always* explain an implementation detail inside the function body, next to the code it explains. A caller shouldn't
  need to read about the internals to use the function.
- *Never* describe how other modules or callers use a symbol. Describe only the symbol itself.
- *Always* check that the tests in a module follow one structure. Put the most important tests first. Name every test
  `state_action_expected_result`. You can leave out `action` when it is obvious.
- *Always* ask the human before you restructure tests to keep them consistent.

### Writing and running tests

The goal is a test suite that fails when behavior changes. Running every line is not the goal.

- *Always* start with a test that fails, and check that it fails for the reason you expect.
- *Always* work out again why each code path your change touches is correct, and give each path its own test.
- *Always* find the inputs whose result your change changes, and test the edges of that group. A change can be right
  for the obvious input and wrong just next to it, and a reviewer can miss that.
- *Always* mutation-test a new test before you call the work done. For example, flip each comparison or boundary the
  change touched, run the tests, check that one fails, then undo the flip. If the tests pass either way, they are too
  weak.
- *Always* run the tests with coverage as the last check before you call the work done.
- *Prefer* to read a coverage report (`cargo llvm-cov`) as a list of questions, not as a number to raise. An uncovered
  branch asks why nobody needed it. The answer is either a case worth testing or code worth deleting. *Never* add a
  test only to turn a line in the coverage report green.

### Committing

- *Never* commit on your own. A human reviews every change first.
- *Always* commit on the current branch, usually `main`, rather than on a new branch. If you think a new branch is really
  needed, ask the human.
- *Always* keep the commit message short: a title, and one to three sentences of description.
- *Never* repeat the diff in a commit message, such as which files changed and how. The diff already shows that.

## The repository lints itself

Source files here contain real `<block ...>` tags. Two runs check them:

- The `commit-msg` hook builds the linter from the working tree and runs it on the whole repository. It pipes in the
  staged diff with `--diff`, so rules that need a change, such as `affects`, fire for the blocks the commit touches.
- CI runs the release that `mennanov/blockwatch-action` pins on the whole repository. That release can be older than
  the code here.

The hook and CI read `blockwatch.toml`. It skips `tests/testdata/`, where the fixtures break rules on purpose. It also
turns off `check-lua`, because the `check-lua` block here needs the network. `.github/workflows/trusted-checks.yml`
runs that block on a schedule.

No test checks these blocks, so `cargo test` can pass while the hook fails. `tests/general.rs` also runs the linter on
the whole tree, but with an empty config, so the fixtures fail that run on purpose. To check the blocks before you
commit, run `cargo run`.

This means:

- In a source comment, tag text counts even inside a code span. A full tag becomes a block, and a partial one can fail
  the run as a malformed tag. Describe a tag in words instead, such as "the start tag" or "an end tag with a space
  before its slash". Tag text is safe in a Rust string literal. In Markdown, it is safe outside `<!-- -->` and
  `[//]: #` comments.
- When a commit fails, it is usually the linter reporting a rule that the change broke. The hook itself is usually
  fine. Read the violation before you work around it.

## Architecture

The whole pipeline is in `src/cli.rs`. `src/main.rs` only calls `blockwatch::run`:

1. Parse the command-line flags (`flags.rs`, built with clap). The flags that `list` also takes are `global`, so they
   can go before or after `list`. The flags only validation uses are in `ValidationFlags`, so `list --help` doesn't
   show them.
2. Find the repository root and build a `fs::FileSystemImpl` limited to it. Every file read goes through it, so a path
   in a block attribute can't reach outside the repository.
3. Decide what the run works on. With `--diff`, a unified diff is read from stdin, and
   `diff_parser::line_changes_from_diff` turns it into a `HashMap<RepoPath, Vec<LineChange>>`. `--only-changed` also
   limits the run to the files in that diff. Without `--diff`, stdin is never read and no block counts as changed.
4. Resolve the run's settings. `config::read` reads `blockwatch.toml` from the repository root, or the file given by
   `--config`. `Args::raw_settings` returns the settings given as flags. `settings::Settings::resolve` validates both
   with the same rules and merges them: the ignore globs, the extension mappings and the validator selection.
   `config::read` also returns the file's virtual blocks: its `[[block]]` entries, each around a symbol. It marks the
   ones whose entry the diff touches.
5. Walk the repository with the `ignore` crate, which respects `.gitignore`. Keep the files that the paths and globs
   from the command line select, and that don't match the settings' ignore globs. `PathArguments::resolve` turns an
   existing file or directory into a glob first, so `src` selects `src/**`. Pick each file's language by its
   extension, using the settings' extension mappings too.
6. For each file, the matching `language_parsers::<lang>` (a tree-sitter grammar) finds the comments. `tag_parser` and
   `block_parser` turn the comment text into `Block` values, with their attributes and their byte and line ranges.
   `VirtualBlock::resolve` finds the symbol of each virtual block in the file, and adds it as a `Block` too. A target
   that does not resolve stops the run. `--only-changed` also parses the file of a virtual block whose entry the diff
   touches, even when the diff does not touch that file. The result is a `blocks::FileBlocks` for each file, kept in a
   `validators::ValidationContext`. Each path or glob argument must select at least one parsed file, or the run
   stops. Under `--only-changed`, an argument that selects no file in the diff is checked against a walk of the tree.
7. `validators::detect_validators` passes each block to the `ValidatorDetector`s, one for each validator. A detector
   returns either `ValidatorType::Sync` or `ValidatorType::Async`. Async validators, such as `check-ai`, run on Tokio.
   The Tokio runtime only starts if at least one async validator is needed.
8. Validators produce `Violation`s, each with a `ViolationRange` and a `BlockSeverity` (error or warning). A symbol
   reference such as `file#/path` is found with `symbols::resolve`. It searches the symbols that
   `LanguageParser::parse_symbols` finds in the target file, once per file. A missing or ambiguous symbol is a
   violation. A target file that doesn't parse stops the run. Addresses given to `--suppress` or `--suppress-from` mark
   the violations they cover as suppressed. Those are still reported, but they no longer fail the run.
9. Violations go to stderr, as JSON diagnostics or as a SARIF 2.1.0 log, depending on `--format`. The `--verbosity`
   report goes to stdout. The exit code depends on the error-severity violations that are not suppressed.

The `list` subcommand stops after step 6 and writes the blocks it found to stdout as JSON. The `skill` subcommand
prints `src/skill.md` right after step 1.

The main modules:

- `src/fs.rs` — the `FileSystem` and `PathChecker` traits, and their real implementations. Tests use these traits to
  swap in fakes: `FakeFileSystem` and `FakePathChecker` in `fs::test_utils`.
- `src/settings.rs` — `RawSettings` holds the settings read from one place: the config file or the flags. `Settings` is
  the validated and merged result. `RawSettings::validate` is the only place the rules for a setting live, so the
  flags and the config file can't drift apart. Its errors quote the bad value but don't show a line and column.
  Tracking where each value is written would cost more code than it saves the reader.
- `src/config.rs` — reads the config file into a `RawSettings` and the virtual blocks, using `toml_edit`'s serde
  support. Only the errors that `toml_edit` raises itself, such as an unknown key, show a line and column. An error
  about a `[[block]]` entry shows the line of its header, which `serde_spanned` gives.
- `src/virtual_blocks.rs` — `VirtualBlock`, a block that the config file declares around a symbol, instead of tags in a
  comment. A file without comments, such as `package.json`, can only get blocks this way. It is a block of the file it
  wraps. Its content is what a reference to the symbol reads: a scalar's decoded
  value (`ContentText::Decoded`), or else the definition's text. Its `Block::declaration` is the config entry, so every
  message about it shows that line, through `Block::declared_at`. `ValidationContext` keeps every virtual block's
  declaration. So when `affects` or `same-as` reads a file the run did not keep, `ValidationContext::parse_file` adds
  that file's virtual blocks, as the run does.
- `src/repo_path.rs` — `RepoPath`, the one way to write a path relative to the repository root. A diff header
  (`b/src/main.rs`), a path found in the walk and a `file:name` attribute all become a `RepoPath`. So the same file is
  always the same map key.
- `src/language_parsers/` — one tree-sitter grammar for each language. Each language has a `parser()` that returns a
  `LanguageParser`, which knows which syntax nodes are comments. `src/block_parser.rs` turns those comments into
  blocks. `language_parsers()` in `language_parsers/mod.rs` returns the map from extension to parser. **To add a
  language, add a module here and register it in that function.**

  Some languages also have symbols. Such a language has a `SymbolsParser`, which its `parser()` passes to
  `.with_symbols(...)`:

  - JSON uses a `QuerySymbolsParser`, built from a tree-sitter query (`.scm`) and a `NodeDecoder`.
  - TOML finds its symbols with `toml_edit`. What a TOML key means depends on the order of the headers, not on how the
    syntax tree is nested.
  - YAML walks its syntax tree itself. An alias has a position in a sequence but can't be addressed, and a query can't
    express that.

  Each language's tests list every symbol it finds. When you give a language symbols, also document its paths in
  `docs/symbols.md` and in the "Symbols" section of the skill, `src/skill.md`. A test in `language_parsers/mod.rs` and the
  repository's own blocks keep the list of file types with symbols, and each language's section, in sync with the
  code.
- `src/symbols.rs` — `Symbol`, the `SymbolsParser` trait, and `resolve`, which finds the symbol a path refers to.
  `QuerySymbolsParser` runs a language's query and finds every addressable `Symbol` in one walk of the tree, with its
  path, its definition range and its decoded value. It suits a language whose structure is its syntax tree. It rejects
  a file with a syntax error, instead of resolving a path through tree-sitter's error recovery. It doesn't check the
  query itself.
- `src/validators/` — one file for each validator (`affects`, `check_ai`, `check_lua`, `keep_sorted`, `keep_unique`,
  `line_count`, `line_pattern`, `same_as`). Each file exports a `*ValidatorDetector`. `validators/mod.rs` registers
  all of them.
- `src/diff_parser.rs` — wraps `unidiff` to produce `LineChange`s. `range_intersects_any` decides whether they touch a
  range of the file, such as a block's start tag or a symbol's definition. A block's content has its own check in
  `blocks.rs`, because a deletion right where the content starts removes content. The results end up in
  `is_content_modified` and `is_start_tag_modified` on `BlockWithContext`, which decide which blocks count as changed.

A block counts as changed only when its content or its start tag overlaps a `LineChange`. With
`--diff --only-changed`, only changed blocks are validated. With `--diff` alone, every block is validated, but rules
that need a change, such as `affects`, only fire for changed blocks. This is the most common source of surprises. If a
rule didn't fire, check whether the diff actually touched the block's lines. The same goes for an `affects` target that
is a symbol, and for a virtual block. They count as changed only when the diff touches the symbol's definition, not
anything else in its file. A virtual block also counts as changed when the diff touches its `[[block]]` entry, which
plays the part of its start tag.

The `check-ai` validator calls an OpenAI-compatible API. It is configured with `BLOCKWATCH_AI_API_KEY`,
`BLOCKWATCH_AI_MODEL` and `BLOCKWATCH_AI_API_URL`. The `check-lua` validator embeds Lua 5.4 through `mlua`. The
`BLOCKWATCH_LUA_MODE` environment variable decides which standard libraries a script can use: `sandboxed` (the
default), `safe` or `unsafe`.

## What ships to users and what is guidance

`src/skill.md`, `.agents/skills/blockwatch/SKILL.md` and `.claude-plugin/` are part of the *product*. They make up the
skill this project ships, so that agents can add blocks to **other** repositories:

- `src/skill.md` is the skill. `blockwatch skill` prints it with the version filled in, and users save that output in
  their projects.
- `.agents/skills/blockwatch/SKILL.md` is a stub for the plugin in `.claude-plugin/`. It runs `blockwatch skill` each
  time it loads. It has its own copy of the description, and `tests/skill.rs` checks that the two copies match.

They are not instructions for working on blockwatch, and changing them changes what users get. This file is the
guidance for working on blockwatch.

A block tag in `src/skill.md` can have a name and a pattern, but no rule. A project that saves the skill parses its
tags too, and a rule's target exists only in this repository. Put the rule on the block at the other end instead.

Each agent's local config directory is left out of git on purpose: `.claude/`, `.cursor/`, `.gemini/`, `.windsurf/`,
`.aider*`, and the others listed in `.gitignore`. Don't commit agent-specific settings. Anything a future contributor
needs belongs in this file.

## Release

The GitHub Actions workflows in `.github/workflows/` build releases with `cargo-dist`, configured in
`dist-workspace.toml`. The version is bumped in `Cargo.toml`, in a commit titled
`chore: Release blockwatch version X.Y.Z`.

A user-facing change gets a `CHANGELOG.md` entry under `## [Unreleased]`, in the same commit as the change.
Refactors, tests and CI changes get no entry. `cargo release` renames that heading to the new version (see the
replacements in `release.toml`). `dist` then publishes that section as the GitHub release notes. So if `Unreleased` is
empty at release time, the release has no notes.
