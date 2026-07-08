//! # funera-orchestrate
//!
//! **Easy-to-use orchestration layer for [funera-core].**
//!
//! This crate provides a high-level builder API (`AgentBuilder` → `Agent`) and a
//! mid-level `Orchestrator` to quickly integrate funera's LLM agent runtime into
//! your own Rust projects — without needing to manually wire up tool registries,
//! event buses, sessions, or the ReAct loop.
//!
//! ---
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use funera_orchestrate::Agent;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let mut agent = Agent::builder()
//!         .api_key(std::env::var("OPENAI_API_KEY")?)
//!         .model("gpt-4o")
//!         .build()?;
//!
//!     let response = agent.chat("Hello!").await?;
//!     println!("{}", response.content);
//!     Ok(())
//! }
//! ```
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
//! ### Agent with built-in tools
//!
//! ```rust,no_run,ignore
//! # use funera_orchestrate::Agent;
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let mut agent = Agent::builder()
//!     .api_key("sk-...")
//!     .model("gpt-4o")
//!     .with_builtin_tools()   // requires "builtin-tools" feature
//!     .build()?;
//!
//! let resp = agent.chat("Read Cargo.toml").await?;
//! println!("{}", resp.content);
//! # Ok(())
//! # }
//! ```
//!
//! ### Streaming tokens
//!
//! ```rust,no_run
//! # use funera_orchestrate::{Agent, AgentEvent};
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let mut agent = Agent::builder()
//!     .api_key("sk-...")
//!     .model("gpt-4o")
//!     .build()?;
//!
//! let mut rx = agent.chat_stream("Explain Rust's ownership model").await?;
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
//! # use funera_orchestrate::Agent;
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let mut agent = Agent::builder()
//!     .api_key("sk-...")
//!     .model("gpt-4o")
//!     .on_token(|t| print!("{t}"))
//!     .on_tool_call(|name, args| eprintln!("[tool] {name}: {args}"))
//!     .on_turn_start(|| eprintln!("--- turn start ---"))
//!     .build()?;
//!
//! let r1 = agent.send("Hi, I'm Alice.").await?;
//! let r2 = agent.send("What's my name?").await?; // remembers Alice
//! println!("{}", r2.content);
//! # Ok(())
//! # }
//! ```
//!
//! ## Architecture
//!
//! The crate wraps `funera_core`'s raw building blocks into a clean API:
//!
//! ```text
//! ┌──────────────┐         ┌─────────────────────────────────────┐
//! │  AgentBuilder │ ──build──►│               Agent                │
//! │  · api_key()  │         │  ┌─────────────────────────────┐   │
//! │  · model()    │         │  │  FuneraSession (type-state) │   │
//! │  · tools()    │         │  │  ReActLoop                  │   │
//! │  · callbacks()│         │  │  ToolExecutor (background)  │   │
//! └──────────────┘         │  │  CallbackDispatcher         │   │
//!                          │  └─────────────────────────────┘   │
//!                          └─────────────────────────────────────┘
//! ```
//!
//! ## Modules
//!
//! - [`agent`] — High-level `AgentBuilder` and `Agent`
//! - [`orchestrator`] — Mid-level `Orchestrator` for advanced use-cases
//! - [`dispatcher`] — Event bus subscription and callback dispatch
//! - [`event`] — [`AgentEvent`] enum for streaming responses
//! - [`response`] — [`ChatResponse`] and related types
//! - [`error`] — [`OrchestrateError`] enum

pub mod agent;
pub mod dispatcher;
pub mod error;
pub mod event;
pub mod orchestrator;
pub mod response;

pub use agent::{Agent, AgentBuilder};
pub use dispatcher::CallbackRegistry;
pub use error::OrchestrateError;
pub use event::AgentEvent;
pub use orchestrator::{Orchestrator, OrchestratorConfig};
pub use response::{ChatResponse, ToolCallInfo};
