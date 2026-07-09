//! Switching between multiple runtime contexts.
//!
//! ```bash
//! cargo run --example multi_runtime
//! ```
//!
//! Each `AgentRuntime` maintains its own independent session.  You can
//! `fire` or `send` messages through different runtimes with the same
//! `Agent`, making it easy to switch LLM providers or tool sets.

use funera_orchestrate::{Agent, AgentRuntime, DeepSeekProvider};

/// Convenience to create a runtime from env vars.
fn make_runtime(model: &str) -> Result<AgentRuntime<DeepSeekProvider>, Box<dyn std::error::Error>> {
    Ok(AgentRuntime::<DeepSeekProvider>::builder()
        .api_key(std::env::var("OPENAI_API_KEY")?)
        .base_url(std::env::var("OPENAI_BASE_URL").ok())
        .model(model)
        .build()?)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Two runtimes, different models
    let mut gpt4 = make_runtime("gpt-4o")?;
    let mut gpt35 = make_runtime("gpt-4o-mini")?;

    let agent = Agent::builder()
        .system_prompt("You are a helpful assistant.")
        .build();

    // ── Talk to gpt-4o ──
    agent.send("I need a detailed explanation of monads.", &mut gpt4).await?;
    agent.send("Can you give me a code example?", &mut gpt4).await?;
    println!("(gpt-4o session has 2 messages)");

    // ── Talk to gpt-4o-mini on the side ──
    // This is a completely independent conversation
    agent.send("Write a haiku about Rust.", &mut gpt35).await?;
    println!("(gpt-4o-mini session has 1 message)");

    // ── Continue gpt-4o conversation ──
    let resp = agent.send("Now explain functors too.", &mut gpt4).await?;
    println!("\n=== gpt-4o response ===");
    println!("{}", resp.content);

    // ── Each runtime's session is independent ──
    println!("\n(gpt-4o session has 3 messages, gpt-4o-mini has 1)");
    println!("Runtimes are fully isolated.");

    Ok(())
}
