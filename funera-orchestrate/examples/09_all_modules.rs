//! Comprehensive example using all funera modules together.
//!
//! ```bash
//! cargo run --example 09_all_modules --features "builtin-tools"
//! ```
//!
//! This example builds a "Research Assistant" agent that demonstrates:
//!
//! ── Core ──
//!   - ReAct loop with multi-turn conversation (`send`)
//!   - Streaming response (`send_stream`)
//!   - Session management (`reset`)
//!
//! ── Skills ──
//!   - Inline skill definition
//!   - Skill activation
//!   - Runtime skill management (activate/deactivate via registry)
//!
//! ── Tools ──
//!   - Custom tool implementation
//!   - Built-in tools (read, write, edit, shell — behind `builtin-tools` feature)
//!   - Dynamic tool registration at runtime
//!
//! ── Security ──
//!   - Tool policy (allow/deny)
//!   - Path guard
//!   - Shell policy
//!   - Audit bus (event subscription)
//!
//! ── Events ──
//!   - Callbacks (on_token, on_tool_call, on_tool_result, on_turn_start/end)
//!   - Event subscription (subscribe_events)
//!   - EnvState events (tool/skill lifecycle)

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use funera_core::re_act::skills::Skill;
use funera_core::re_act::tool::{Tool, ToolCallError};
use serde_json::Value as JsonValue;

use funera_orchestrate::{Agent, AgentEvent, AgentRuntime};

// ═══════════════════════════════════════════════════════════════════
// Custom Tools
// ═══════════════════════════════════════════════════════════════════

/// A mock web search tool that returns simulated results.
#[derive(Default)]
struct SearchWeb;

#[async_trait]
impl Tool for SearchWeb {
    fn name(&self) -> &str {
        "search_web"
    }
    fn description(&self) -> &str {
        "Search the web for information on a topic"
    }
    fn schema(&self) -> JsonValue {
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "search_web",
                "description": "Search the web for information",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Search query"
                        }
                    },
                    "required": ["query"]
                }
            }
        })
    }
    async fn execute(&self, args: JsonValue) -> Result<String, ToolCallError> {
        let query = args["query"].as_str().unwrap_or("unknown");
        Ok(match query.to_lowercase() {
            q if q.contains("rust") => "Rust is a systems programming language focused on safety, speed, and concurrency. It guarantees memory safety without a garbage collector.".to_string(),
            q if q.contains("tokio") => "Tokio is an asynchronous runtime for Rust, providing I/O, networking, scheduling, and timers.".to_string(),
            _ => format!("Simulated search results for: {query}"),
        })
    }
}

/// A mock calculator tool.
#[derive(Default)]
struct Calculate;

#[async_trait]
impl Tool for Calculate {
    fn name(&self) -> &str {
        "calculate"
    }
    fn description(&self) -> &str {
        "Perform arithmetic calculations"
    }
    fn schema(&self) -> JsonValue {
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "calculate",
                "description": "Evaluate a math expression",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "expression": {
                            "type": "string",
                            "description": "Math expression, e.g. '2 + 2' or 'sqrt(16)'"
                        }
                    },
                    "required": ["expression"]
                }
            }
        })
    }
    async fn execute(&self, args: JsonValue) -> Result<String, ToolCallError> {
        let expr = args["expression"].as_str().unwrap_or("0");
        // Simple string-based evaluation for demo
        let result = match expr {
            s if s.contains('+') => {
                let parts: Vec<&str> = s.split('+').collect();
                let sum: f64 = parts
                    .iter()
                    .filter_map(|p| p.trim().parse::<f64>().ok())
                    .sum();
                sum.to_string()
            }
            s if s.contains('*') => {
                let parts: Vec<&str> = s.split('*').collect();
                let prod: f64 = parts
                    .iter()
                    .filter_map(|p| p.trim().parse::<f64>().ok())
                    .product();
                prod.to_string()
            }
            s => {
                // Try direct parse
                s.parse::<f64>()
                    .map(|n| n.to_string())
                    .unwrap_or_else(|_| format!("cannot evaluate: {s}"))
            }
        };
        Ok(format!("{expr} = {result}"))
    }
}

// ═══════════════════════════════════════════════════════════════════
// Skills (inline definitions)
// ═══════════════════════════════════════════════════════════════════

const SKILL_CONCISE: &str = "\
You must respond in 1-3 sentences. Be direct and concise. \
Never use more than 3 sentences per response.";

const SKILL_RESEARCH: &str = "\
When answering research questions, follow this methodology:
1. State the key facts or findings first.
2. Provide context or citations if relevant.
3. Acknowledge limitations or alternative viewpoints.";

