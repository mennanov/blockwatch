# Changelog

All notable user-facing changes to BlockWatch are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Releases up to and including v0.3.11 predate this file. Their notes live on the
[GitHub releases page](https://github.com/mennanov/blockwatch/releases).

<!-- next-header -->

## [Unreleased] - ReleaseDate

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
