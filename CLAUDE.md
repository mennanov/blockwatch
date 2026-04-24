# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

BlockWatch is a language-agnostic linter (Rust CLI, published on crates.io) that enforces rules declared inside HTML-like `<block ...>` tags found in source-file comments. It can run across the whole tree or only on the changed lines of a piped `git diff --patch`.

## Common commands

```shell
cargo build                                # build CLI
cargo run -- [args]                        # run locally
cargo test                                 # run all unit + integration tests
cargo test --test keep_sorted              # run one integration test file (tests/<name>.rs)
cargo test <pattern>                       # run tests whose name matches
cargo test -- --nocapture                  # show stdout/stderr from tests
cargo fmt                                  # format
cargo clippy --all-targets -- -D warnings  # lint
```

Fuzzing (nightly toolchain + `cargo-afl` required) lives in `fuzz/`; see `fuzz/README.md`.

Integration tests use `assert_cmd` to invoke the compiled binary against fixtures in `tests/testdata/`. Many of them also pipe synthetic diffs into stdin — when adding a test, mirror the existing pattern in `tests/<validator>.rs`.

## Architecture

Pipeline, end-to-end, lives in `src/main.rs`:

1. Parse CLI flags (`flags.rs`, clap-derived).
2. If stdin is not a TTY, parse the unified diff via `diff_parser::line_changes_from_diff` → `HashMap<PathBuf, Vec<LineChange>>`. No diff → every line is treated as modified.
3. Walk the repo (using the `ignore` crate, honoring `.gitignore`) filtered by globs from args and `--ignore`. Language is resolved by file extension (with `-E ext=lang` overrides).
4. For each file, the appropriate `language_parsers::<lang>` (tree-sitter grammar) extracts comments; `tag_parser` + `block_parser` turn comment text into `Block` values (attributes + byte/position ranges). Result: `blocks::FileBlocks`.
5. `validators::mod` dispatches each block through `ValidatorDetector`s (one per validator type). A detector returns either `ValidatorType::Sync` or `ValidatorType::Async`. Async validators (e.g. `check-ai`) run on Tokio; the runtime is only started if at least one async validator is detected.
6. Validators produce `Violation`s with `ViolationRange` + `BlockSeverity` (error/warning). `main` prints them and sets the exit code.

Key module boundaries:

- `src/blocks.rs` — `Block`, `FileBlocks`, repo walking, `FileSystem` / `PathChecker` traits. These traits are the seam that tests use to inject fakes (see `FakeFileSystem`, `FakePathChecker` in `src/lib.rs` `test_utils`).
- `src/tag_parser.rs` — winnow-based parser for the `<block ...>` / `</block>` tag syntax.
- `src/block_parser.rs` + `src/language_parsers/` — one tree-sitter grammar per language, each with a `parser()` returning a `BlocksParser` that knows which tree-sitter node kinds are comments. `language_parsers/mod.rs::language_parsers()` returns the extension→parser map; **adding a new language means adding a module here and registering it in that function**.
- `src/validators/` — one file per validator (`affects`, `check_ai`, `check_lua`, `keep_sorted`, `keep_unique`, `line_count`, `line_pattern`), each exporting a `*ValidatorDetector`. All detectors are wired up in `validators/mod.rs`.
- `src/diff_parser.rs` — unidiff wrapper producing `LineChange`s used to decide whether a block's content (or start tag) was modified. `is_content_modified` / `intersects_with_any` on `Block` drive the "only check touched blocks" behavior.

Only blocks whose content or start-tag range intersects a `LineChange` are validated when a diff is provided; this is the primary source of subtlety — when debugging "why didn't my rule fire," check whether the diff actually hit the block's line range.

The `check-ai` validator calls an OpenAI-compatible API configured via `BLOCKWATCH_AI_API_KEY` / `BLOCKWATCH_AI_MODEL` / `BLOCKWATCH_AI_API_URL`. The `check-lua` validator embeds `mlua` (Lua 5.4); the `BLOCKWATCH_LUA_MODE` env var (`sandboxed` default / `safe` / `unsafe`) controls which stdlibs are exposed.

## Release

Releases are produced by `cargo-dist` (see `dist-workspace.toml`) and the GitHub Actions workflow in `.github/workflows/`. Version bumps happen in `Cargo.toml` and land via a `chore: Release blockwatch version X.Y.Z` commit.
