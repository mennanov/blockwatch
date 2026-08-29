# `check-ai`

Validates a block against a rule stated in plain English, using an LLM. For rules that regex cannot express — "must
mention the company name", "no TODOs left", "the tone matches the rest of the page".

## Syntax

| Attribute          | Value                                                     | Default     |
|--------------------|-----------------------------------------------------------|-------------|
| `check-ai`         | the condition, in natural language                        | —           |
| `check-ai-pattern` | regex; every match's `(?P<value>…)` group, or whole match | whole block |

## Example

```html
<!-- <block check-ai="Must mention the company name 'Acme Corp'"> -->
<p>Welcome to Acme Corp!</p>
<!-- </block> -->
```

## Targeted checks

`check-ai-pattern` sends only the matching parts of the block to the model, which keeps the prompt focused and the token
cost down:

```python
prices = [
    # <block check-ai="Prices must be under $100" check-ai-pattern="\$(?P<value>\d+)">
    "Item A: $50",
    "Item B: $150",  # Violation
    # </block>
]
```

Multiple matches are concatenated with `\n` (newline).

A pattern that matches nothing is reported as a violation.

## Configuration

[//]: # (<block name="check-ai-env-vars" same-as-pattern="BLOCKWATCH_AI_[A-Z_]+">)

- `BLOCKWATCH_AI_API_KEY`: API Key.
- `BLOCKWATCH_AI_MODEL`: Model name (default: `gpt-5-nano`).
- `BLOCKWATCH_AI_API_URL`: Custom OpenAI compatible API base URL (optional, default:
  `https://api.openai.com/v1`).

[//]: # (</block>)

Any OpenAI-compatible endpoint works. The URL is a **base**, not a full endpoint — `/chat/completions`
is appended to it — so point it at the part of the path the provider shares across routes (`https://host/v1`, not
`https://host/v1/chat/completions`).

## Before you send code to a model

`check-ai` sends the content of every block in scope to a third-party service and asks a model whether it satisfies your
condition. Four things follow from that:

- **The block content leaves your machine.** Whatever is in the block — credentials, customer data, unreleased work —
  goes to the provider, under the provider's retention and training policy. Keep `check-ai` off blocks that can hold
  secrets, and scope runs with `--diff --only-changed`.
- **The block content is part of the prompt.** Text in a block can contradict your condition and talk the model into
  answering `OK`. Anyone who can edit a checked file can try this — a fork pull request, for example.
- **The answer is a guess, not a proof.** The same block and condition can pass one run and fail the next. Use
  `check-ai` for prose and style rules. For anything you have to rely on, use a deterministic validator.
- **CI runs are not repeatable.** Outages, rate limits, a model that changed or was retired, and ordinary sampling
  variance all change the result when nothing in the code changed. Start with `severity="warning"` until a condition
  proves stable.

## Notes

- **This is the expensive validator.** It makes a network call per block and needs an API key. Reach for a deterministic
  validator first — see [choosing a validator](README.md#which-validator-do-i-want).
- **Scope the run, or pay for the whole repository.** The cost of a run is set by how many `check-ai` blocks are in
  scope, and a bare `blockwatch` — or `blockwatch --diff` — puts every one of them in scope. For per-pull-request CI and
  hooks use `--diff --only-changed`, which checks only the blocks the diff touched. See
  [Run Modes](../cli.md#run-modes).
- Blocks are checked concurrently, so a run with many `check-ai` blocks costs roughly one round trip rather than N.
- Disable it for local runs with `blockwatch -d check-ai` when you do not want to spend tokens. See
  the [CLI reference](../cli.md).
- An empty condition is a hard error. So is a missing or rejected API key: the run stops instead of reporting the block
  as a violation. A run without a working key has to disable the validator — see
  [A validator that cannot run fails the check](../ci.md#a-validator-that-cannot-run-fails-the-check).

---

← [Validators](README.md) · [README](../../README.md)
