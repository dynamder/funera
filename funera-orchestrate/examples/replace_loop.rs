//! Replace the built-in ReAct loop with a custom AgentLoop, and show that the
//! custom loop is also a mountable Plugin.
//!
//! ```bash
//! cargo run --example replace_loop
//! ```

use std::sync::Arc;

use async_trait::async_trait;
use funera_core::chat::message::FuneraMessage;
use funera_core::chat::session::FuneraSession;
use funera_core::event_bus::env_state_bus::EnvStateEvent;
use funera_core::event_bus::react_bus::ReactEvent;
use funera_core::event_bus::token_bus::TokenEvent;
use funera_core::middleware::{EventSenderFn, MiddlewareEvent, MiddlewareProcessor};
use funera_core::plugin::{Plugin, PluginPhase, PluginRegistry};
use funera_core::re_act::ReActLoopConfig;
use funera_orchestrate::{Agent, AgentEvent, AgentLoop, AgentRuntime, DeepSeekProvider};

#[derive(Default)]
struct FixedLoop;

impl Plugin for FixedLoop {
    fn name(&self) -> &str {
        "fixed-loop"
    }
}

#[async_trait]
impl AgentLoop for FixedLoop {
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

        let (token_tx, react_bus) = config.turn_highway_handle.prepare_turn().await;
        let _ = token_tx.send(TokenEvent::Text("replacement-loop: hello".into()));
        let _ = react_bus.send(ReactEvent::TurnStart);

        let mut event = AgentEvent::Text("replacement-loop: hello".into());
        if let Some(chain) = middleware {
            event = chain.process(event.clone()).unwrap_or(event);
        }
        if let Some((role, variant)) = event.clone().into_session_message() {
            session.push_message(FuneraMessage::new(role, variant));
        }

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
    // 1. Since AgentLoop is a Plugin subtrait, the same custom loop can be
    //    mounted into a PluginRegistry.
    let (env, _watcher) =
        funera_core::env::FuneraEnv::new(async_openai::Client::new(), "test-model");
    let mut registry = PluginRegistry::new(env);
    let loop_id = registry.mount(Arc::new(FixedLoop::default()));
    registry.refresh().await;
    assert_eq!(registry.phase(loop_id), Some(PluginPhase::Active));

    // 2. It can also replace the runtime's built-in loop.
    let runtime = AgentRuntime::<DeepSeekProvider>::builder()
        .api_key("sk-test")
        .model("deepseek-v4-flash")
        .with_loop(Arc::new(FixedLoop::default()))
        .build()?;

    let agent = Agent::builder().build();
    let resp = agent.fire("Hello!", &runtime).await?;
    assert_eq!(resp.content, "replacement-loop: hello");

    println!("replacement-loop response: {}", resp.content);
    println!("replace_loop: all assertions passed");
    Ok(())
}
