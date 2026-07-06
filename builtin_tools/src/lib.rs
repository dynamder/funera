pub mod edit;
pub mod hashline;
pub mod read;
pub mod shell;
pub mod write;

pub use edit::EditTool;
pub use read::ReadTool;
pub use shell::ShellTool;
pub use write::WriteTool;

use funera_core::re_act::tool::ToolRegistry;

pub fn register_all_tools(registry: &mut ToolRegistry) {
    registry.add_tool(Box::new(ReadTool));
    registry.add_tool(Box::new(WriteTool));
    registry.add_tool(Box::new(EditTool));
    registry.add_tool(Box::new(ShellTool));
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
