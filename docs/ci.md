# CI & Git Hooks Integration

Checking only the blocks a diff modified keeps validation fast and allows incremental adoption. That is what
`--diff --only-changed` does: `--diff` supplies the diff on stdin, and `--only-changed` narrows the run to the blocks it
touched. See [Run Modes](cli.md#run-modes).

## Pre-commit Framework

To use [pre-commit](https://pre-commit.com), add the hook to `.pre-commit-config.yaml`. Cargo will build `blockwatch`
from source on the first run:

<!-- <block name="pre-commit-rev" same-as="Cargo.toml:crate-version" same-as-mode="subset"
     same-as-pattern='rev: v(?P<value>\d+\.\d+\.\d+)'> -->

```yaml
- repo: https://github.com/mennanov/blockwatch
  rev: v0.5.3  # Use latest release
  hooks:
    - id: blockwatch
```

<!-- </block> -->

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

### Reading Suppressions From the Commit Message

The `blockwatch` hook runs before the commit message exists, so it cannot see a `Blockwatch-suppress:` trailer. The
`blockwatch-commit-msg` hook runs one stage later, at `commit-msg`, and hands the message Git is about to use to
[`--suppress-from`](cli.md#suppressing-from-a-file):

<!-- <block name="commit-msg-rev" same-as="Cargo.toml:crate-version" same-as-mode="subset"
     same-as-pattern='rev: v(?P<value>\d+\.\d+\.\d+)'> -->

```yaml
- repo: https://github.com/mennanov/blockwatch
  rev: v0.5.3  # Use latest release
  hooks:
    - id: blockwatch-commit-msg
```

<!-- </block> -->

Use one hook or the other, not both, or every commit is checked twice.

`pre-commit install` on its own installs only the `pre-commit` stage, so the hook needs the stage installed as well —
either once by hand, or for everyone by declaring it in `.pre-commit-config.yaml`:

```shell
pre-commit install --hook-type commit-msg
```

```yaml
default_install_hook_types: [ pre-commit, commit-msg ]
```

Miss that step and nothing says so: no hook matches the stage, and the commit succeeds unchecked.

The trade-off is when the rejection lands. `blockwatch` refuses a change before a message is written;
`blockwatch-commit-msg` refuses it after. The message is not lost — Git leaves it in `.git/COMMIT_EDITMSG`, so it can be
reused with `git commit -e -F .git/COMMIT_EDITMSG` instead of being retyped.

## Plain Git Hook

Without pre-commit, add the diff pipe directly to `.git/hooks/pre-commit` and make it executable (`chmod +x`):

```bash
#!/bin/sh
git diff --patch --cached --unified=0 | blockwatch --diff --only-changed
```

`set -o pipefail` is deliberately absent here — `/bin/sh` does not portably support it. Under `--diff` an empty stdin is
rejected outright, so a failing diff command still fails the hook rather than passing silently.

To read suppressions from the message instead, write the same pipe to `.git/hooks/commit-msg`, where Git passes the
message file as the first argument:

```bash
#!/bin/sh
git diff --patch --cached --unified=0 | blockwatch --diff --only-changed --suppress-from "$1"
```

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

## Suppressing Violations From a Job

A violation the team has looked at and decided to live with can be kept out of the exit code without touching the
source. Pass its [address](cli.md#the-address-of-a-violation) with `--suppress`, repeating the flag once per address:

```shell
blockwatch \
  --suppress docs/cli.md:cli-docs:keep-sorted \
  --suppress src/lib.rs:languages:line-count
```

Dropping segments from the end widens what an address covers, so `--suppress vendor/generated.py` clears the whole file.

The violations stay in the output, marked `"suppressed": true`.

**BlockWatch holds no state.** It is told which addresses to suppress for the run it is about to perform and remembers
nothing afterwards. Where that list lives is the job's decision — a variable in the workflow, a file the repository
commits and the job expands onto the command line, or a record kept by whatever renders the annotations.

An address that covers nothing does nothing, so a suppression left behind by a rename cannot break the build.

Alternatively, `--suppress-from <FILE>` reads suppression addresses from commit message trailers
(`Blockwatch-suppress: ADDRESS`), which allows suppressions to be scoped to specific commits or pull request ranges
without modifying the workflow:

```shell
git log --format=%B "$BASE..$HEAD" > msgs
git diff --patch "$BASE...$HEAD" | blockwatch --diff --suppress-from msgs
```

**Who writes those messages decides what the range form is worth.** Over a pull request the commit messages come from
whoever opened it, so anyone able to open one can clear a violation their own change introduced by writing the trailer.
Use the range form where commit authors are already trusted to merge. Where they are not, keep the addresses in the
job with `--suppress`, and let the message-driven form run in the local `commit-msg` hook instead, where the author
and the person running the check are the same.

Neither form hides anything, which caps the damage either way: a suppressed violation stays in the diagnostics as
`"suppressed": true`, and in a [SARIF log](cli.md#sarif-output) as `"suppressions": [{"kind": "external"}]`. A reviewer
sees a suppressed finding, not the absence of one.

## GitHub Code Scanning

`--format sarif` writes the violations as a [SARIF log](cli.md#sarif-output), which GitHub's code scanning reads to
show each one as an annotation on the pull request and to keep track of it between runs:

```yaml
name: blockwatch
on:
  pull_request: { branches: [ main ] }
  push: { branches: [ main ] }
permissions: { contents: read, security-events: write }

jobs:
  blockwatch:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: cargo install blockwatch
      - name: Check the blocks
        run: blockwatch --format sarif 2> blockwatch.sarif
      - name: Upload the results
        # The check step exits non-zero on a violation, which is the point; the log still has to be
        # uploaded, or the annotations never appear.
        if: always()
        uses: github/codeql-action/upload-sarif@v3
        with: { sarif_file: blockwatch.sarif }
```

`security-events: write` is what lets a job upload a log; without it the upload step fails with a permissions error.

A clean run writes an empty log rather than no log, so the upload step always has a file, and alerts GitHub is still
holding from an earlier run are closed. A run that stops for a reason other than a violation — an unparseable tag, a
missing API key — writes its error to stderr and so into the same file, which the upload step then rejects as malformed;
the job fails either way, and the reason is in the step's own log.

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

### A validator that cannot run fails the check

Sandboxing controls what a validator may do, not whether it runs. `check-ai` and `check-lua` still run on every block in
scope. When one of them cannot do its job, the result is not "skipped":

| Situation                                                              | Result                                            |
|------------------------------------------------------------------------|---------------------------------------------------|
| `check-ai` and the API key is empty or rejected                        | the run stops with an error                       |
| a `check-lua` script calls `os` or `io` under `sandboxed`              | the run stops with an error                       |
| a `check-lua` script checks for `os`/`io` itself and returns a message | a violation, which looks like a real rule failure |

None of these mean the code is wrong. A fork pull request that changed nothing related still gets a red check. Disable
those validators if that's undesirable:

```yaml
jobs:
  blockwatch:
    runs-on: ubuntu-latest
    env:
      TRUSTED: ${{ github.event_name == 'push' || github.event.pull_request.head.repo.full_name == github.repository }}
    steps:
      - uses: mennanov/blockwatch-action@v1
        env:
          BLOCKWATCH_LUA_MODE: ${{ env.TRUSTED == 'true' && 'safe' || 'sandboxed' }}
          BLOCKWATCH_AI_API_KEY: ${{ env.TRUSTED == 'true' && secrets.BLOCKWATCH_AI_API_KEY || '' }}
        with:
          # Untrusted runs disable I/O capable validators.
          disable: ${{ env.TRUSTED == 'true' && '' || 'check-ai,check-lua' }}
```

`--verbosity summary` prints how many blocks a run did not check.

---

[← Return to README](../README.md)
