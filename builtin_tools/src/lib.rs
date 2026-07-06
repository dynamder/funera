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
