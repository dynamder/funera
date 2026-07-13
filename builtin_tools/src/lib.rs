//! # builtin_tools
//!
//! Default tool implementations for the funera agent framework.
//!
//! Provides four built-in tools that implement the funera_core `Tool` trait:
//!
//! | Tool | Description |
//! |------|-------------|
//! | [`ReadTool`] | Read files and directories with hashline-anchored output |
//! | [`WriteTool`] | Write content to files, auto-creating parent directories |
//! | [`EditTool`] | Edit files using hashline-anchored replace/append/prepend operations |
//! | [`ShellTool`] | Execute shell commands cross-platform with timeout |
//!
//! ## Quick start
//!
//! ```rust,ignore
//! use builtin_tools::register_all_tools;
//! use funera_core::re_act::tool::ToolRegistry;
//!
//! let mut registry = ToolRegistry::new();
//! register_all_tools(&mut registry);
//! ```

pub mod edit;
pub mod hashline;
pub mod read;
pub mod shell;
pub mod write;

pub use edit::EditTool;
pub use read::ReadTool;
pub use shell::ShellTool;
pub use write::WriteTool;

#[cfg(feature = "sandbox")]
pub use funera_core::security::sandbox::SandboxPolicy;

use funera_core::re_act::tool::ToolRegistry;

/// Register all four built-in tools (read, write, edit, shell) in the given registry.
///
/// The `shell` tool is registered **without** kernel sandboxing.
/// Use [`register_all_tools_with_sandbox`] to enable it.
pub fn register_all_tools(registry: &mut ToolRegistry) {
    registry.add_tool(Box::new(ReadTool));
    registry.add_tool(Box::new(WriteTool));
    registry.add_tool(Box::new(EditTool));
    registry.add_tool(Box::new(ShellTool::new()));
}

/// Register all built-in tools, configuring the `shell` tool with a
/// [`SandboxPolicy`] for kernel-enforced isolation.
///
/// | Platform | Mechanism |
/// |----------|-----------|
/// | Linux    | Landlock (nono crate) |
/// | macOS    | Seatbelt (nono crate) |
/// | Windows  | Write-Restricted Token + ACLs |
///
/// Unsupported platforms/kernels gracefully degrade to normal execution.
#[cfg(feature = "sandbox")]
pub fn register_all_tools_with_sandbox(
    registry: &mut ToolRegistry,
    policy: SandboxPolicy,
) {
    registry.add_tool(Box::new(ReadTool));
    registry.add_tool(Box::new(WriteTool));
    registry.add_tool(Box::new(EditTool));
    registry.add_tool(Box::new(ShellTool::with_sandbox(policy)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_all_tools_adds_four_tools() {
        let mut registry = ToolRegistry::new();
        register_all_tools(&mut registry);
        assert_eq!(registry.tool_count(), 4);
        assert!(registry.tool_exists("read"));
        assert!(registry.tool_exists("write"));
        assert!(registry.tool_exists("edit"));
        assert!(registry.tool_exists("shell"));
    }

    #[test]
    fn each_tool_has_valid_schema() {
        let mut registry = ToolRegistry::new();
        register_all_tools(&mut registry);
        let json = registry.available_tools_json();
        let tools = json.as_array().unwrap();
        assert_eq!(tools.len(), 4);
        for tool in tools {
            assert!(tool["function"]["name"].as_str().is_some());
            assert!(tool["function"]["parameters"].is_object());
        }
    }
}
