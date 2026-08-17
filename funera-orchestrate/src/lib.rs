//! # funera-orchestrate
//!
//! **Easy-to-use orchestration layer for [funera-core].**
//!
//! This crate provides a high-level agent API (`Agent`) and a runtime container
//! (`AgentRuntime`) that together let you integrate funera's LLM agent runtime
//! into your own projects with minimal boilerplate.
//!
//! ## Features
//!
//! | Feature | Default | Description |
//! |---------|---------|-------------|
//! | `deepseek` | ✅ | DeepSeek provider |
//! | `openai` | ❌ | OpenAI provider |
//! | `tool` | ✅ | Tool system (trait, registry, executor) |
//! | `funera-builtin-tools` | ❌ | Built-in tools (Read, Write, Edit, Shell) |
//! | `security` | ❌ | Tool policy enforcement |
//! | `middleware` | ❌ | Event interception pipeline (Inspector + Mutator) |
//! | `skill` | ❌ | Skill loading and prompt injection |
//! | `sandbox` | ❌ | Kernel-level subprocess isolation |
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
//!         .api_key(std::env::var("DEEPSEEK_API_KEY")?)
//!         .model("deepseek-v4-flash")
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
//! | **Approval** | [`ApprovalHandle`] | Lightweight cloneable handle for tool-call approval |
//!
//! ## Examples
//!
//! ### One-shot query with stream
//!
//! ```rust,no_run
//! # use funera_orchestrate::{Agent, AgentEvent, AgentRuntime, DeepSeekProvider};
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let runtime = AgentRuntime::<DeepSeekProvider>::builder()
//!     .api_key(std::env::var("DEEPSEEK_API_KEY")?)
//!     .model("deepseek-v4-flash")
//!     .build()?;
//!
//! let agent = Agent::builder()
//!     .on_token(|t| print!("{t}"))
//!     .build();
//!
//! let mut rx = agent.fire_stream("Explain Rust's ownership model", &runtime).await?;
//! while let Some(event) = rx.recv().await {
//!     if let AgentEvent::Text(t) = event {
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
//! let runtime = AgentRuntime::<DeepSeekProvider>::builder()
//!     .api_key(std::env::var("DEEPSEEK_API_KEY")?)
//!     .model("deepseek-v4-flash")
//!     .build()?;
//!
//! let agent = Agent::builder()
//!     .system_prompt("You are helpful.")
//!     .on_tool_call(|name, _| eprintln!("[tool] {name}"))
//!     .on_turn_start(|| eprintln!("--- turn ---"))
//!     .build();
//!
//! let handle = agent.send("Hi, I'm Alice.", runtime).await?;
//! let (runtime, _resp) = handle.await?;
//! let handle = agent.send("What's my name?", runtime).await?;
//! let (_runtime, _resp) = handle.await?;
//! # Ok(())
//! # }
//! ```
//!
//! ### Switching models on the same provider
//!
//! ```rust,no_run
//! # use funera_orchestrate::{Agent, AgentRuntime, DeepSeekProvider};
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let fast = AgentRuntime::<DeepSeekProvider>::builder()
//!     .api_key(std::env::var("DEEPSEEK_API_KEY")?)
//!     .model("deepseek-v4-flash")
//!     .build()?;
//!
//! let powerful = AgentRuntime::<DeepSeekProvider>::builder()
//!     .api_key(std::env::var("DEEPSEEK_API_KEY")?)
//!     .model("deepseek-r1")
//!     .build()?;
//!
//! let agent = Agent::builder().build();
//!
//! let (fast, _) = agent.send("Hello", fast).await?.await?;              // fast model
//! agent.fire("What is Rust?", &powerful).await?;                        // powerful model (temp)
//! let (_fast, _) = agent.send("Tell me more", fast).await?.await?;       // back to fast
//! # Ok(())
//! # }
//! ```
//!
//! ### Security with tool-call approval
//!
//! Requires the `security` feature (and optionally `funera-builtin-tools`, `sandbox`).
//! Use [`ApprovalHandle`] to approve or reject tool calls from a spawned task
//! while the agent is running — works with `fire()`, `send()`, and `send_stream()`.
//!
//! ```rust,no_run
//! # use funera_orchestrate::{Agent, AgentRuntime, ApprovalHandle, DeepSeekProvider};
//! # use std::time::Duration;
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let (approval_tx, mut approval_rx) = tokio::sync::mpsc::unbounded_channel();
//!
//! let runtime = AgentRuntime::<DeepSeekProvider>::builder()
//!     .api_key(std::env::var("DEEPSEEK_API_KEY")?)
//!     .model("deepseek-v4-flash")
//!     .on_approval_required(move |call_id, tool, reason| {
//!         eprintln!("[{tool}] needs approval: {reason}");
//!         let _ = approval_tx.send(call_id.to_string());
//!     })
//!     .with_approval_timeout(Duration::from_secs(30))
//!     .build()?;
//!
//! // Clone the ApprovalHandle *before* send() consumes the runtime.
//! let approver: ApprovalHandle = runtime.approval_handle();
//! tokio::spawn(async move {
//!     while let Some(call_id) = approval_rx.recv().await {
//!         approver.approve_tool_call(&call_id, true).await.ok();
//!     }
//! });
//!
//! let agent = Agent::builder().build();
//! let (_runtime, resp) = agent.send("do something", runtime).await?.await?;
//! println!("{}", resp.content);
//! # Ok(())
//! # }
//! ```
//!
//! ## Module Structure
//!
//! - [`runtime`] — [`AgentRuntimeBuilder`] and [`AgentRuntime`]
//! - [`agent`] — [`AgentBuilder`] and [`Agent`]
//! - [`send_handle`] — [`SendHandle`], [`SendStreamHandle`], [`FireStreamHandle`], [`ApprovalHandle`]
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
pub mod send_handle;

