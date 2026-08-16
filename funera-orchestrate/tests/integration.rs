#![cfg(feature = "real-llm")]

//! End-to-end integration tests that exercise the full LLM pipeline.
//!
//! These tests require:
//! - `OPENAI_API_KEY` environment variable
//! - `cargo test --features real-llm`

use funera_orchestrate::{Agent, AgentEvent, AgentRuntime, DeepSeekProvider};

/// Create a runtime from environment variables (must have API key).
fn make_runtime(model: &str) -> AgentRuntime<DeepSeekProvider> {
    AgentRuntime::<DeepSeekProvider>::builder()
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

    let resp = agent
        .fire("Say exactly: hello world", &runtime)
        .await
        .unwrap();
    assert!(
        non_empty(&resp.content),
        "response should contain meaningful text"
    );
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
    let runtime = AgentRuntime::<DeepSeekProvider>::builder()
        .api_key(std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY must be set"))
        .base_url(std::env::var("OPENAI_BASE_URL").ok())
        .model("gpt-4o-mini")
        .max_iterations(5)
        .build()
        .unwrap();
    let agent = Agent::builder().system_prompt("You are concise.").build();

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

    let (_runtime, r1) = agent
        .send("My name is Alice.", runtime)
        .await
        .unwrap()
        .await
        .unwrap();
    assert!(non_empty(&r1.content));

    let (_runtime, r2) = agent
        .send("What is my name?", _runtime)
        .await
        .unwrap()
        .await
        .unwrap();
    let answer = r2.content.to_lowercase();
    assert!(
        answer.contains("alice"),
        "agent should remember name from previous turn, got: {answer}"
    );
}

#[tokio::test]
async fn send_reset_forgets() {
    let mut runtime = make_runtime("gpt-4o-mini");
    let agent = Agent::builder().system_prompt("You are helpful.").build();

    let (mut runtime, _) = agent
        .send("My name is Bob.", runtime)
        .await
        .unwrap()
        .await
        .unwrap();
    runtime.reset();

    let (_runtime, r2) = agent
        .send("What is my name?", runtime)
        .await
        .unwrap()
        .await
        .unwrap();
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
    let agent = Agent::builder().system_prompt("Be concise.").build();

    let mut rx = agent
        .fire_stream("Count to three: 1 2 3", &runtime)
        .await
        .unwrap();

    let mut tokens = Vec::new();
    while let Some(event) = rx.recv().await {
        match event {
            AgentEvent::Text(t) => tokens.push(t),
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
    let rt1 = make_runtime("gpt-4o-mini");
    let rt2 = make_runtime("gpt-4o-mini");
    let agent = Agent::builder().system_prompt("You are helpful.").build();

    let (mut rt1, _) = agent
        .send("Remember: the secret word is 'banana'.", rt1)
        .await
        .unwrap()
        .await
        .unwrap();

    // rt2 should NOT know the secret
    let (mut rt2, resp) = agent
        .send("What is the secret word?", rt2)
        .await
        .unwrap()
        .await
        .unwrap();
    let answer = resp.content.to_lowercase();
    assert!(
        !answer.contains("banana"),
        "isolated runtime should not know secrets from rt1, got: {answer}"
    );

    // rt1 still remembers
    let (_rt1, resp) = agent
        .send("What is the secret word?", rt1)
        .await
        .unwrap()
        .await
        .unwrap();
    let answer = resp.content.to_lowercase();
    assert!(
        answer.contains("banana"),
        "original runtime should still remember, got: {answer}"
    );
}

// ── error cases ───────────────────────────────────────────────────

#[tokio::test]
async fn build_without_key_fails() {
    let original_key = std::env::var("OPENAI_API_KEY").ok();
    unsafe { std::env::remove_var("OPENAI_API_KEY") };

    let result = AgentRuntime::<DeepSeekProvider>::builder()
        .model("gpt-4o-mini")
        .build();

    assert!(result.is_err(), "building without API key should fail");

    // Restore original key if it was set
    if let Some(key) = original_key {
        unsafe { std::env::set_var("OPENAI_API_KEY", key) };
    }
}
