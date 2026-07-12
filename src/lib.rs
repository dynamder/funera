//! # funera
//!
//! High-level LLM agent framework for Rust. Build AI agents with tools, skills, middleware,
//! and pluggable LLM backends — all with multi-layered security.
//!
//! ## Quick Start
//!
//! ```rust,ignore
//! use funera::{Agent, AgentRuntime, DeepSeekProvider};
//!
//! let runtime = AgentRuntime::<DeepSeekProvider>::builder()
//!     .api_key(std::env::var("OPENAI_API_KEY")?)
//!     .model("gpt-4o")
//!     .build()?;
//!
//! let agent = Agent::builder()
//!     .system_prompt("You are a helpful assistant.")
//!     .build();
//!
//! let resp = agent.fire("Hello!", &runtime).await?;
//! println!("{}", resp.content);
//! ```
//!
//! ## Accessing the core layer
//!
//! Lower-level APIs from `funera_core` are available under [`core`]:
//!
//! ```rust,ignore
//! use funera::core::re_act::tool::Tool;
//! use funera::core::security::policy::ToolPolicy;
//! use funera::core::middleware::MiddlewareChain;
//! ```

/// Re-export of the public orchestration API.
///
/// This is the primary entry point for most users.
pub use funera_orchestrate::*;

/// Re-export of the core engine crate.
///
/// Provides low-level access to the ReAct loop, tool/skill system,
/// security policies, event buses, and middleware.
pub use funera_core as core;
