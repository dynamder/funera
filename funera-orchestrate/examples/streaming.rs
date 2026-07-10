//! Token-level streaming with `fire_stream` and `send_stream`.
//!
//! ```bash
//! cargo run --example streaming
//! ```
//!
//! Both `fire_stream` (one-shot) and `send_stream` (multi-turn) return an
//! `mpsc::Receiver<AgentEvent>` that yields tokens, tool calls, and turn
//! boundary events in real time.

use funera_orchestrate::{Agent, AgentEvent, AgentRuntime, DeepSeekProvider};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = AgentRuntime::<DeepSeekProvider>::builder()
        .api_key(std::env::var("OPENAI_API_KEY")?)
        .base_url(std::env::var("OPENAI_BASE_URL").ok())
        .model(std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o".into()))
        .build()?;

    let agent = Agent::builder()
        .system_prompt("You are a concise tutor.")
        .build();

    // ── One-shot with streaming ──
    println!("=== Streaming: What is Rust? ===\n");
    let mut rx = agent
        .fire_stream("What is Rust's ownership model in 3 sentences?", &runtime)
        .await?;

    while let Some(event) = rx.recv().await {
        match event {
            AgentEvent::Text(t) => eprint!("{t}"),
            AgentEvent::TurnStart => eprintln!("\n[Turn Start]"),
            AgentEvent::TurnEnd { .. } => eprintln!("\n[Turn End]"),
            AgentEvent::Done => eprintln!("\n[Done]"),
            _ => {}
        }
    }
    println!();

    Ok(())
}
