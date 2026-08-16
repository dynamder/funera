# Examples

All examples live in `funera-orchestrate/examples`.

Run any example with:

```bash
cargo run -p funera-orchestrate --example <name> [--features <features>]
```

No-LLM examples:

- `plugin_architecture`
- `loader_declarative`
- `service_broker`
- `replace_loop`
- `replace_react_loop`
- `replace_core_tool`
- `tool_policy`

LLM examples require an API key:

- `minimal`, `multi_turn`, `streaming`, `custom_tool`, `middleware`, `skills`,
  `builtin_tools`, `session_reset`, `raw_events`, `multi_runtime`, `sandbox`,
  `security`, `streaming_with_tools`
