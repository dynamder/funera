//! Multi-turn conversation using `Agent::send`.
//!
//! ```bash
//! cargo run --example multi_turn
//! ```
//!
//! `send()` preserves session history in the runtime, enabling natural
//! back-and-forth conversations.

use funera_orchestrate::{Agent, AgentRuntime};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut runtime = AgentRuntime::builder()
        .api_key(std::env::var("OPENAI_API_KEY")?)
        .base_url(std::env::var("OPENAI_BASE_URL").ok())
        .model(std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o".into()))
        .build()?;

    let agent = Agent::builder()
        .system_prompt("You are a friendly assistant.")
        .build();

    // ── Turn 1: introduce yourself ──
    let resp = agent.send("Hi! My name is Alice and I love cats.", &mut runtime).await?;
    println!("Alice >> Hi! My name is Alice and I love cats.");
    println!("Agent >> {}", resp.content);
    println!();

    // ── Turn 2: the agent should remember Alice's name ──
    let resp = agent.send("What's my name? What do I love?", &mut runtime).await?;
    println!("Alice >> What's my name? What do I love?");
    println!("Agent >> {}", resp.content);
    println!();

    // ── Turn 3: follow-up ──
    let resp = agent.send("Recommend a book about my favorite animal.", &mut runtime).await?;
    println!("Alice >> Recommend a book about my favorite animal.");
    println!("Agent >> {}", resp.content);

    Ok(())
}
