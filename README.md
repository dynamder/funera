# Funera

> **Everything is a plugin.** A Rust LLM agent framework where tools, skills, middleware,
> providers, callbacks, and even the agent loop itself are dynamically composable plugins.

WARNING: This crate is still under development, the documentation may be incomplete or wrong. And the API may change.
WARNING: The security features are still under development and testing, and cannot be trusted to be secure.

[![CI](https://github.com/dynamder/funera/actions/workflows/ci.yml/badge.svg)](https://github.com/dynamder/funera/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/funera.svg)](https://crates.io/crates/funera)
[![docs.rs](https://docs.rs/funera/badge.svg)](https://docs.rs/funera)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://opensource.org/licenses/MIT)
[![MSRV](https://img.shields.io/badge/MSRV-1.88-orange.svg)](https://github.com/dynamder/funera/blob/main/CONTRIBUTING.md#minimum-supported-rust-version-msrv)
[![Rust](https://img.shields.io/badge/edition-2024-orange)](https://rust-lang.org)

## Why Funera?

- **Everything is a plugin** — `Tool`, `Skill`, `Middleware`, `Provider`, `Callback`, and even
  the `AgentLoop` are all `Plugin` subtraits with one unified lifecycle.
- **Dynamic composition** — load, unload, hot-replace, and reconcile plugins at runtime without
  restarting the process.
- **Typestate lifecycle** — plugin state transitions are compile-time checked.
- **Service multiplexing** — multiple providers can back one service via `ServiceBroker`.
- **Security-oriented** — tool policies, path guards, audit, sandbox, and reversible effects.

## Quick Start

```rust
use funera::{Agent, AgentRuntime, DeepSeekProvider};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = AgentRuntime::<DeepSeekProvider>::builder()
        .api_key(std::env::var("DEEPSEEK_API_KEY")?)
        .model("deepseek-v4-flash")
        .build()?;

    let agent = Agent::builder()
        .system_prompt("You are a helpful assistant.")
        .build();

    let resp = agent.fire("Hello!", &runtime).await?;
    println!("{}", resp.content);
    Ok(())
}
```

## Architecture

### Crate Layers

```
┌──────────────────────────────────────────────────────────────┐
│  funera-orchestrate    high-level builder API                │
│  Agent · AgentRuntime · callbacks · streaming                │
├──────────────────────────────────────────────────────────────┤
│  funera_core           core engine                           │
│  FuneraEnv · EnvActor · ReActLoop · SessionActor · EventBus │
│  Middleware · Security · Provider · Tools · Skills           │
├──────────────────────────────────────────────────────────────┤
│  funera_builtin_tools  default tool implementations          │
│  ReadTool · WriteTool · EditTool · ShellTool                 │
└──────────────────────────────────────────────────────────────┘
```

### Runtime Communication Flow

```mermaid
graph TD
    subgraph "AgentRuntime (thin wrapper)"
        env_cmd_tx[env_cmd_tx: mpsc Sender]
        session_tx[session_tx: mpsc Sender]
        mw[ middleware_chain: Arc ]
    end

    subgraph "EnvActor (owns all env state)"
        env_state[FuneraEnv<br/>model · client · watch tx]
        watcher[FuneraEnvWatcher<br/>watch rx]
        registry[ToolRegistry: Arc]
        skill_reg[SkillRegistry: Arc]
        executor[ToolExecutor<br/>tokio task]
        audit[AuditBus]
        state_tx[env_state_tx: broadcast Sender]
    end

    subgraph "SessionActor"
        msgs[Vec&lt;FuneraMessage&gt;]
    end

    subgraph "ReActLoop (per fire/send)"
        loop_core[ReActLoop::run]
    end

    env_cmd_tx -->|EnvCmd| env_state
    env_cmd_tx -->|GetReActConfig · SubscribeEnvState · model · tool_names · approve_tool_call| watcher
    session_tx -->|SessionCmd| msgs

    env_state -->|watch::Sender push| watcher
    watcher -->|watch::Receiver read| loop_core

    state_tx -->|EnvStateEvent broadcast| subscribers[subscribe_env_state subscribers]

    executor -.->|owns| registry
    registry -.->|Arc shared| env_state
    audit -.->|used by| executor
```

### ReAct Loop Data Flow

```mermaid
sequenceDiagram
    participant User
    participant Agent
    participant Runtime as AgentRuntime
    participant EnvActor as EnvActor
    participant ReAct as ReActLoop
    participant LLM
    participant Tool as ToolExecutor

    User->>Agent: fire("hello", &runtime)
    Agent->>Runtime: get_react_config()
    Runtime->>EnvActor: EnvCmd::GetReActConfig
    EnvActor-->>Runtime: ReActConfig { watcher, tool_bus, max_iters, buf }
    Runtime-->>Agent: ReActConfig

    Agent->>ReAct: run with config

    loop each iteration
        ReAct->>ReAct: watcher.watch_model/client/tools/skills
        Note over ReAct: hot-reload via watch channels

        ReAct->>LLM: create_stream(client, request)
        LLM-->>ReAct: token stream

        alt tool call needed
            ReAct->>Tool: tool_bus.execute(tool_call)
            Tool-->>ReAct: tool result
        else text response
            ReAct-->>Agent: AgentEvent::Text
        end
    end

    ReAct-->>Agent: AgentEvent::Done
    Agent-->>User: ChatResponse
```

### Env Hot-Reload Flow

```mermaid
sequenceDiagram
    participant User
    participant Runtime
    participant EnvActor
    participant Watcher as FuneraEnvWatcher
    participant ReAct as ReActLoop

    User->>Runtime: set_model("gpt-5")
    Runtime->>EnvActor: EnvCmd::SetModel("gpt-5")
    EnvActor->>EnvActor: env.set_model() → push model_tx watch channel
    EnvActor->>EnvActor: broadcast EnvStateEvent::LlmChanged

    ReAct->>Watcher: watch_model() at next iteration
    Watcher-->>ReAct: "gpt-5"
    Note over ReAct: picks up change seamlessly
```

## Features

- **ReAct loop** — iterative tool-calling agent execution with configurable max iterations and runtime hot-reloading
- **Actor-based architecture** — all mutable state lives in background tasks (EnvActor, SessionActor, ToolExecutor); `AgentRuntime` is a thin channel-wrapper
- **Pluggable providers** — OpenAI and DeepSeek backends with streaming support
- **Tool system** — define custom tools by implementing the `Tool` trait; built-in file I/O and shell
- **Skill system** — load prompt templates from YAML-frontmatter Markdown files
- **Middleware pipeline** — intercept agent events with inspectors (read-only, parallel) and mutators (pass/modify/block, sequential)
- **Security layer** — tool/shell policies, path allowlisting, audit logging, secure API key storage
- **Type-state session** — compile-time enforcement of session ownership (`Idle` / `Acquired`)

## Examples

| Example | Description |
|---|---|
| `minimal` | One-shot `Agent::fire` |
| `multi_turn` | Persistent multi-turn conversation |
| `streaming` / `streaming_with_tools` | Token streaming with/without tools |
| `custom_tool` | Define and register a custom tool |
| `middleware` | Inspector/Mutator middleware pipeline |
| `plugin_architecture` | Reactive plugin lifecycle and HMR (no LLM) |
| `loader_declarative` | JSON/YAML declarative plugin loading |
| `service_broker` | Multi-provider round-robin routing |
| `replace_core_tool` | Replace a built-in tool at runtime |
| `replace_loop` / `replace_react_loop` | Replace the built-in ReAct loop with a custom `AgentLoop` |
| `tool_policy` / `security` / `sandbox` | Security policies, audit, and sandboxing |

## Plugin architecture

funera composes agents from **plugins** — one unified abstraction over tools, providers, skills, and middleware.

- **`Plugin` trait** — `name` (identity), `inject` (dependencies it reads), `provides` (services it writes), and an async `apply` (load hook). `Tool`, `ChatProvider`, `InspectorMiddleware`, and `MutatorMiddleware` are all subtraits of `Plugin`, so any capability is also a mountable plugin. Adapter plugins (`ToolPlugin`, `SkillPlugin`, `MiddlewarePlugin`, `ProviderPlugin`, `CallbackPlugin`) wrap existing capability traits.
- **Capability layer** — a `FuneraEnv` carries a typed service table (primary `TypeId` slots plus named `ServiceKey` slots) and a per-env effect accumulator: `env.effect(..)` registers a reversible effect (the returned disposer runs on unload, in LIFO order), while `env.provide::<T>(..)` / `env.get::<T>()` publish and resolve typed services.
- **`PluginRegistry`** — a notification-driven, typestate lifecycle (`Pending → Loading → Active → Unloading → Inactive/Failed`) with provider identity, retry backoff, and metrics. It activates a plugin only once its `inject` requirements are met and deactivates it when they are withdrawn.
- **`Loader`** — reconciles a declarative plugin set (a list of `PluginEntry`s) against the registry with minimal mount/unmount operations; bumping an entry's `revision` hot-replaces it in place, with `HmrPolicy::Replace` (new-first, rollback on failure) or `HmrPolicy::Swap`. `AgentRuntime::reconcile_plugins` exposes the same reconciliation at runtime.
- **`ServiceBroker`** — a round-robin broker for multi-provider services. Providers register through [`BrokerProvider`](funera_core::plugin::BrokerProvider), consumers inject the broker and call [`next`](funera_core::plugin::ServiceBroker::next). Run `cargo run -p funera-orchestrate --example service_broker`.

Run the end-to-end demo:

```bash
cargo run -p funera-orchestrate --example plugin_architecture
```

See `funera-orchestrate/examples/plugin_architecture.rs` for the full reactive-lifecycle walkthrough: dependency-driven activation, deactivation on provider removal, and hot reload.

Declarative plugin sets can be loaded from JSON/YAML via [`PluginFactory`](funera_core::loader::config::PluginFactory):

```rust,no_run
use funera_core::loader::config::{PluginFactory, load_entries_from_json};
use funera_core::plugin::{Plugin, PluginConfig};

struct HelloPlugin;
impl Plugin for HelloPlugin { fn name(&self) -> &str { "hello" } }

let mut factory = PluginFactory::new();
factory.register("hello", |_cfg: Option<PluginConfig>| HelloPlugin);

let entries = load_entries_from_json(
    r#"[{"id": "hello-1", "plugin": "hello", "revision": 1}]"#,
    &factory,
)?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

At runtime, an `AgentRuntime` can reconcile a desired plugin set without rebuilding the runtime:

```rust,no_run
# use funera_core::loader::PluginEntry;
# use funera_core::plugin::ToolPlugin;
# use std::sync::Arc;
# async fn example(rt: &funera_orchestrate::AgentRuntime<funera_orchestrate::DeepSeekProvider>) {
# struct MyTool; impl funera_core::plugin::Plugin for MyTool { fn name(&self) -> &str { "my_tool" } }
let report = rt.reconcile_plugins(vec![
    PluginEntry::new("tool:my_tool", Arc::new(ToolPlugin::new(Arc::new(MyTool)))),
]).await;
# }
```

Run the declarative loader demo:

```bash
cargo run -p funera-orchestrate --example loader_declarative
```

### Limitations

- `FuneraEnv::dispose` runs sync disposers in LIFO order with panic isolation. A sync disposer cannot be safely timed out by the runtime; plugin authors should keep disposers short and non-blocking. Async teardown can be scheduled from the disposer (as the built-in tool/skill adapters do) and awaited by the caller if needed.
- `security`-featured tool execution runs outside the registry lock via a cloned guarded registry; the clone shares approval and react-bus state, while policy/path configuration is snapshotted at call time.

## Installation

Add the root crate to your `Cargo.toml`:

```toml
[dependencies]
funera = { git = "https://github.com/dynamder/funera" }
```

### Features

| Feature | Default | Description |
|---------|:-------:|-------------|
| `funera-builtin-tools` | ❌ | Bundled Read, Write, Edit, Shell tools |
| `tool` | ✅ | Tool system (trait, registry, executor) |
| `deepseek` | ✅ | DeepSeek provider |
| `openai` | ❌ | OpenAI provider |
| `security` | ❌ | Tool policy enforcement, path guards, audit logging |
| `sandbox` | ❌ | Kernel-level subprocess isolation (Landlock/Seatbelt/Token) |
| `middleware` | ❌ | Event interception pipeline |
| `skill` | ❌ | Skill loading and prompt injection |

## Quick Start

### One-shot query

```rust
use funera::{Agent, AgentRuntime, DeepSeekProvider};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = AgentRuntime::<DeepSeekProvider>::builder()
        .api_key(std::env::var("DEEPSEEK_API_KEY")?)
        .model("deepseek-v4-flash")
        .build()?;

    let agent = Agent::builder()
        .system_prompt("You are a helpful assistant.")
        .build();

    let resp = agent.fire("Hello!", &runtime).await?;
    println!("{}", resp.content);
    Ok(())
}
```

### Streaming with callbacks

```rust
use funera::{Agent, AgentEvent, AgentRuntime, DeepSeekProvider};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = AgentRuntime::<DeepSeekProvider>::builder()
        .api_key(std::env::var("DEEPSEEK_API_KEY")?)
        .model("deepseek-v4-flash")
        .build()?;

    let agent = Agent::builder()
        .on_token(|t| print!("{t}"))
        .build();

    let mut rx = agent.fire_stream("Explain Rust ownership", &runtime).await?;
    while let Some(event) = rx.recv().await {
        if let AgentEvent::Text(t) = event {
            print!("{t}");
        }
    }
    Ok(())
}
```

### Multi-turn conversation

```rust
let runtime = AgentRuntime::<DeepSeekProvider>::builder()
    .api_key(std::env::var("DEEPSEEK_API_KEY")?)
    .model("deepseek-v4-flash")
    .build()?;

let agent = Agent::builder()
    .system_prompt("You are helpful.")
    .build();

let handle = agent.send("Hi, I'm Alice.", runtime).await?;
let (runtime, _resp) = handle.await?;
let handle = agent.send("What's my name?", runtime).await?;
let (_runtime, _resp) = handle.await?;
```

### Runtime hot-reload

```rust
let runtime = AgentRuntime::<DeepSeekProvider>::builder()
    .api_key(std::env::var("DEEPSEEK_API_KEY")?)
    .model("deepseek-v4-flash")
    .build()?;

// Switch model mid-conversation — picked up on next ReAct iteration
runtime.set_model("deepseek-r1");

// Dynamically add a tool
runtime.add_tool(Box::new(MyTool));

// Subscribe to env state changes
let mut env_rx = runtime.subscribe_env_state().await;
```

### Custom tool

```rust
use async_trait::async_trait;
use funera::core::plugin::Plugin;
use funera::core::re_act::tool::{Tool, ToolCallError};
use serde_json::{json, Value as JsonValue};

struct Calculator;

impl Plugin for Calculator {
    fn name(&self) -> &str { "calculator" }
}

#[async_trait]
impl Tool for Calculator {
    fn description(&self) -> &str { "Evaluate a math expression" }
    fn schema(&self) -> JsonValue {
        json!({
            "type": "function",
            "function": {
                "name": "calculator",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "expression": { "type": "string" }
                    },
                    "required": ["expression"]
                }
            }
        })
    }
    async fn execute(&self, args: JsonValue) -> Result<String, ToolCallError> {
        let expr = args["expression"].as_str().unwrap_or("");
        Ok(format!("TODO: compute {expr}"))
    }
}

// Register it:
let runtime = AgentRuntime::<DeepSeekProvider>::builder()
    .api_key(std::env::var("DEEPSEEK_API_KEY")?)
    .model("deepseek-v4-flash")
    .with_tool_instance(Box::new(Calculator))
    .build()?;
```

### Security configuration

Requires the `security` feature (and optionally `funera-builtin-tools`, `sandbox`):

```rust
use funera::{Agent, AgentRuntime, DeepSeekProvider, ToolPolicy, ShellPolicy};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = AgentRuntime::<DeepSeekProvider>::builder()
        .api_key(std::env::var("DEEPSEEK_API_KEY")?)
        .model("deepseek-v4-flash")
        .with_builtin_tools()
        .with_tool_policy(ToolPolicy {
            denied_tools: ["shell".into()].into_iter().collect(),
            shell_policy: Some(ShellPolicy::with_allowed(
                vec!["git".into(), "cargo".into()],
            )),
            ..Default::default()
        })
        .build()?;

    let agent = Agent::builder().build();
    let resp = agent.fire("List the git log.", &runtime).await?;
    println!("{}", resp.content);
    Ok(())
}
```

## Project structure

```
funera/
├── funera_core/          Core agent engine
│   └── src/
│       ├── chat/         Message types, session actor
│       ├── env.rs        Runtime environment, watch hot-reload, capability layer
│       ├── env_actor.rs  EnvActor — single source of truth for all env state
│       ├── event_bus/    Token, React, EnvState, Tool buses
│       ├── loader.rs     Declarative plugin loader (PluginEntry, Loader, HMR)
│       ├── middleware.rs  Event interception pipeline
│       ├── plugin.rs     Plugin trait, PluginRegistry, PluginInstance
│       ├── provider/     OpenAI & DeepSeek backends
│       ├── re_act/       ReAct loop, Tool trait, Skill system
│       └── security/     Policies, path guard, audit, secrets
├── funera-orchestrate/   High-level builder API
│   ├── src/
│   │   ├── agent.rs      Agent & AgentBuilder
│   │   ├── runtime.rs    AgentRuntime & AgentRuntimeBuilder
│   │   ├── event.rs      AgentEvent enum
│   │   ├── dispatcher.rs  Callback dispatch
│   │   └── send_handle.rs Ownership handles
│   └── examples/         Example programs (incl. plugin_architecture)
├── funera_builtin_tools/  Default tool implementations
│   └── src/
│       ├── read.rs       ReadTool (file/dir, hashline output)
│       ├── write.rs      WriteTool (auto parent dirs)
│       ├── edit.rs       EditTool (hashline-anchored editing)
│       └── shell.rs      ShellTool (cross-platform, timeout)
```

## Contributing

Contributions are welcome! Please read:

- [CONTRIBUTING.md](CONTRIBUTING.md) — build, test, and pull-request workflow
- [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) — community standards
- [SECURITY.md](SECURITY.md) — reporting security vulnerabilities

## License

MIT — see the [repository](https://github.com/dynamder/Funera) for details.
