# Changelog

All notable user-facing changes to blockwatch are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Releases up to and including v0.3.11 predate this file. Their notes live on the
[GitHub releases page](https://github.com/mennanov/blockwatch/releases).

<!-- next-header -->

## [Unreleased] - ReleaseDate

## [0.10.0] - 2026-10-07

### Added

- `--only-block FILE:BLOCK_NAME` checks only the listed blocks, and `--skip-block FILE:BLOCK_NAME` checks every block
  but those. Repeat a flag to list more blocks. A violation of a named block prints an address that starts with
  `FILE:BLOCK_NAME`, so you can copy it from there. The `only-blocks` and `skip-blocks` keys of `blockwatch.toml` set
  the same lists for a project, and a flag replaces them. `affects` and `same-as` can still refer to a skipped block,
  and `blockwatch list` still shows it. An address that matches no block fails the run, so a renamed block can't
  quietly turn its checks on or off. Before, a run could pick files and validators, but not single blocks
  ([#150](https://github.com/mennanov/blockwatch/issues/150)).

## [0.9.0] - 2026-10-07

### Added

- `blockwatch skill` prints the skill for AI agents as a complete `SKILL.md`, for the installed version. Save it with
  `blockwatch skill > .claude/skills/blockwatch/SKILL.md`, or into your agent's skills directory. The saved skill
  records its version, and after an upgrade it tells the agent to save it again. Before, the README had agents
  download the skill from GitHub, and the copy went stale. The Claude Code plugin now runs `blockwatch skill` each time
  it loads the skill.

### Changed

- The agent skill now loads whenever an agent writes or changes code, and the agent adds blocks as it writes, even in a
  project that has none yet. Before, the skill applied only to a project that already used blockwatch. The agent still
  annotates a whole project only when you ask.

- An argument that selects no file to check fails the run. Before, it checked nothing and passed. This happened with
  a typo such as `'scr/**/*.py'`, with `notes.txt`, whose extension is not supported, and with a file that
  `.gitignore` or `--ignore` leaves out. With `--only-changed`, an argument only has to select a file in the
  repository, not a changed one.
- An unknown `BLOCKWATCH_LUA_MODE` value, such as `Safe`, stops a run that checks a `check-lua` block. The error
  lists the valid values. Before, it quietly meant `sandboxed`, so a script lost `os` and `io` with an error that didn't
  mention the variable. An empty value still means `sandboxed`.
- `--diff` accepts an empty diff, which is what `git diff` prints when nothing changed. `--diff` then checks every
  block, and `--diff --only-changed` checks none. Before, the run failed with `diff in stdin is empty.` So
  `git diff --patch | blockwatch --diff`, as the README shows it, failed on a clean tree and after `git add`.

### Fixed

- A project that saved the agent skill in its tree failed every run with
  `failed to canonicalize path "docs/validators/README.md"`. The skill had block tags that point at files in the
  blockwatch repository. Upgrade, then save the skill again with `blockwatch skill`.
- The output of `blockwatch list` now ends with a newline. Before, it ended at the closing `}`, so the shell prompt
  started on the same line, and `while read` skipped the last line.
- `--help` recommended `--only-changed` for hooks and CI. It now recommends `--diff` alone, like the docs and the
  shipped hooks. Only a full scan catches a change that breaks a block in a file it never touched.
- The `--help` line for `-e`/`--enable` said "Enable a validator". It now says the flag runs only the validators it
  lists. So `-e check-ai` no longer reads like it adds `check-ai` to the ones that already run.
- A path argument selects the file or directory it points at. `./src/x.py`, `src`, `src/`, `.` and an absolute path
  used to match no file. A file whose name is also a glob, such as `app/[id].tsx`, is now checked itself, instead of
  `app/i.tsx`. Paths and globs still start from the repository root, from any directory.
- Some documented commands piped in the staged diff, but blockwatch reads the files on disk. With unstaged changes,
  the run checked other content than the diff showed. So a broken commit could pass, and a correct one could fail. The
  pre-commit framework hooks were not affected.
  - The plain Git hooks in [Plain Git Hook](docs/ci.md#plain-git-hook) and in the agent skill now stop, and ask you to
    stage or stash the unstaged changes.
  - The staged-diff examples in `--help`, the README, `docs/cli.md` and the agent skill now pipe
    `git diff --patch HEAD`. It covers staged and unstaged changes.
- `--verbosity summary` printed `1 blocks`, `1 checks`, `1 violations` and `2 needs --diff`. It now prints `1 block`,
  `1 check`, `1 violation` and `2 need --diff`. A script that looks for these words in the line must accept both
  forms ([#143](https://github.com/mennanov/blockwatch/issues/143)).
- When the program that reads the output stops early, such as `head`, blockwatch now stops writing without an error.
  Before, it printed `Broken pipe (os error 32)` and exited with 1. Now the exit code depends only on the violations.
  This covers `list`, `--verbosity` and the violations on stderr
  ([#142](https://github.com/mennanov/blockwatch/issues/142)).
- One file that is not valid UTF-8, such as a Latin-1 file, stopped the whole run with
  `stream did not contain valid UTF-8`. This happened even when the file had no blocks, and even with `--ignore` when
  a diff from `--diff` touched the file. Now such a file is skipped if it has no blocks. If it has blocks, the run
  fails with an error that shows the file and how to skip it
  ([#139](https://github.com/mennanov/blockwatch/issues/139)).

## [0.8.1] - 2026-10-05

### Changed

- `check-ai` uses `gpt-5.4-nano` by default, instead of `gpt-5-nano`. It costs more per token. To keep the old model,
  set `BLOCKWATCH_AI_MODEL=gpt-5-nano`.
- `blockwatch list --help` shows only the flags `list` takes: `--diff`, `--only-changed`, `--config`, `-E` and
  `--ignore`. `list` now rejects `--enable` and `--disable`, which it used to ignore.

## [0.8.0] - 2026-10-05

### Added

- A config file. `blockwatch.toml` at the repository root holds the settings that stay the same for a project:
  `ignore`, `extensions`, `enable` and `disable`, one for each flag. The file's ignore globs and extension mappings
  add to the flags', and a flag wins for the same extension. `--enable` or `--disable` on the command line replaces
  the file's selection. An unknown key or a bad value fails the run, and the error quotes it. `--config FILE` reads
  another file instead. See [Config File](docs/cli.md#config-file).
- Blocks in the config file. A `[[block]]` entry in `blockwatch.toml` declares a block around a symbol, such as
  `target = 'package.json#/version'`, with the attributes a tag would have. So `package.json`, which can't hold a
  comment, can have rules, and a key in a TOML or YAML file can have them without tags. A symbol can have one such
  block. Its content is what a reference to the symbol
  reads: a scalar's value, or the text of an object, a list or a table. Its violations are reported in that file. A
  named one is found by references such as `affects="package.json:react-version"`, as a tag is. It counts as changed
  when the diff touches the symbol or the entry, so `--diff --only-changed` checks a block whose entry you edited.
  `blockwatch list` shows the config line that declares it, as `config_line`. A target that does not resolve fails
  the run, and the error shows the entry's line. See
  [Blocks in the Config File](docs/cli.md#blocks-in-the-config-file).

### Changed

- **Breaking:** the pre-commit hooks `blockwatch` and `blockwatch-commit-msg` check every block in the repository, not
  only the blocks the staged diff touched. `affects` still checks the staged diff. A violation anywhere in the
  repository now fails the commit, so run `blockwatch` once before you bump `rev`. To check only the changed blocks, see
  [Checking Only Changed Blocks](docs/ci.md#checking-only-changed-blocks).
- A run over the whole repository is much faster. A file is parsed only when it contains `<block` or `</block`, and
  most files contain neither. On the kubernetes repository, a run takes about 1 s instead of 15 s. As a result, in a
  file without a start tag, an end tag with a space in it, such as `</ block>`, is no longer reported.

### Fixed

- The pre-commit hooks no longer fail when nothing is staged, as under `pre-commit run --all-files` or
  `git commit --amend`. They run `blockwatch` without `--diff` then.
- A comment such as `// only <block, filesystem> is tested` no longer fails the run with `Malformed block tag`.
  `<block` and `</block` start a tag only when whitespace follows them or the comment ends there.

## [0.7.0] - 2026-09-28

### Added

- Support for JSON (`.json`, `.jsonc`) files.
- Symbol references in `same-as` and `affects`. `same-as="package.json#/dependencies/inngest"` compares the block
  with the value at that path in a JSON, TOML or YAML file, and `same-as="#/version"` with one in the block's own
  file. A value that holds other values, such as an object, a table or a list, is compared as its source text.
  `affects="package.json#/version"` is satisfied only when the diff touches that key, not when anything else in the
  file changes, and a `check-lua` script sees that target in `ctx.affects`, named `#/version`, with its value as its
  content. A missing or ambiguous symbol is reported as a violation. A target file with a syntax error fails the run,
  and a JSON trailing comma counts as one.
  - In a TOML file, a path follows TOML's own keys, however they are written: `[tool.ruff]` with `line-length = 88`,
    and `tool.ruff.line-length = 88`, are both `#/tool/ruff/line-length`. The entries of an array of tables count from
    0, as in `#/bin/0/name`.
  - A TOML table covers every place it is written. `affects="Cargo.toml#/package"` is satisfied by a change under
    `[package.metadata.docs]` too, and `same-as` compares the text of every place, in order.
  - Invalid TOML fails the run, and a duplicate key counts as invalid.
  - In a YAML file, block and flow style give the same paths, and a string compares as the text YAML reads, `|` and
    `>` blocks included. An alias (`*name`) is not a symbol, so a path to it or through it is reported as not found.
    A file holding several documents fails the run, since a path cannot say which one it means.

### Changed

- **Breaking:** `#` is now reserved in target references to introduce symbol references (`file#/path` or `#/path`).
  Consequently, block names containing `#` (`file:block#name`) are rejected as invalid, and files with `#` in their
  path can no longer be addressed. Combining `#` and `:` in the same reference is prohibited.

## [0.6.0] - 2026-09-15

### Changed

- **Breaking:** the JSON diagnostic contract now defines `range.end.character` as exclusive (`[start, end)`,
  advancing it by one past the last highlighted character) to align with SARIF and LSP conventions.

### Fixed

- SARIF violation ranges are now half-open `[start, end)` (1-based, exclusive end character),
  fixing single-character highlights appearing as zero-width cursors and off-by-one under-highlighting in SARIF
  viewers.
- Unexpected closed block parser errors now report the 1-based character column (`column {}`)
  instead of a 0-based byte offset (`position {}`), matching malformed tag errors.

## [0.5.5] - 2026-09-12

### Fixed

- Deleting the lines immediately above a block no longer counts as a change to that block. `affects` reported the
  blocks it points at as out of date when nothing needed updating, and — the worse half — treated a target that had
  only lost lines above it as updated, silently dropping a violation that should have been reported. With
  `--only-changed`, a block left untouched this way is no longer pulled into the run at all, so its other rules stop
  re-running too. Deleting every line of a block's content still counts as a change to it.
- Rewriting a block's start tag across two or more extra lines no longer counts as a change to the block's content.
  `affects` reported the blocks such a block points at as out of date when nothing needed updating, and — the worse
  half — treated a target whose own tag had been rewritten this way as updated, silently dropping a violation that
  should have been reported. Reformatting a tag now leaves `is_content_modified` false, whichever language the file
  is in.

## [0.5.4] - 2026-09-10

### Added

- `--suppress-from FILE`, repeatable, reads suppression addresses from `blockwatch-suppress: ADDRESS` lines in a text
  file. The prefix is matched case-insensitively and every other line is ignored, so an ordinary commit message is
  valid input and a suppression can travel with the commit that needs it instead of living in the CI configuration.
  The path may point anywhere the run can read, so a `commit-msg` hook can pass the message file Git hands it even
  from a linked worktree, where that file sits outside the tree being checked.
- A second [pre-commit](https://pre-commit.com) hook id, `blockwatch-commit-msg`, runs the same check at the
  `commit-msg` stage and feeds the message being written to `--suppress-from`. The original `blockwatch` hook is
  unchanged, so an existing configuration keeps working; see [CI Integration](docs/ci.md) for which one to pick and for
  the extra install step a `commit-msg` hook needs.

### Fixed

- The `rev:` of the [pre-commit](https://pre-commit.com) snippets in the README and in
  [CI Integration](docs/ci.md) named releases that were stale or, in one case, had never been published. Both
  now name the current release, and the release process keeps them and the SARIF sample output in step from
  here on.

## [0.5.3] - 2026-09-05

### Added

- `--format sarif` writes the violations as a [SARIF 2.1.0](https://docs.oasis-open.org/sarif/sarif/v2.1.0/sarif-v2.1.0.html)
  log on stderr, in place of the JSON diagnostics, for GitHub code scanning and anything else that reads the format. A
  violation's address travels with it as a `partialFingerprints` entry, so a consumer can match it against the same
  violation in a later run, and a suppressed violation carries SARIF's own `"suppressions": [{"kind": "external"}]`.
  Unlike the JSON diagnostics, a SARIF log is written even by a run that found nothing.

## [0.5.2] - 2026-09-04

### Added

- `--suppress FILE[:BLOCK_NAME[:VALIDATOR[:HASH]]]`, repeatable, stops reported violations failing the run without
  editing the source it points at. Blocks with no `name` have no address of their own, so their violations can only be
  suppressed by a file-wide address.

### Changed

- Diagnostics carry two new fields: `address` (absent for an unnamed block) and `suppressed` (absent unless true).

## [0.5.1] - 2026-08-28

### Added

- `affects`, `same-as` and `check-lua`'s `ctx.affects` accept a whole file as a target, written without a `:`
  (`affects="config/schema.json"`). Nothing is parsed out of the file, so formats that cannot declare a block — JSON,
  `.env`, lockfiles, plain-text fixtures — can now be linked to. `affects` counts the target as modified when the diff
  touches the file at all; `same-as` compares against the file's entire content, read under the referencing block's
  `same-as-pattern`; a `ctx.affects` entry for a whole file carries the file's text and no `name`.

### Fixed

- A `<block>` tag written inside a string is no longer picked up as a real rule in GraphQL, nor in the JSON form of a
  Dockerfile instruction (`CMD ["# ..."]`).

### Changed

- A reference whose block name is empty (`affects="config.json:"`) is now rejected as an authoring error instead of
  being reported as a dangling reference to a block with no name.

## [0.5.0] - 2026-08-26

### Changed

- A `check-ai-pattern` that matches nothing in its block is now reported as a violation and no request is made. The
  model used to be sent an empty string, answer that it met the condition, and leave the block counted as checked
  without any of its content having been examined — so a pattern broken by a typo or by content that drifted passed
  quietly. `check-lua-pattern` is unchanged: its script still receives an empty array and decides for itself.

- **Breaking:** a `check-lua` script whose block sets `check-lua-pattern` now receives `content` as a 1-based array of
  the extracted values instead of a string. A script that expects a single value reads `content[1]`. The argument is an
  array whenever the attribute is present — a single match gives a one-element array and a pattern that matches nothing
  gives an empty array — so a script never has to branch on the type of its argument. Blocks without
  `check-lua-pattern` are unaffected: `content` is still the block's trimmed text.

### Fixed

- Use every match a `*-pattern` finds, in `check-ai`, `check-lua` and `same-as`. Each of them kept only the first match
  and silently ignored the rest. Now `check-ai` receives the values joined by newlines, `check-lua` receives them as an
  array, and `same-as` compares them as separate items. Matches whose value is empty are skipped everywhere. One
  consequence for `same-as`: because every line's matches now flow into one list, two blocks holding the same values
  across the lines now agree where they used to differ earlier. Fixes
  ([#125](https://github.com/mennanov/blockwatch/issues/125)).

- Scan dot-prefixed files and directories, such as `.github/`. The repository walk dropped them before the file patterns
  were applied, so blocks there were never validated and an explicit `blockwatch ".github/**"` reported nothing. The
  directories a version control system keeps its state in (`.git`, `.hg`, `.jj`, `.svn`) are still skipped, and
  `.gitignore` and `--ignore` still apply. Fixes ([#100](https://github.com/mennanov/blockwatch/issues/100)).

## [0.4.4] - 2026-08-25

### Added

- Accept `_` digit separators in `keep-sorted-format="numeric"` and `same-as-format="numeric"`, so long literals can keep
  the spelling their language gives them (`1_000_000`). A separator must sit between two digits.

### Fixed

- Apply the `keep-unique` and `keep-sorted-pattern` regexes to the trimmed line. A line whose match is empty is now
  skipped like any other unmatched line. Fixes ([#120](https://github.com/mennanov/blockwatch/issues/120)).
- Compare numbers exactly in `keep-sorted-format="numeric"` and `same-as-format="numeric"`. Values that exceed the
  precision or the range of a 64-bit float — long identifiers, for instance — no longer compare equal to each other.
  `inf` and `NaN` are no longer accepted as numbers. Fixes
  ([#103](https://github.com/mennanov/blockwatch/issues/103)).

## [0.4.3] - 2026-08-24

### Added

- Add a `check-lua-timeout` block attribute which limits how long a Lua script may run (default: 30 seconds). Fixes
  ([#107](https://github.com/mennanov/blockwatch/issues/107)).

### Fixed

- `-E` extension mappings now apply to the files `same-as` and `affects` read to resolve a reference, not only to the
  files the run scans. A referenced file the mapping made parseable was reported as an unsupported format by `same-as`,
  and treated as unchanged by `affects`. Fixes ([#105](https://github.com/mennanov/blockwatch/issues/105)).
- `same-as` now reports a violation when its `same-as-pattern` matches no lines on both the source and target sides,
  instead of treating the two empty results as trivially equal and passing. Fixes
  ([#102](https://github.com/mennanov/blockwatch/issues/102)).
- Duplicate blocks with the same name within the same file are rejected. Fixes
  ([#104](https://github.com/mennanov/blockwatch/issues/104)).
- A `<block>`/`</block>` tag that fails to parse (for example a missing closing `>`) now fails the whole run with
  `Malformed block tag at line N, column N`, instead of being silently skipped. Fixes
  ([#108](https://github.com/mennanov/blockwatch/issues/108)).
- `affects` now checks that each referenced block still exists, matching `same-as`. A reference to a renamed or deleted
  target block is reported as a violation (rather than passing silently, or reporting the misleading "is modified, but X
  is not"), and this check runs even without a diff; a reference to a missing target *file* fails the run. Fixes
  ([#109](https://github.com/mennanov/blockwatch/issues/109)).

## [0.4.2] - 2026-08-21

### Fixed

- A proper handling of non-ASCII chars. Fixes ([#127](https://github.com/mennanov/blockwatch/issues/127)).
- `line-pattern`, `keep-sorted` and `keep-unique` violations now point at the failing text in the source file. Fixes
  ([#123](https://github.com/mennanov/blockwatch/issues/123)).
- `check-lua` accepts scripts that start with a `#!` line or a UTF-8 byte order mark. Fixes
  ([#121](https://github.com/mennanov/blockwatch/issues/121)).
- Block markers written inside a Markdown table cell are now found. Fixes
  ([#116](https://github.com/mennanov/blockwatch/issues/116)).
- Marker text inside a string is no longer read as a block. An HTML attribute value and a quoted Dockerfile argument
  each invented a block the source never declared, which could also report a violation against a file nobody had
  tagged.

## [0.4.1] - 2026-08-20

### Breaking changes in v0.3.11

- **`check-lua` scripts can no longer read custom block attributes.** This changed in **0.3.11**, as a consequence of
  rejecting misspelled attribute names ([#97](https://github.com/mennanov/blockwatch/issues/97)).

### Fixed

- Added support for git worktrees and submodules ([#114](https://github.com/mennanov/blockwatch/issues/114)).
- A change to a block's content is no longer missed when the same edit also touched the block's start tag or the lines
  above it ([#106](https://github.com/mennanov/blockwatch/issues/106)).

## [0.4.0] - 2026-08-20

### Changed

- **Breaking:** the run mode is now chosen by flags rather than inferred from stdin. `--diff` supplies the diff and is
  the only thing that reads stdin; `--only-changed` narrows the run to the blocks the diff touched. A bare
  `git diff | blockwatch` now scans the whole repository instead of the diff — pass both flags for the previous
  behavior: `git diff | blockwatch --diff --only-changed`.
- **Breaking:** the `--verbosity summary` line and the JSON report now name the run mode in a `mode` field. Under
  `mode=all` they also report `blocks_needing_diff`: the number of blocks carrying a rule, such as `affects`, that
  cannot fire without a diff.
- `--diff` now rejects stdin that cannot be a diff — empty input, colorized diff, or text that is not a patch — instead
  of silently checking nothing. A valid diff that produces no line changes is still accepted.
- Positional globs now narrow the files a diff selects, so they restrict a run in every mode.

### Fixed

- `affects` resolves its reference targets even when they are excluded by the globs. An out-of-scope file is read only
  to answer the reference: it is never validated and never appears in a run report.
- A diff that does not contain a single valid path (likely a mistake) is now an error under
  `--diff`, which silently succeeded previously. A single unresolvable path among valid ones remains normal.
