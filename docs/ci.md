# CI & Git Hooks Integration

Checking only modified blocks in git diffs keeps validation fast and allows incremental adoption.

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
      entry: bash -c 'git diff --patch --cached --unified=0 | blockwatch'
      language: system
      stages: [ pre-commit ]
      pass_filenames: false
```

The `--unified=0` flag minimizes diff context lines so unchanged adjacent blocks aren't included in the check.

## Plain Git Hook

Without pre-commit, add the diff pipe directly to `.git/hooks/pre-commit` and make it executable (`chmod +x`):

```bash
#!/bin/sh
git diff --patch --cached --unified=0 | blockwatch
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

The piped diff must carry Git's path prefixes and be repository-relative. A normal `git diff`
satisfies both, so no extra flags are needed for a standard checkout.

Diffs produced with `--no-prefix`, `diff.noprefix`, a custom `diff.srcPrefix` / `diff.dstPrefix`, or
`diff.relative` are rejected with the flag that fixes them — BlockWatch stops rather than risk validating the wrong
file. If your repositories set any of these globally, pin the output:

```shell
git diff --patch --unified=0 --default-prefix --no-relative | blockwatch
```

See [Supported Diff Input](cli.md#supported-diff-input) for details.

## Full-Tree Runs

While diff-based checks catch `affects` violations in changed files, [`same-as`](validators/same-as.md) checks benefit
from periodic full-tree runs. Running `blockwatch` without piped diff input scans all blocks across the entire
repository to ensure untouched copies haven't drifted.

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
