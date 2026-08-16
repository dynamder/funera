//! Replace the built-in ReAct loop with a custom AgentLoop.
//!
//! ```bash
//! cargo run --example replace_react_loop
//! ```

use std::sync::Arc;

use async_trait::async_trait;
use funera_core::chat::message::FuneraMessage;
use funera_core::chat::session::FuneraSession;
use funera_core::event_bus::env_state_bus::EnvStateEvent;
use funera_core::event_bus::react_bus::ReactEvent;
use funera_core::event_bus::token_bus::TokenEvent;
use funera_core::middleware::{EventSenderFn, MiddlewareEvent, MiddlewareProcessor};
use funera_core::re_act::ReActLoopConfig;
use funera_orchestrate::{Agent, AgentEvent, AgentLoop, AgentRuntime, DeepSeekProvider};

/// A trivial replacement loop that never calls an LLM.
///
/// It stores the user message, emits a fixed assistant text event through the
/// normal middleware/event pipeline, and signals `Done`.
#[derive(Default)]
struct FixedAgentLoop;

#[async_trait]
impl AgentLoop for FixedAgentLoop {
    async fn run(
        &self,
        session: &FuneraSession,
        init_msg: FuneraMessage,
        mut config: ReActLoopConfig,
        _env_state_tx: tokio::sync::broadcast::Sender<EnvStateEvent>,
        middleware: Option<Arc<dyn MiddlewareProcessor<AgentEvent>>>,
        event_sender: Option<EventSenderFn<AgentEvent>>,
    ) -> anyhow::Result<()> {
        session.push_message(init_msg);

        // 1. Prepare the per-turn buses through the turn highway.
        let (token_tx, react_bus) = config.turn_highway_handle.prepare_turn().await;

        // 2. Publish the token-bus and react-bus lifecycle messages.
        let _ = token_tx.send(TokenEvent::Text(
            "custom-loop: hello from replacement".into(),
        ));
        let _ = react_bus.send(ReactEvent::TurnStart);

        // 3. Run the middleware pipeline and persist the assistant message.
        let mut event = AgentEvent::Text("custom-loop: hello from replacement".into());
        if let Some(chain) = middleware {
            event = chain.process(event.clone()).unwrap_or(event);
        }
        if let Some((role, variant)) = event.clone().into_session_message() {
            session.push_message(FuneraMessage::new(role, variant));
        }

        // 4. Close the turn and feed the orchestration layer.
        let _ = react_bus.send(ReactEvent::TurnEnd);
        if let Some(sender) = event_sender {
            sender(event);
            sender(AgentEvent::Done);
        }

        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = AgentRuntime::<DeepSeekProvider>::builder()
        .api_key("sk-test")
        .model("deepseek-v4-flash")
        .with_loop(Arc::new(FixedAgentLoop))
        .build()?;

    let agent = Agent::builder()
        .system_prompt("You are a helpful assistant.")
        .build();

    let resp = agent.fire("Hello!", &runtime).await?;
    assert_eq!(resp.content, "custom-loop: hello from replacement");
    println!("response: {}", resp.content);

    println!("replace_react_loop: all assertions passed");
    Ok(())
}
