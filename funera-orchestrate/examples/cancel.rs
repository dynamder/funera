//! Cancellation — interrupt an agent call mid-flight.
//!
//! ```bash
//! cargo run -p funera-orchestrate --example cancel
//! # with the middleware demo:
//! cargo run -p funera-orchestrate --example cancel --features middleware
//! ```
//!
//! Every funera entry point returns a cancellable handle:
//!
//! - [`Agent::fire`] / [`Agent::fire_stream`] → one-shot call
//! - [`Agent::send`] / [`Agent::send_stream`] → multi-turn call
//!
//! [`cancel()`](FireStreamHandle::cancel) immediately stops receiving output,
//! abandons in-flight tool executions, and emits [`AgentEvent::Cancelled`] to
//! subscribers — and through the middleware chain when the `middleware`
//! feature is enabled — so middleware holding external services can clean up.
//! **Dropping the handle cancels automatically**: no detached background work
//! is left running.
//!
//! Requires an API key (set `OPENAI_API_KEY`, or call `.api_key()`).

use std::time::Duration;

use funera_orchestrate::{Agent, AgentEvent, AgentRuntime, DeepSeekProvider};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg_attr(not(feature = "middleware"), allow(unused_mut))]
    let mut builder = AgentRuntime::<DeepSeekProvider>::builder()
        .api_key(std::env::var("OPENAI_API_KEY")?)
        .model(std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "deepseek-v4-flash".into()));

    // A tiny middleware inspector that logs AgentEvent::Cancelled, so the
    // "cancellation reaches middleware" contract is visible. Requires the
    // `middleware` feature.
    #[cfg(feature = "middleware")]
    {
        use funera_orchestrate::middleware::{InspectorError, InspectorMiddleware};
        use funera_orchestrate::middleware_bundle::MiddlewareBundle;

        struct CancelLogger;
        impl InspectorMiddleware<AgentEvent> for CancelLogger {
            fn name(&self) -> &str {
                "cancel_logger"
            }
            fn inspect(&self, event: &AgentEvent) -> Result<(), InspectorError> {
                if matches!(event, AgentEvent::Cancelled) {
                    eprintln!(
                        "[middleware] saw AgentEvent::Cancelled — clean up external services here"
                    );
                }
                Ok(())
            }
        }

        let bundle = MiddlewareBundle::from_chain(
            funera_orchestrate::middleware::MiddlewareChain::<AgentEvent>::new()
                .with_inspector(CancelLogger),
        );
        builder = builder.with_middleware_bundle(bundle);
    }

    let runtime = builder.build()?;
    let agent = Agent::builder()
        .system_prompt("You are a helpful assistant.")
        .build();

    // ── 1. Explicit cancel() on a streaming call ─────────────────
    let mut stream = agent
        .fire_stream("Write a very long essay about Rust.", &runtime)
        .await?;

    // Subscribe before cancelling so we can observe AgentEvent::Cancelled.
    let mut events = agent.subscribe_events();
    tokio::time::sleep(Duration::from_millis(200)).await;
    stream.cancel();

    // recv() returns None once cancelled.
    assert!(
        stream.recv().await.is_none(),
        "recv should end after cancel"
    );

    // The cancellation event reaches subscribers (and middleware, if enabled).
    loop {
        match events.recv().await {
            Ok(AgentEvent::Cancelled) => {
                println!("[1] explicit cancel: subscriber received AgentEvent::Cancelled");
                break;
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }

    // Awaiting a cancelled handle yields OrchestrateError::Cancelled.
    match stream.await {
        Err(funera_orchestrate::OrchestrateError::Cancelled) => {
            println!("[1] explicit cancel: wait() reported OrchestrateError::Cancelled");
        }
        other => println!("[1] unexpected result: {other:?}"),
    }

    // ── 2. Dropping the handle auto-cancels ─────────────────────
    let handle = agent.fire("Explain the borrow checker.", &runtime).await?;
    // Drop without awaiting: the background call is cancelled, nothing leaks.
    drop(handle);
    println!("[2] dropped handle: background call auto-cancelled");

    // ── 3. Timeout via tokio::select! ───────────────────────────
    let handle = agent.fire("Write a 10-page novel.", &runtime).await?;
    let resp = tokio::select! {
        r = handle => r?,
        _ = tokio::time::sleep(Duration::from_secs(3)) => {
            println!("[3] timeout: the fire future was dropped, call cancelled");
            return Ok(());
        }
    };
    println!("[3] completed before timeout: {} chars", resp.content.len());

    Ok(())
}
