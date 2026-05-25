use std::{collections::HashMap, fmt::Display};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ToolType {
    Function,
}
impl Display for ToolType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "function")
    }
}

pub trait Tool {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn execute(&self, args: &HashMap<String, String>) -> Result<String, ToolExecError>;
    fn get_type(&self) -> ToolType {
        ToolType::Function
    }
    fn schema(&self) -> JsonValue;
}

#[derive(Debug, Error)]
pub enum ToolExecError {
    #[error("parameter mismatch: {mismatched:?}")]
    ParameterMismatch {
        mismatched: HashMap<String, String>, //mismatched parameter name, required value
    },
    #[error("tool execution error: {0}")]
    ToolExecutionError(#[from] anyhow::Error),
}

pub struct ToolRegistryEntry {
    pub tool: Box<dyn Tool>,
    pub available: bool,
}
impl ToolRegistryEntry {
    pub fn new(tool: Box<dyn Tool>, available: bool) -> Self {
        Self { tool, available }
    }
    pub fn is_available(&self) -> bool {
        self.available
    }
    pub fn new_available(tool: Box<dyn Tool>) -> Self {
        Self::new(tool, true)
    }
    pub fn new_unavailable(tool: Box<dyn Tool>) -> Self {
        Self::new(tool, false)
    }
}

pub struct ToolRegistry {
    tools: HashMap<String, ToolRegistryEntry>,
}

impl ToolRegistry {
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
}
