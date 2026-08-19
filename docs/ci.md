# CI & Git Hooks Integration

Checking only the blocks a diff modified keeps validation fast and allows incremental adoption. That is what
`--diff --only-changed` does: `--diff` supplies the diff on stdin, and `--only-changed` narrows the run to the blocks it
touched. See [Run Modes](cli.md#run-modes).

## Pre-commit Framework

To use [pre-commit](https://pre-commit.com), add the hook to `.pre-commit-config.yaml`. Cargo will build `blockwatch`
from source on the first run:

```yaml
- repo: https://github.com/mennanov/blockwatch
  rev: v0.2.27  # Use latest release
  hooks:
    - id: blockwatch
```

If `blockwatch` is already installed locally (e.g. via Homebrew or Cargo), use a local hook to skip building from
source:

```yaml
- repo: local
  hooks:
    - id: blockwatch
      name: blockwatch
      entry: bash -c 'set -o pipefail; git diff --patch --cached --unified=0 | blockwatch --diff --only-changed'
      language: system
      stages: [ pre-commit ]
      pass_filenames: false
```

The `--unified=0` flag minimizes diff context lines so unchanged adjacent blocks aren't included in the check.

`set -o pipefail` makes the pipeline report the diff command's failure instead of only `blockwatch`'s exit code. A diff
command that fails without writing anything is already caught — `blockwatch` rejects empty stdin under `--diff` — but
one that dies partway through writing leaves a shorter, still well-formed diff, which would otherwise pass as a clean
run over a change set that was never fully read.

## Plain Git Hook

Without pre-commit, add the diff pipe directly to `.git/hooks/pre-commit` and make it executable (`chmod +x`):

```bash
#!/bin/sh
git diff --patch --cached --unified=0 | blockwatch --diff --only-changed
```

`set -o pipefail` is deliberately absent here — `/bin/sh` does not portably support it. Under `--diff` an empty stdin
is rejected outright, so a failing diff command still fails the hook rather than passing silently.

## GitHub Actions

Use the official GitHub Action to validate pull requests and pushes:

```yaml
name: blockwatch
on:
  pull_request: { branches: [ main ] }
  push: { branches: [ main ] }
permissions: { contents: read }

jobs:
  blockwatch:
    runs-on: ubuntu-latest
    steps:
      - uses: mennanov/blockwatch-action@v1
        # Optional: set API key for check-ai
        # env: { BLOCKWATCH_AI_API_KEY: ${{ secrets.BLOCKWATCH_AI_API_KEY }} }
```

## Diff Input

Under `--diff` the piped diff must carry Git's path prefixes and be repository-relative. A normal `git diff`
satisfies both, so no extra flags are needed for a standard checkout. Empty, ANSI-colorized, or non-diff input is
rejected rather than read as "nothing changed", so produce the diff with `--color=never` where color is forced on.

Diffs produced with `--no-prefix`, `diff.noprefix`, a custom `diff.srcPrefix` / `diff.dstPrefix`, or
`diff.relative` are rejected — BlockWatch stops rather than risk validating the wrong file. If your repositories set any
of these globally, pin the output:

```shell
git diff --patch --unified=0 --default-prefix --no-relative | blockwatch --diff --only-changed
```

See [Supported Diff Input](cli.md#supported-diff-input) for details.

## Full-Tree Runs

`blockwatch` on its own scans every block in the repository — no diff, no flags. That is the run to schedule
periodically on the main branch: [`same-as`](validators/same-as.md), `keep-sorted` and the other deterministic
validators catch copies that drifted apart in files no recent diff happened to touch.

One rule is missing from it. [`affects`](validators/affects.md) asks whether two blocks were edited *together*, which
only a diff can answer, so a bare run does not check it at all. To audit the whole tree and still enforce `affects`,
pass `--diff` without `--only-changed`:

```shell
git diff --patch <base>..<head> | blockwatch --diff
```

`--verbosity summary` reports how many blocks carry a rule that needs a diff, so a run says plainly what it could not
check.

## Security: Sandboxing Fork Pull Requests

If your repository uses [`check-lua`](validators/check-lua.md) or [`check-ai`](validators/check-ai.md), pull requests
from forks execute Lua scripts defined in files. To prevent unauthorized execution or credential leakage on public
repositories, sandbox untrusted PRs:

```yaml
jobs:
  blockwatch:
    runs-on: ubuntu-latest
    env:
      # Trusted if run on push or PR from internal branch
      TRUSTED: ${{ github.event_name == 'push' || github.event.pull_request.head.repo.full_name == github.repository }}
    steps:
      - uses: mennanov/blockwatch-action@v1
        env:
          BLOCKWATCH_LUA_MODE: ${{ env.TRUSTED == 'true' && 'safe' || 'sandboxed' }}
          BLOCKWATCH_AI_API_KEY: ${{ env.TRUSTED == 'true' && secrets.BLOCKWATCH_AI_API_KEY || '' }}
```

When `BLOCKWATCH_LUA_MODE` is set to `sandboxed`, Lua scripts run without OS or filesystem access and without API
secrets.

---

[← Return to README](../README.md)
