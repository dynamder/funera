#![cfg(feature = "tool")]

use std::{collections::HashMap, fmt::Display};

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use thiserror::Error;

/// The type of a tool, as communicated to the LLM.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ToolType {
    /// Standard OpenAI-compatible function tool.
    Function,
}

impl Display for ToolType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "function")
    }
}

/// A callable tool exposed to the LLM agent.
///
/// Implement this trait to define custom tools. The framework will expose
/// the tool's [`schema`](Tool::schema) to the LLM and invoke
/// [`execute`](Tool::execute) when the LLM requests it.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Unique name for this tool (e.g. `"read"`, `"shell"`).
    fn name(&self) -> &str;

    /// Human-readable description sent to the LLM.
    fn description(&self) -> &str;

    /// Execute the tool with the given JSON arguments.
    ///
    /// Returns a string result on success, or a [`ToolCallError`] on failure.
    async fn execute(&self, args: JsonValue) -> Result<String, ToolCallError>;

    /// Returns the tool type (defaults to [`ToolType::Function`]).
    fn get_type(&self) -> ToolType {
        ToolType::Function
    }

    /// Returns the JSON schema describing this tool's parameters.
    ///
    /// This is sent to the LLM so it can generate well-formed invocations.
    fn schema(&self) -> JsonValue;
}

/// Errors that can occur during tool execution.
#[derive(Debug, Error)]
pub enum ToolCallError {
    /// The arguments did not match the expected schema.
    #[error("parameter mismatch: {0}")]
    ParameterMismatch(JsonValue),

    /// The tool encountered a runtime error during execution.
    #[error("tool execution error: {0}")]
    ToolExecutionError(#[from] anyhow::Error),

    /// The tool exists but is currently unavailable (e.g. disabled by policy).
    #[error("tool unavailable: {0}")]
    ToolUnavailable(String),

    /// No tool with the given name is registered.
    #[error("tool not found: {0}")]
    ToolNotFound(String),
}

/// An entry in the tool registry, pairing a tool with its availability status.
pub struct ToolRegistryEntry {
    pub tool: Box<dyn Tool>,
    pub available: bool,
}
impl ToolRegistryEntry {
    /// Create a new registry entry with explicit availability.
    pub fn new(tool: Box<dyn Tool>, available: bool) -> Self {
        Self { tool, available }
    }

    /// Whether the tool is currently available for execution.
    pub fn is_available(&self) -> bool {
        self.available
    }

    /// Create a new registry entry with the tool available.
    pub fn new_available(tool: Box<dyn Tool>) -> Self {
        Self::new(tool, true)
    }

    /// Create a new registry entry with the tool unavailable.
    pub fn new_unavailable(tool: Box<dyn Tool>) -> Self {
        Self::new(tool, false)
    }
}

/// Raw tool registry (no security checks).
///
/// When the `security` feature is enabled, [`ToolRegistry`] aliases to
/// [`GuardedToolRegistry`](crate::security::registry::GuardedToolRegistry)
/// instead, which wraps this registry with policy checks and audit logging.
#[doc(hidden)]
pub struct RawToolRegistry {
    tools: HashMap<String, ToolRegistryEntry>,
}

impl RawToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }
    pub fn add_tool(&mut self, tool: Box<dyn Tool>) {
        self.tools.insert(
            tool.name().to_string(),
            ToolRegistryEntry::new_available(tool),
        );
    }
    pub fn get_tool(&self, name: &str) -> Option<&ToolRegistryEntry> {
        self.tools.get(name)
    }
    pub fn remove_tool(&mut self, name: &str) {
        self.tools.remove(name);
    }
    pub fn tool_exists(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }
    pub fn tool_count(&self) -> usize {
        self.tools.len()
    }
    pub fn get_all_tools(&self) -> &HashMap<String, ToolRegistryEntry> {
        &self.tools
    }
    pub fn available_tools_json(&self) -> JsonValue {
        self.tools
            .values()
            .filter_map(|tool| {
                if tool.is_available() {
                    Some(tool.tool.schema())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .into()
    }
    pub async fn call_tool(&self, name: &str, args: JsonValue) -> Result<String, ToolCallError> {
        if let Some(tool) = self.get_tool(name) {
            if tool.is_available() {
                tool.tool.execute(args).await
            } else {
                Err(ToolCallError::ToolUnavailable(name.to_string()))
            }
        } else {
            Err(ToolCallError::ToolNotFound(name.to_string()))
        }
    }
}

/// The active tool registry type.
///
/// When the `security` feature is enabled, this aliases to
/// [`GuardedToolRegistry`](crate::security::registry::GuardedToolRegistry)
/// which enforces tool policies and logs audit events on every tool call.
/// Without `security`, it is the raw registry with no policy checks.
#[cfg(feature = "security")]
pub use crate::security::registry::GuardedToolRegistry as ToolRegistry;

#[cfg(not(feature = "security"))]
pub use RawToolRegistry as ToolRegistry;
