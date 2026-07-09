//! Streaming with tool calls — real-time token output and tool progress.
//!
//! ```bash
//! cargo run --example streaming_with_tools
//! ```
//!
//! Unlike `fire()` which waits for the entire response, `fire_stream()`
//! returns events as they happen — tokens print character-by-character,
//! tool calls show start/result inline, and turn boundaries are visible.

use async_trait::async_trait;
use funera_core::re_act::tool::{Tool, ToolCallError};
use funera_orchestrate::{Agent, AgentEvent, AgentRuntime, DeepSeekProvider};
use serde_json::Value as JsonValue;

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
                        "city": { "type": "string", "description": "City name" }
                    },
                    "required": ["city"]
                }
            }
        })
    }
    async fn execute(&self, args: JsonValue) -> Result<String, ToolCallError> {
        let city = args["city"].as_str().unwrap_or("unknown");
        Ok(format!("{city}: 22°C, sunny"))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = AgentRuntime::<DeepSeekProvider>::builder()
        .api_key(std::env::var("OPENAI_API_KEY")?)
        .base_url(std::env::var("OPENAI_BASE_URL").ok())
        .model(std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o".into()))
        .with_tool::<WeatherTool>()
        .build()?;

    let agent = Agent::builder()
        .system_prompt("You are helpful. Use get_weather when asked about weather.")
        .build();

    println!("=== Streaming with tools ===\n");

    let mut rx = agent
        .fire_stream("What's the weather in Tokyo, Beijing, and Paris?", &runtime)
        .await?;

    while let Some(event) = rx.recv().await {
        match event {
            AgentEvent::Token(t) => print!("{t}"),
            AgentEvent::ToolCallRequest {
                call_id,
                name,
                args,
                ..
            } => {
                eprintln!("  🔧 [{call_id}] {name}({args}) ...");
            }
            AgentEvent::ToolCallResult { name, result, .. } => match result {
                Ok(r) => eprintln!("  {name} => {r}"),
                Err(e) => eprintln!("  {name} ❌ {e}"),
            },
            AgentEvent::TurnStart => eprintln!("\n── Turn ──"),
            AgentEvent::TurnEnd => eprintln!(),
            AgentEvent::Done => eprintln!("\n── Done ──"),
            _ => {}
        }
    }

    Ok(())
}
