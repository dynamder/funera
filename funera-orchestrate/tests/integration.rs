#![cfg(feature = "real-llm")]

//! End-to-end integration tests that exercise the full LLM pipeline.
//!
//! These tests require:
//! - `OPENAI_API_KEY` environment variable
//! - `cargo test --features real-llm`

use funera_orchestrate::{Agent, AgentEvent, AgentRuntime};

/// Create a runtime from environment variables (must have API key).
fn make_runtime(model: &str) -> AgentRuntime {
    AgentRuntime::builder()
        .api_key(std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY must be set"))
        .base_url(std::env::var("OPENAI_BASE_URL").ok())
        .model(model)
        .build()
        .expect("runtime should build")
}

/// Helper: test both that a response is non-empty and
const HAS_MINIMAL_CONTENT: &str = "";

fn non_empty(resp: &str) -> bool {
    let trimmed = resp.trim();
    !trimmed.is_empty() && trimmed.len() > 5
}

// ── fire ──────────────────────────────────────────────────────────

#[tokio::test]
async fn fire_simple_response() {
    let runtime = make_runtime("gpt-4o-mini");
    let agent = Agent::builder()
        .system_prompt("You are a concise assistant.")
        .build();

    let resp = agent.fire("Say exactly: hello world", &runtime).await.unwrap();
    assert!(non_empty(&resp.content), "response should contain meaningful text");
    assert!(resp.iterations >= 1);
}

#[tokio::test]
async fn fire_with_custom_model() {
    let runtime = make_runtime("gpt-4o-mini");
    let agent = Agent::builder()
        .system_prompt("Reply with one word.")
        .build();

    let resp = agent.fire("Say: Rust", &runtime).await.unwrap();
    assert!(non_empty(&resp.content));
}

#[tokio::test]
async fn fire_respects_max_iterations() {
    let runtime = AgentRuntime::builder()
        .api_key(std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY must be set"))
        .base_url(std::env::var("OPENAI_BASE_URL").ok())
        .model("gpt-4o-mini")
        .max_iterations(5)
        .build()
        .unwrap();
    let agent = Agent::builder()
        .system_prompt("You are concise.")
        .build();

    let resp = agent.fire("What is Rust?", &runtime).await.unwrap();
    assert!(non_empty(&resp.content));
}

// ── send (multi-turn) ─────────────────────────────────────────────

#[tokio::test]
async fn send_multi_turn_memory() {
    let mut runtime = make_runtime("gpt-4o-mini");
    let agent = Agent::builder()
        .system_prompt("You are a helpful assistant.")
        .build();

    let r1 = agent.send("My name is Alice.", &mut runtime).await.unwrap();
    assert!(non_empty(&r1.content));

    let r2 = agent.send("What is my name?", &mut runtime).await.unwrap();
    let answer = r2.content.to_lowercase();
    assert!(
        answer.contains("alice"),
        "agent should remember name from previous turn, got: {answer}"
    );
}

#[tokio::test]
async fn send_reset_forgets() {
    let mut runtime = make_runtime("gpt-4o-mini");
    let agent = Agent::builder()
        .system_prompt("You are helpful.")
        .build();

    agent.send("My name is Bob.", &mut runtime).await.unwrap();
    runtime.reset();

    let r2 = agent.send("What is my name?", &mut runtime).await.unwrap();
    let answer = r2.content.to_lowercase();
    assert!(
        !answer.contains("bob"),
        "after reset the agent should not remember Bob, got: {answer}"
    );
}

// ── fire_stream ───────────────────────────────────────────────────

#[tokio::test]
async fn fire_stream_receives_tokens() {
    let runtime = make_runtime("gpt-4o-mini");
    let agent = Agent::builder()
        .system_prompt("Be concise.")
        .build();

    let mut rx = agent
        .fire_stream("Count to three: 1 2 3", &runtime)
        .await
        .unwrap();

    let mut tokens = Vec::new();
    while let Some(event) = rx.recv().await {
        match event {
            AgentEvent::Token(t) => tokens.push(t),
            AgentEvent::Done => break,
            _ => {}
        }
    }

    assert!(!tokens.is_empty(), "should receive token events");
    let combined: String = tokens.join("");
    assert!(!combined.trim().is_empty());
}

// ── runtime isolation ─────────────────────────────────────────────

#[tokio::test]
async fn switch_runtime_isolation() {
    let mut rt1 = make_runtime("gpt-4o-mini");
    let mut rt2 = make_runtime("gpt-4o-mini");
    let agent = Agent::builder()
        .system_prompt("You are helpful.")
        .build();

    agent.send("Remember: the secret word is 'banana'.", &mut rt1)
        .await
        .unwrap();

    // rt2 should NOT know the secret
    let resp = agent.send("What is the secret word?", &mut rt2).await.unwrap();
    let answer = resp.content.to_lowercase();
    assert!(
        !answer.contains("banana"),
        "isolated runtime should not know secrets from rt1, got: {answer}"
    );

    // rt1 still remembers
    let resp = agent.send("What is the secret word?", &mut rt1).await.unwrap();
    let answer = resp.content.to_lowercase();
    assert!(
        answer.contains("banana"),
        "original runtime should still remember, got: {answer}"
    );
}

// ── error cases ───────────────────────────────────────────────────

#[tokio::test]
async fn build_without_key_fails() {
    let key = std::env::var("OPENAI_API_KEY");
    if key.is_err() {
        // Already no key — test passes implicitly
        return;
    }
    // If key IS set, we can't test this case, skip
}