#[cfg(feature = "middleware")]
pub mod middleware_bundle;

pub use agent::{Agent, AgentBuilder};
pub use dispatcher::CallbackRegistry;
pub use error::OrchestrateError;
pub use event::{AgentEvent, RawAgentEvent};
#[cfg(feature = "deepseek")]
pub use funera_core::provider::deepseek::DeepSeekProvider;
#[cfg(feature = "openai")]
pub use funera_core::provider::openai::OpenAIProvider;
pub use response::{ChatResponse, ToolCallInfo};
pub use runtime::{Acquired, AgentRuntime, AgentRuntimeBuilder, Idle};
#[cfg(all(feature = "tool", feature = "security"))]
pub use send_handle::ApprovalHandle;
pub use send_handle::{FireStreamHandle, SendHandle, SendStreamHandle};

// Re-export security policy types for convenience.
#[cfg(feature = "security")]
pub use funera_core::security::audit::{AuditBus, AuditEvent};
#[cfg(feature = "security")]
pub use funera_core::security::policy::{PolicyError, ShellPolicy, ToolPolicy};

// Re-export core event types for direct access
pub use funera_core::event_bus::env_state_bus::EnvStateEvent;
pub use funera_core::event_bus::react_bus::{
    ReactEvent, ToolCallErrorInfo, ToolCallRequest, ToolCallResponse,
};
pub use funera_core::event_bus::token_bus::TokenEvent;

/// Middleware 相关的类型和 trait。
///
/// 该模块提供了 [`InspectorMiddleware`]、[`MutatorMiddleware`] 等核心 trait，
/// 以及 [`MiddlewareChain`]、[`MiddlewareBundle`] 等构建管道所需的类型。
///
/// ## Feature gate
///
/// 需要启用 `middleware` feature：
///
/// ```toml
/// funera-orchestrate = { features = ["middleware"] }
/// ```
#[cfg(feature = "middleware")]
pub mod middleware {
    pub use crate::middleware_bundle::MiddlewareBundle;
    pub use funera_core::middleware::*;
}