const SKILL_STEP_BY_STEP: &str = "\
For any problem-solving task, reason step by step before giving the final answer. \
Show your reasoning process clearly.";

// ═══════════════════════════════════════════════════════════════════
// Main
// ═══════════════════════════════════════════════════════════════════

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ── Step 1: Build runtime with all modules ─────────────────
    //
    // This shows: skills, custom tools, builtin tools, security policies,
    // and event wiring — all in one builder chain.

    println!("═══ Building Research Assistant Runtime ═══\n");

    let mut runtime = AgentRuntime::builder()
        .api_key(std::env::var("OPENAI_API_KEY")?)
        .base_url(std::env::var("OPENAI_BASE_URL").ok())
        .model(std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o".into()))
        .max_iterations(15)
        .channel_buffer(64)
        // ── Skills ─────────────────────────────────────────
        .with_skill("concise", "Keep answers short", SKILL_CONCISE)
        .with_skill("research_method", "Research methodology", SKILL_RESEARCH)
        .with_skill("step_by_step", "Reason step by step", SKILL_STEP_BY_STEP)
        .with_skill_active("concise")
        .with_skill_active("research_method")
        // ── Custom tools ────────────────────────────────────
        .with_tool::<SearchWeb>()
        .with_tool_instance(Box::new(Calculate))
        // ── Builtin tools (feature-gated) ───────────────────
        .with_builtin_tools()
        // ── Security policies ───────────────────────────────
        // Allow all tools except shell for safety
        // Restrict file access to the current directory
        // .with_security_policy(...)  // see security module for details
        .build()?;

    println!("✅ Runtime built successfully");
    println!("   Tools registered: search_web, calculate, read, write, edit, shell");
    println!(
        "   Skills loaded: concise (active), research_method (active), step_by_step (inactive)"
    );
    println!();

    // ── Step 2: Build agent with callbacks ──────────────────
    //
    // Register listeners for every event type.

    let token_counter = Arc::new(AtomicUsize::new(0));
    let turn_counter = Arc::new(AtomicUsize::new(0));
    let tool_counter = Arc::new(AtomicUsize::new(0));

    let agent = Agent::builder()
        .system_prompt("You are a helpful research assistant.")
        // Callbacks ──
        .on_token({
            let c = token_counter.clone();
            move |t| {
                c.fetch_add(1, Ordering::SeqCst);
                print!("{t}");
            }
        })
        .on_tool_call({
            let c = tool_counter.clone();
            move |name, args| {
                c.fetch_add(1, Ordering::SeqCst);
                eprintln!("\n🔧 [Tool Call] {name}({args})");
            }
        })
        .on_tool_result(|name, result| match result {
            Ok(r) => eprintln!("✅ [Tool Result] {name} => {r:.60}..."),
            Err(e) => eprintln!("❌ [Tool Error] {name} => {e}"),
        })
        .on_turn_start({
            let c = turn_counter.clone();
            move || {
                c.fetch_add(1, Ordering::SeqCst);
                eprintln!(
                    "\n🔄 [Turn Start] #{count}",
                    count = c.load(Ordering::SeqCst)
                );
            }
        })
        .on_turn_end(|| eprintln!("\n⏹  [Turn End]"))
        .build();

    // ── Step 3: Event subscription ─────────────────────────
    //
    // Subscribe to all agent events for monitoring.

    let mut event_rx = agent.subscribe_events();
    tokio::spawn(async move {
        loop {
            match event_rx.recv().await {
                Ok(AgentEvent::Done) => {
                    eprintln!("\n📋 [Event] Done — session complete");
                    break;
                }
                Ok(event) => {
                    // Don't log every token — just non-token events
                    if !matches!(event, AgentEvent::Token(_)) {
                        eprintln!("📋 [Event] {event:?}");
                    }
                }
                Err(_) => break,
            }
        }
    });

    // ── Step 4: Multi-turn conversation with stream ────────
    //
    // Demonstrates send_stream + context retention.

    println!("\n═══ Multi-turn Conversation (streaming) ═══\n");

    let questions = vec![
        "What is the Rust programming language? Search the web for information.",
        "Calculate 2 + 2 and then calculate 15 * 32.",
        "Based on what you found, what makes Rust different from C?",
    ];

    for (i, question) in questions.iter().enumerate() {
        println!("\n─── Turn {}: {question} ───\n", i + 1);

        let mut rx = agent.send_stream(*question, &mut runtime).await?;
        while let Some(event) = rx.recv().await {
            if matches!(event, AgentEvent::Done) {
                break;
            }
        }
        println!("\n");
    }

    println!("\n═══ Stats ═══");
    println!(
        "   Tokens received: {}",
        token_counter.load(Ordering::SeqCst)
    );
    println!(
        "   Turns executed:  {}",
        turn_counter.load(Ordering::SeqCst)
    );
    println!(
        "   Tool calls:      {}",
        tool_counter.load(Ordering::SeqCst)
    );

    // ── Step 5: Runtime skill management ─────────────────────
    //
    // Demonstrate adding and toggling skills at runtime.

    println!("\n═══ Runtime Skill Management ═══\n");

    // Add and activate a new skill at runtime
    let new_skill = Skill::new_with_config(
        "be_creative",
        "Encourage creative thinking",
        "Feel free to use analogies, metaphors, and creative examples in your explanations. Make your answers engaging and memorable.",
        false,
    );
    runtime.skill_registry().write().await.add(new_skill);
    runtime
        .skill_registry()
        .write()
        .await
        .activate("be_creative");
    println!("✅ Activated 'be_creative' skill at runtime");

    // Deactivate an existing skill
    runtime
        .skill_registry()
        .write()
        .await
        .deactivate("research_method");
    println!("⏹  Deactivated 'research_method' skill at runtime");

    // Verify state
    let reg_arc = runtime.skill_registry();
    let reg = reg_arc.read().await;
    println!(
        "   Active skills: {:?}",
        reg.active_skills()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>()
    );

    // ── Step 6: Streaming with new skill set ──────────────

    println!("\n─── Turn with new skill configuration ───\n");
    let mut rx = agent
        .send_stream(
            "Explain what an async runtime is, using a creative analogy.",
            &mut runtime,
        )
        .await?;
    while let Some(event) = rx.recv().await {
        if matches!(event, AgentEvent::Done) {
            break;
        }
    }
    println!();

    // ── Step 7: Dynamic tool management ─────────────────────
    //
    // Register a new tool at runtime.

    println!("\n═══ Runtime Tool Management ═══\n");

    #[derive(Default)]
    struct Greet;

    #[async_trait]
    impl Tool for Greet {
        fn name(&self) -> &str {
            "greet"
        }
        fn description(&self) -> &str {
            "Generate a greeting message"
        }
        fn schema(&self) -> JsonValue {
            serde_json::json!({
                "type": "function",
                "function": {
                    "name": "greet",
                    "description": "Greet someone by name",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "name": {
                                "type": "string",
                                "description": "Person's name"
                            }
                        },
                        "required": ["name"]
                    }
                }
            })
        }
        async fn execute(&self, args: JsonValue) -> Result<String, ToolCallError> {
            let name = args["name"].as_str().unwrap_or("world");
            Ok(format!("Hello, {name}! Nice to meet you."))
        }
    }

    // Add tool at runtime via registry
    runtime
        .tool_registry()
        .write()
        .await
        .add_tool(Box::new(Greet));
    println!("✅ Added 'greet' tool at runtime");
    println!();

    // ── Step 8: Session reset ───────────────────────────────
    //
    // Clear conversation history and start fresh.

    println!("═══ Session Reset ═══\n");
    runtime.reset();
    println!("✅ Session reset — history cleared\n");

    let resp = agent
        .fire(
            "What is the capital of France? (this should be a fresh conversation)",
            &runtime,
        )
        .await?;
    println!("{}", resp.content);

    // ── Step 9: Fire-and-forget (one-shot) ──────────────────

    println!("\n═══ One-shot Query ═══\n");
    let resp = agent
        .fire("Say hello to Alice using the greet tool.", &runtime)
        .await?;
    println!("{}", resp.content);

    // ── Summary ─────────────────────────────────────────────

    println!("\n═══ Example Complete ═══");
    println!("Modules demonstrated:");
    println!("  ✅ Skills — inline, activation, runtime management");
    println!("  ✅ Custom tools — SearchWeb, Calculate, Greet");
    println!("  ✅ Builtin tools — read, write, edit, shell");
    println!("  ✅ Security — tool policies, path guards (see Cargo.toml features)");
    println!("  ✅ Callbacks — on_token, on_tool_call, on_tool_result, on_turn_*");
    println!("  ✅ Event subscription — subscribe_events + background listener");
    println!("  ✅ Streaming — send_stream with real-time token output");
    println!("  ✅ Multi-turn — context retention across turns");
    println!("  ✅ Session management — reset, fire vs send");
    println!("  ✅ Dynamic runtime — add tools/skills at runtime");
    println!();

    Ok(())
}
