//! # funera-core
//!
//! Core LLM agent engine providing the ReAct execution loop, message/session
//! management, event buses, a pluggable provider architecture, a tool system,
//! a skill system, a middleware pipeline, and multi-layered security.
//!
//! ## Modules
//!
//! | Module | Description |
//! |--------|-------------|
//! | [`mod@chat`] | Message types and session actor for conversation history |
//! | [`mod@env`] | Shared runtime environment (tool registry, skill registry, LLM client) |
//! | [`mod@event_bus`] | Event buses for streaming tokens, ReAct cycle events, session lifecycle, and tool commands |
//! | [`mod@provider`] | Provider abstraction over LLM backends (OpenAI, DeepSeek) |
//! | [`mod@re_act`] | The ReAct execution loop, [`Tool`](re_act::tool::Tool) trait and registry, and skill system |
//! | [`mod@security`] | Tool/shell policy enforcement, path allowlisting, audit logging, and secure API key storage |
//! | [`mod@middleware`] | Pluggable event interception pipeline (Inspector + Mutator) with typestate error channel |

pub mod chat;
pub mod env;
pub mod event_bus;
pub mod provider;
pub mod re_act;
pub mod security;
pub mod middleware;

#[cfg(any(test, feature = "test-utils"))]
pub mod test_helpers;
