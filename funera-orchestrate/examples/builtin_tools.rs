//! Using built-in file/shell tools.
//!
//! ```bash
//! cargo run --example builtin_tools --features funera-builtin-tools
//! ```
//!
//! The `funera-builtin-tools` feature bundles Read, Write, Edit, and Shell tools
//! that the agent can use to interact with the filesystem and shell.

use funera_orchestrate::{Agent, AgentRuntime, DeepSeekProvider};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = AgentRuntime::<DeepSeekProvider>::builder()
        .api_key(std::env::var("OPENAI_API_KEY")?)
        .base_url(std::env::var("OPENAI_BASE_URL").ok())
        .model(std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "deepseek-v4-flash".into()))
        .with_builtin_tools()
        .build()?;

    let agent = Agent::builder()
        .system_prompt("You are a helpful assistant with file access.")
        .on_tool_call(|name, args| eprintln!("[tool] {name} {args}"))
        .build();

    // Ask the agent to read Cargo.toml (it will use the Read tool)
    let resp = agent
        .fire("Read Cargo.toml and tell me the dependencies.", &runtime)
        .await?;
    println!("{}", resp.content);

    Ok(())
}
