# `check-ai`

Validates a block against a rule stated in plain English, using an LLM. For rules that regex cannot
express — "must mention the company name", "no TODOs left", "the tone matches the rest of the page".

## Syntax

| Attribute          | Value                                                    | Default |
|--------------------|----------------------------------------------------------|---------|
| `check-ai`         | the condition, in natural language                        | —       |
| `check-ai-pattern` | regex; the `(?P<value>…)` group, or the whole match       | whole block |

## Example

```html
<!-- <block check-ai="Must mention the company name 'Acme Corp'"> -->
<p>Welcome to Acme Corp!</p>
<!-- </block> -->
```

## Targeted checks

`check-ai-pattern` sends only the matching parts of the block to the model, which keeps the prompt
focused and the token cost down:

```python
prices = [
    # <block check-ai="Prices must be under $100" check-ai-pattern="\$(?P<value>\d+)">
    "Item A: $50",
    "Item B: $150",  # Violation
    # </block>
]
```

## Configuration

[//]: # (<block name="check-ai-env-vars" same-as-pattern="BLOCKWATCH_AI_[A-Z_]+">)

- `BLOCKWATCH_AI_API_KEY`: API Key.
- `BLOCKWATCH_AI_MODEL`: Model name (default: `gpt-5-nano`).
- `BLOCKWATCH_AI_API_URL`: Custom OpenAI compatible API URL (optional).

[//]: # (</block>)

Any OpenAI-compatible endpoint works.

## Notes

- **This is the expensive validator.** It makes a network call per block and needs an API key. Reach
  for a deterministic validator first — see [choosing a validator](README.md#which-validator-do-i-want).
- **Scope the run, or pay for the whole repository.** The cost of a run is set by how many `check-ai` blocks are in
  scope, and a bare `blockwatch` — or `blockwatch --diff` — puts every one of them in scope. For per-pull-request CI and
  hooks use `--diff --only-changed`, which checks only the blocks the diff touched. See
  [Run Modes](../cli.md#run-modes).
- Blocks are checked concurrently, so a run with many `check-ai` blocks costs roughly one round trip
  rather than N.
- Disable it for local runs with `blockwatch -d check-ai` when you do not want to spend tokens. See
  the [CLI reference](../cli.md).
- An empty condition is a hard error.

---

← [Validators](README.md) · [README](../../README.md)
