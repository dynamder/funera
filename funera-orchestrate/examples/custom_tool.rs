//! Custom tool implementation with callbacks.
//!
//! ```bash
//! cargo run --example custom_tool
//! ```
//!
//! Demonstrates:
//! - Implementing the `Tool` trait
//! - Registering custom tools on the runtime
//! - Using `on_tool_call` / `on_tool_result` callbacks

use async_trait::async_trait;
use funera_core::re_act::tool::{Tool, ToolCallError};
use funera_orchestrate::{Agent, AgentRuntime, DeepSeekProvider};
use serde_json::Value as JsonValue;

// ── A simple weather tool ─────────────────────────────────────────

#[derive(Default)]
struct WeatherTool;

#[async_trait]
impl Tool for WeatherTool {
    fn name(&self) -> &str {
        "get_weather"
    }

    fn description(&self) -> &str {
        "Get the current weather for a city"
    }

    fn schema(&self) -> JsonValue {
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "get_weather",
                "description": "Get the current weather for a city",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "city": {
                            "type": "string",
                            "description": "City name, e.g. 'Beijing'"
                        }
                    },
                    "required": ["city"]
                }
            }
        })
    }

    async fn execute(&self, args: JsonValue) -> Result<String, ToolCallError> {
        let city = args["city"].as_str().unwrap_or("unknown");
        // Simulate weather lookup
        Ok(format!("The weather in {city} is 22°C and sunny."))
    }
}

// ── Main ─────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut runtime = AgentRuntime::<DeepSeekProvider>::builder()
        .api_key(std::env::var("OPENAI_API_KEY")?)
        .base_url(std::env::var("OPENAI_BASE_URL").ok())
        .model(std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o".into()))
        .with_tool::<WeatherTool>()
        .build()?;

    let agent = Agent::builder()
        .system_prompt("You are helpful. Use tools when needed.")
        .on_tool_call(|name, args| {
            eprintln!("[tool call] {name}({args})");
        })
        .on_tool_result(|name, result| match result {
            Ok(r) => eprintln!("[tool result] {name} => {r}"),
            Err(e) => eprintln!("[tool error] {name} => {e}"),
        })
        .build();

    let resp = agent
        .send("What's the weather in Tokyo?", &mut runtime)
        .await?;
    println!("\n{}", resp.content);

    Ok(())
}
