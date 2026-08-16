# Architecture

```text
┌──────────────────────────────────────────────────────────────┐
│  funera-orchestrate    Agent, AgentRuntime, AgentLoop        │
├──────────────────────────────────────────────────────────────┤
│  funera_core           FuneraEnv, PluginRegistry, Loader     │
│                        ReActLoop, Middleware, Security       │
├──────────────────────────────────────────────────────────────┤
│  funera_builtin_tools  Read, Write, Edit, Shell              │
└──────────────────────────────────────────────────────────────┘
```

## Runtime communication

- `AgentRuntime` is a thin channel wrapper.
- `EnvActor` owns the `Loader`/`FuneraEnv` and processes `EnvCmd`.
- `SessionActor` owns conversation history.
- `ReActLoop` (or a custom `AgentLoop`) runs one turn.

## Plugin lifecycle

`Pending -> Loading -> Active -> Unloading -> Inactive/Failed`

The registry is notification-driven: service changes mark dependent instances dirty, and a
refresh only re-evaluates affected instances.
