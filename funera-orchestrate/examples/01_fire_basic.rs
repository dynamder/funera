//! A minimal one-shot query using `Agent::fire`.
//!
//! ```bash
//! cargo run --example 01_fire_basic
//! ```
//!
//! This demonstrates the simplest possible use:
//! - Build an `AgentRuntime` with a model
//! - Build an `Agent` with a system prompt
//! - Call `fire()` for a single turn (session is discarded after)

use funera_orchestrate::{Agent, AgentRuntime};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Runtime from env vars
    let runtime = AgentRuntime::builder()
        .api_key(std::env::var("OPENAI_API_KEY")?)
        .base_url(std::env::var("OPENAI_BASE_URL").ok())
        .model(std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o".into()))
        .build()?;

    // Agent is pure config — no runtime dependency
    let agent = Agent::builder()
        .system_prompt("You speak like a pirate.")
        .build();

    // fire() shares the runtime (&) — no session state is mutated
    let resp = agent.fire("Tell me about Rust programming.", &runtime).await?;

    println!("=== Response ===");
    println!("{}", resp.content);
    println!();
    println!("Iterations: {}", resp.iterations);
    println!("Finish reason: {:?}", resp.finish_reason);

    Ok(())
}
