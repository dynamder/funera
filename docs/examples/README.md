# Examples

All examples live in `funera-orchestrate/examples`.

Run any example with:

```bash
cargo run -p funera-orchestrate --example <name> [--features <features>]
```

No-LLM examples:

- `reversible_effects` — LIFO teardown of registered effects
- `tool_policy` — deny-list policy + audit bus (requires `--features security`)

LLM examples require an API key:

- `minimal`, `multi_turn`, `streaming`, `custom_tool`, `middleware`, `skills`,
  `builtin_tools`, `session_reset`, `raw_events`, `multi_runtime`, `sandbox`,
  `security`, `streaming_with_tools`
