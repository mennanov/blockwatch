# Security Policy

## Supported Versions

Only the latest release gets security fixes. A fix ships in a new release. Older releases don't get it.

To update, run the command you installed blockwatch with, such as `brew upgrade blockwatch` or
`cargo install blockwatch`. Read [`CHANGELOG.md`](CHANGELOG.md) first: before 1.0, a new minor version can change flags
and behavior.

## Reporting a Vulnerability

Don't report a vulnerability in a public issue, pull request or discussion.

Report it privately instead:
[open a security advisory](https://github.com/mennanov/blockwatch/security/advisories/new). Only you and the
maintainers of this repository can see it.

Include:

- the version, from `blockwatch --version`;
- the files, flags and environment variables that reproduce the problem;
- what an attacker can do with it.

## What Happens Next

1. You get a reply within 7 days.
2. If the problem is real, it gets fixed in private.
3. A new release ships the fix. Then the advisory is published. It credits you, unless you ask it not to.

Please keep the problem private until the advisory is published. If no fix ships within 90 days of your report, you
may publish it yourself.

## What Counts as a Vulnerability

blockwatch often runs in CI on pull requests from strangers. Such a pull request controls the files, the block tags and
`blockwatch.toml`. It doesn't control the flags or the environment variables. A vulnerability lets such a pull request
do more than the person who runs blockwatch allowed. For example:

- reading a file outside the repository, through a path in a block attribute, a symlink or `blockwatch.toml`;
- a `check-lua` script that reaches files, the network, other programs or the environment while `BLOCKWATCH_LUA_MODE`
  is `sandboxed`;
- sending `BLOCKWATCH_AI_API_KEY`, or the code, to anywhere other than `BLOCKWATCH_AI_API_URL`;
- running code from a file in any other way than a `check-lua` script.

A problem in the release binaries, the install scripts or the crate on crates.io is a vulnerability too.

These are not vulnerabilities. Report them in a [public issue](https://github.com/mennanov/blockwatch/issues):

- What a `check-lua` script can do while `BLOCKWATCH_LUA_MODE` is `safe` or `unsafe`. These modes give the script `io`
  and `os` on purpose.
- `check-ai` sending blocks to `BLOCKWATCH_AI_API_URL`. That is how it works.
- A crash or a slow run on unusual input.
