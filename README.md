# Funera

An LLM agent framework for Rust. Build AI agents with tools, skills, middleware, and pluggable
LLM backends — all with multi-layered security.

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/edition-2024-orange)](https://rust-lang.org)

## Architecture

```
┌─────────────────────────────────────────────────────┐
│  funera-orchestrate    high-level builder API       │
│  Agent · AgentRuntime · callbacks · streaming       │
├─────────────────────────────────────────────────────┤
│  funera_core           core engine                  │
│  ReActLoop · Session · EventBus · Middleware        │
│  Security · Provider · Tools · Skills               │
├─────────────────────────────────────────────────────┤
│  builtin_tools         default tool implementations │
│  ReadTool · WriteTool · EditTool · ShellTool        │
└─────────────────────────────────────────────────────┘
```

## Features

- **ReAct loop** — iterative tool-calling agent execution with configurable max iterations
- **Pluggable providers** — OpenAI and DeepSeek backends with streaming support
- **Tool system** — define custom tools by implementing the `Tool` trait; built-in file I/O and shell
- **Skill system** — load prompt templates from YAML-frontmatter Markdown files
- **Middleware pipeline** — intercept agent events with inspectors (read-only, parallel) and mutators
  (pass/modify/block, sequential)
- **Security layer** — tool/shell policies, path allowlisting, audit logging, secure API key storage
- **Type-state session** — compile-time enforcement of session ownership (`Idle` / `Acquired`)

## Installation

Add the orchestration crate to your `Cargo.toml`:

```toml
[dependencies]
funera-orchestrate = { git = "https://github.com/anomalyco/funera" }
```

### Features

| Feature | Default | Description |
|---------|:-------:|-------------|
| `deepseek` | ✅ | DeepSeek provider |
| `openai` | ❌ | OpenAI provider |
| `builtin-tools` | ❌ | Bundled Read, Write, Edit, Shell tools |
| `security` | ✅ | Tool policy enforcement, path guards, audit logging |
| `middleware` | ❌ | Event interception pipeline |
| `tool` | ✅ | Tool execution support |
| `skill` | ❌ | Skill loading and prompt injection |

## Quick Start

### One-shot query

```rust
use funera_orchestrate::{Agent, AgentRuntime, DeepSeekProvider};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = AgentRuntime::<DeepSeekProvider>::builder()
        .api_key(std::env::var("OPENAI_API_KEY")?)
        .model("gpt-4o")
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
use funera_orchestrate::{Agent, AgentEvent, AgentRuntime, DeepSeekProvider};

let runtime = AgentRuntime::<DeepSeekProvider>::builder()
    .api_key(std::env::var("OPENAI_API_KEY")?)
    .model("gpt-4o")
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
```

### Multi-turn conversation

```rust
let mut runtime = AgentRuntime::<DeepSeekProvider>::builder()
    .api_key(std::env::var("OPENAI_API_KEY")?)
    .model("gpt-4o")
    .build()?;

let agent = Agent::builder()
    .system_prompt("You are helpful.")
    .build();

let handle = agent.send("Hi, I'm Alice.", runtime).await?;
let (runtime, _resp) = handle.await?;
let handle = agent.send("What's my name?", runtime).await?;
let (_runtime, _resp) = handle.await?;
```

### Using built-in tools

```rust
let runtime = AgentRuntime::<DeepSeekProvider>::builder()
    .api_key(std::env::var("OPENAI_API_KEY")?)
    .model("gpt-4o")
    .with_builtin_tools()   // registers Read, Write, Edit, Shell
    .build()?;

let agent = Agent::builder()
    .system_prompt("You can read and write files.")
    .on_tool_call(|name, _args| eprintln!("[tool] {name}"))
    .build();

let resp = agent.fire("Read Cargo.toml and summarize it", &runtime).await?;
```

### Custom tool

```rust
use async_trait::async_trait;
use funera_core::re_act::tool::{Tool, ToolCallError};
use serde_json::{json, Value as JsonValue};

struct Calculator;

#[async_trait]
impl Tool for Calculator {
    fn name(&self) -> &str { "calculator" }
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
    .api_key(std::env::var("OPENAI_API_KEY")?)
    .model("gpt-4o")
    .with_tool_instance(Box::new(Calculator))
    .build()?;
```

## Project structure

```
funera/
├── funera_core/          Core agent engine
│   └── src/
│       ├── chat/         Message types, session actor
│       ├── env.rs        Shared runtime environment
│       ├── event_bus/    Token, React, EnvState, Tool buses
│       ├── middleware.rs  Event interception pipeline
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
│   └── examples/         10 example programs
├── builtin_tools/        Default tool implementations
│   └── src/
│       ├── read.rs       ReadTool (file/dir, hashline output)
│       ├── write.rs      WriteTool (auto parent dirs)
│       ├── edit.rs       EditTool (hashline-anchored editing)
│       └── shell.rs      ShellTool (cross-platform, timeout)
└── tui/                  TUI frontend (stub)
```

## License

MIT — see the [repository](https://github.com/anomalyco/funera) for details.
