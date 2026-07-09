//! Resetting a runtime's session history.
//!
//! ```bash
//! cargo run --example session_reset
//! ```
//!
//! `AgentRuntime::reset()` clears the persisted session, allowing the
//! runtime to start a fresh conversation without building a new runtime.

use funera_orchestrate::{Agent, AgentRuntime, DeepSeekProvider};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut runtime = AgentRuntime::<DeepSeekProvider>::builder()
        .api_key(std::env::var("OPENAI_API_KEY")?)
        .base_url(std::env::var("OPENAI_BASE_URL").ok())
        .model(std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o".into()))
        .build()?;

    let agent = Agent::builder()
        .system_prompt("You are helpful.")
        .build();

    // Phase 1: chat about cats
    agent.send("I love cats!", &mut runtime).await?;
    agent.send("What's a good cat name?", &mut runtime).await?;
    println!("(session has 2 messages — agent remembers cats)");

    // Reset — the runtime now behaves as if brand-new
    runtime.reset();
    println!("(session reset)");

    // Phase 2: chat about something else
    let resp = agent.send("What's the capital of France?", &mut runtime).await?;
    // The agent does NOT remember cats — session was empty
    println!("Agent >> {}", resp.content);

    Ok(())
}
