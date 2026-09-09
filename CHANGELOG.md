# Changelog

All notable user-facing changes to BlockWatch are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Releases up to and including v0.3.11 predate this file. Their notes live on the
[GitHub releases page](https://github.com/mennanov/blockwatch/releases).

<!-- next-header -->

## [Unreleased] - ReleaseDate

### Added

- `--suppress-from FILE`, repeatable, reads suppression addresses from `Blockwatch-suppress: ADDRESS` lines in a text
  file. The prefix is matched case-insensitively and every other line is ignored, so an ordinary commit message is
  valid input and a suppression can travel with the commit that needs it instead of living in the CI configuration.
  The path may point anywhere the run can read, so a `commit-msg` hook can pass the message file Git hands it even
  from a linked worktree, where that file sits outside the tree being checked.
- A second [pre-commit](https://pre-commit.com) hook id, `blockwatch-commit-msg`, runs the same check at the
  `commit-msg` stage and feeds the message being written to `--suppress-from`. The original `blockwatch` hook is
  unchanged, so an existing configuration keeps working; see [CI Integration](docs/ci.md) for which one to pick and for
  the extra install step a `commit-msg` hook needs.

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
