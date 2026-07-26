# CI integration

Validating only the diff keeps these near-instant.

## Pre-commit

Using the [pre-commit](https://pre-commit.com) framework — it builds blockwatch from source with cargo on first run:

```yaml
- repo: https://github.com/mennanov/blockwatch
  rev: v0.2.27  # use the latest release tag
  hooks:
    - id: blockwatch
```

If the binary is already installed (via Homebrew, say), the local form avoids the build:

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

`--unified=0` gives tighter diffs, so fewer blocks are pulled in by surrounding context lines.

## Plain git hook

No framework needed. Write this to `.git/hooks/pre-commit` and `chmod +x` it:

```bash
#!/bin/sh
git diff --patch --cached --unified=0 | blockwatch
```

## GitHub Actions

```yaml
- uses: mennanov/blockwatch-action@v1
```

The action figures out the diff for you. A minimal workflow:

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
        # Only needed if you use check-ai:
        # env: { BLOCKWATCH_AI_API_KEY: ${{ secrets.BLOCKWATCH_AI_API_KEY }} }
```

## Full-tree runs

Validating the PR diff is enough for `affects` and other drift checks. A periodic full-tree run —
`blockwatch` with nothing on stdin — is a useful extra net, since it catches
[`same-as`](validators/same-as.md) disagreements on blocks that no recent diff happened to touch.

## Fork PRs and untrusted input

If your repo uses [`check-lua`](validators/check-lua.md) or
[`check-ai`](validators/check-ai.md), a pull request from a fork is untrusted input: the Lua scripts that get executed
come from the scanned files, so a fork PR could otherwise run arbitrary commands with your secrets in the environment.

This repository's own workflow handles it by keeping fork PRs sandboxed and secret-free:

```yaml
jobs:
  blockwatch:
    runs-on: ubuntu-latest
    env:
      # Trusted = a push (which requires write access), or a PR from a branch in this repo.
      TRUSTED: ${{ github.event_name == 'push' || github.event.pull_request.head.repo.full_name == github.repository }}
    steps:
      - uses: mennanov/blockwatch-action@v1
        env:
          BLOCKWATCH_LUA_MODE: ${{ env.TRUSTED == 'true' && 'safe' || 'sandboxed' }}
          BLOCKWATCH_AI_API_KEY: ${{ env.TRUSTED == 'true' && secrets.BLOCKWATCH_AI_API_KEY || '' }}
```

Fork PRs are still linted — just in the sandboxed mode, with no file or OS access and no secrets.

---

← [README](../README.md)
