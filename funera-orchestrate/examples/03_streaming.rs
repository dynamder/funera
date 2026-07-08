//! Token-level streaming with `fire_stream` and `send_stream`.
//!
//! ```bash
//! cargo run --example 03_streaming
//! ```
//!
//! Both `fire_stream` (one-shot) and `send_stream` (multi-turn) return an
//! `mpsc::Receiver<AgentEvent>` that yields tokens, tool calls, and turn
//! boundary events in real time.

use funera_orchestrate::{Agent, AgentEvent, AgentRuntime};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = AgentRuntime::builder()
        .api_key(std::env::var("OPENAI_API_KEY")?)
        .model(std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o".into()))
        .build()?;

    let agent = Agent::builder()
        .system_prompt("You are a concise tutor.")
        .build();

    // ── One-shot with streaming ──
    println!("=== Streaming: What is Rust? ===\n");
    let mut rx = agent.fire_stream("What is Rust's ownership model in 3 sentences?", &runtime).await?;

    while let Some(event) = rx.recv().await {
        match event {
            AgentEvent::Token(t) => print!("{t}"),
            AgentEvent::TurnStart => println!("\n[Turn Start]"),
            AgentEvent::TurnEnd => println!("\n[Turn End]"),
            AgentEvent::Done => println!("\n[Done]"),
            _ => {}
        }
    }
    println!();

    Ok(())
}
