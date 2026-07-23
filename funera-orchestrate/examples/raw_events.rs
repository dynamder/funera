//! Raw underlying event access — subscribe to `TokenEvent`, `ReactEvent`,
//! and `EnvStateEvent` directly.
//!
//! ```bash
//! cargo run --example raw_events
//! ```
//!
//! Unlike the standard `fire_stream()` which returns curated `AgentEvent`
//! items, `subscribe_raw_events()` delivers the original core events including
//! `TokenEvent::ToolDelta`, `ReactEvent::MessageQueued`, and all
//! `EnvStateEvent` variants.  Additionally, `subscribe_env_state()` provides
//! runtime-level events (tool/skill registration, etc.) that persist across
//! agent calls.
//!
//! This example prints both streams side-by-side for comparison.

use async_trait::async_trait;
use funera_core::re_act::tool::{Tool, ToolCallError};
use funera_orchestrate::{
    Agent, AgentEvent, AgentRuntime, DeepSeekProvider, EnvStateEvent, RawAgentEvent, ReactEvent,
    TokenEvent,
};
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
        .model(std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "deepseek-v4-flash".into()))
        .with_tool::<WeatherTool>()
        .build()?;

    // ── Runtime-level env state events ──────────────────────────────
    // subscribe_env_state() captures events emitted during build()
    // (ToolAdded, SkillAdded) and any future runtime changes.
    let mut env_rx = runtime.subscribe_env_state().await;

    eprintln!("── Runtime env state events ──");
    loop {
        match env_rx.try_recv() {
            Ok(EnvStateEvent::ToolAdded(name)) => eprintln!("  tool added: {name}"),
            #[cfg(feature = "skill")]
            Ok(EnvStateEvent::SkillAdded(name)) => eprintln!("  skill added: {name}"),
            Ok(_) => {}
            Err(_) => break,
        }
    }

    let agent = Agent::builder()
        .system_prompt("You are a concise assistant with access to weather data.")
        .build();

    // ── Subscribe to raw events BEFORE fire ─────────────────────────
    let mut raw_rx = agent.subscribe_raw_events();
    let mut stream = agent
        .fire_stream("What's the weather in Tokyo?", &runtime)
        .await?;

    // Spawn a task that prints raw events as they arrive
    let raw = tokio::spawn(async move {
        eprintln!("\n── Raw events (subscribe_raw_events) ──");
        loop {
            match raw_rx.recv().await {
                Ok(RawAgentEvent::Token(TokenEvent::Text(t))) => {
                    eprintln!("  [token] {t}");
                }
                Ok(RawAgentEvent::Token(TokenEvent::Reasoning(r))) => {
                    eprintln!("  [reasoning] {r}");
                }
                Ok(RawAgentEvent::Token(TokenEvent::ToolDelta {
                    name, args_chunk, ..
                })) => {
                    let args = args_chunk.unwrap_or_default();
                    eprintln!("  [tool delta] {name:?} args={args:?}");
                }
                Ok(RawAgentEvent::Token(TokenEvent::Finish(reason))) => {
                    eprintln!("  [finish] {reason:?}");
                }
                Ok(RawAgentEvent::React(ReactEvent::TurnStart)) => {
                    eprintln!("  [react] turn start");
                }
                Ok(RawAgentEvent::React(ReactEvent::TurnEnd)) => {
                    eprintln!("  [react] turn end");
                }
                Ok(RawAgentEvent::React(ReactEvent::MessageQueued(msg))) => {
                    eprintln!("  [react] message queued: {:?}", msg.role());
                }
                Ok(RawAgentEvent::React(ReactEvent::ToolExecRequest(req))) => {
                    eprintln!(
                        "  [react] tool exec request: {} args={}",
                        req.name, req.args
                    );
                }
                Ok(RawAgentEvent::React(ReactEvent::ToolExecResponse(res))) => match res {
                    Ok(r) => eprintln!("  [react] tool exec ok: {} => {}", r.name, r.result),
                    Err(e) => eprintln!("  [react] tool exec err: {} => {}", e.name, e.error),
                },
                Ok(RawAgentEvent::EnvState(EnvStateEvent::SessionStart)) => {
                    eprintln!("  [env] session start");
                }
                Ok(RawAgentEvent::EnvState(EnvStateEvent::SessionClosed)) => {
                    eprintln!("  [env] session closed");
                    break;
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
        eprintln!("── raw stream ended ──");
    });

    // ── Standard AgentEvent stream (curated view) ───────────────────
    println!("\n── Agent stream (fire_stream) ──\n");
    while let Some(event) = stream.recv().await {
        match event {
            AgentEvent::Text(t) => print!("{t}"),
            AgentEvent::ToolCallRequest { name, args, .. } => {
                eprintln!("\n  [tool] {name}({args})");
            }
            AgentEvent::ToolCallResult { name, result, .. } => match result {
                Ok(r) => eprintln!("  [tool] {name} => {r}"),
                Err(e) => eprintln!("  [tool] {name} ❌ {e}"),
            },
            AgentEvent::Done => {
                println!();
                break;
            }
            _ => {}
        }
    }

    let _ = raw.await;
    Ok(())
}
