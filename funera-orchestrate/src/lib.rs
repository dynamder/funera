//! # funera-orchestrate
//!
//! **Easy-to-use orchestration layer for [funera-core].**
//!
//! This crate provides a high-level agent API (`Agent`) and a runtime container
//! (`AgentRuntime`) that together let you integrate funera's LLM agent runtime
//! into your own projects with minimal boilerplate.
//!
//! ---
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use funera_orchestrate::{Agent, AgentRuntime, DeepSeekProvider};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let runtime = AgentRuntime::<DeepSeekProvider>::builder()
//!         .api_key(std::env::var("OPENAI_API_KEY")?)
//!         .model("gpt-4o")
//!         .build()?;
//!
//!     let agent = Agent::builder()
//!         .system_prompt("You are a helpful assistant.")
//!         .build();
//!
//!     let resp = agent.fire("Hello!", &runtime).await?;
//!     println!("{}", resp.content);
//!     Ok(())
//! }
//! ```
//!
//! ## Core Concepts
//!
//! | Concept | Type | Description |
//! |---------|------|-------------|
//! | **Runtime** | [`AgentRuntime`] | Shared infrastructure + conversation session |
//! | **Agent** | [`Agent`] | Behavioural config (system prompt, callbacks) |
//! | **One-shot** | [`Agent::fire`] | Temporary session, discarded after call |
//! | **Multi-turn** | [`Agent::send`] | Persistent session across calls |
//! | **Streaming** | [`fire_stream`](Agent::fire_stream) / [`send_stream`](Agent::send_stream) | Token-by-token streaming |
//!
//! ## Features
//!
//! | Feature | Description |
//! |---------|-------------|
//! | `builtin-tools` | Bundles Read, Write, Edit, Shell tools (requires `builtin_tools` crate) |
//! | `security` | Enables tool security policies, path guards, and audit logging |
//!
//! ## Examples
//!
//! ### One-shot query with stream
//!
//! ```rust,no_run
//! # use funera_orchestrate::{Agent, AgentEvent, AgentRuntime, DeepSeekProvider};
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let runtime = AgentRuntime::<DeepSeekProvider>::builder()
//!     .api_key(std::env::var("OPENAI_API_KEY")?)
//!     .model("gpt-4o")
//!     .build()?;
//!
//! let agent = Agent::builder()
//!     .on_token(|t| print!("{t}"))
//!     .build();
//!
//! let mut rx = agent.fire_stream("Explain Rust's ownership model", &runtime).await?;
//! while let Some(event) = rx.recv().await {
//!     if let AgentEvent::Token(t) = event {
//!         print!("{t}");
//!     }
//! }
//! # Ok(())
//! # }
//! ```
//!
//! ### Multi-turn conversation with callbacks
//!
//! ```rust,no_run
//! # use funera_orchestrate::{Agent, AgentRuntime, DeepSeekProvider};
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let mut runtime = AgentRuntime::<DeepSeekProvider>::builder()
//!     .api_key(std::env::var("OPENAI_API_KEY")?)
//!     .model("gpt-4o")
//!     .build()?;
//!
//! let agent = Agent::builder()
//!     .system_prompt("You are helpful.")
//!     .on_tool_call(|name, _| eprintln!("[tool] {name}"))
//!     .on_turn_start(|| eprintln!("--- turn ---"))
//!     .build();
//!
//! agent.send("Hi, I'm Alice.", &mut runtime).await?;
//! agent.send("What's my name?", &mut runtime).await?;
//! # Ok(())
//! # }
//! ```
//!
//! ### Switching runtimes
//!
//! ```rust,no_run
//! # use funera_orchestrate::{Agent, AgentRuntime, DeepSeekProvider};
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let mut gpt = AgentRuntime::<DeepSeekProvider>::builder()
//!     .api_key("sk-...").model("gpt-4o").build()?;
//!
//! let mut claude = AgentRuntime::<DeepSeekProvider>::builder()
//!     .api_key("sk-ant-...").model("claude-3-opus").build()?;
//!
//! let agent = Agent::builder().build();
//!
//! agent.send("What is Rust?", &mut gpt).await?;     // session in gpt
//! agent.fire("What is Python?", &claude).await?;     // session in claude
//! agent.send("Tell me more", &mut gpt).await?;       // continues gpt session
//! # Ok(())
//! # }
//! ```
//!
//! ## Module Structure
//!
//! - [`runtime`] — [`AgentRuntimeBuilder`] and [`AgentRuntime`]
//! - [`agent`] — [`AgentBuilder`] and [`Agent`]
//! - [`dispatcher`] — Event bus subscription and callback dispatch
//! - [`event`] — [`AgentEvent`] enum
//! - [`response`] — [`ChatResponse`] and [`ToolCallInfo`]
//! - [`error`] — [`OrchestrateError`]

pub mod agent;
pub mod dispatcher;
pub mod error;
pub mod event;
pub mod response;
pub mod runtime;

pub use agent::{Agent, AgentBuilder};
pub use dispatcher::CallbackRegistry;
pub use error::OrchestrateError;
pub use event::AgentEvent;
#[cfg(feature = "deepseek")]
pub use funera_core::provider::deepseek::DeepSeekProvider;
#[cfg(feature = "openai")]
pub use funera_core::provider::openai::OpenAIProvider;
pub use response::{ChatResponse, ToolCallInfo};
pub use runtime::{AgentRuntime, AgentRuntimeBuilder};
