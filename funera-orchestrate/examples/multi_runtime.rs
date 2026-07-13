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
    let agent = Agent::builder()
        .system_prompt("You are a helpful assistant.")
        .build();

    // Two runtimes, different models
    let gpt4 = make_runtime("deepseek-v4-pro")?;
    let gpt35 = make_runtime("deepseek-v4-flash")?;

    // ── Talk to gpt-4o ──
    let (gpt4, _) = agent
        .send("I need a detailed explanation of monads.", gpt4)
        .await?
        .await?;
    let (gpt4, _) = agent
        .send("Can you give me a code example?", gpt4)
        .await?
        .await?;
    println!("(gpt-4o session has 2 messages)");

    // ── Talk to gpt-4o-mini on the side ──
    let (_gpt35, _) = agent
        .send("Write a haiku about Rust.", gpt35)
        .await?
        .await?;
    println!("(gpt-4o-mini session has 1 message)");

    // ── Continue gpt-4o conversation ──
    let (_, resp) = agent.send("Now explain functors too.", gpt4).await?.await?;
    println!("\n=== gpt-4o response ===");
    println!("{}", resp.content);

    println!("\n(gpt-4o session has 3 messages, gpt-4o-mini has 1)");
    println!("Runtimes are fully isolated.");

    Ok(())
}
