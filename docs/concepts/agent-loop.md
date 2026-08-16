# AgentLoop and Bus Contract

`AgentLoop` is a `Plugin` subtrait that replaces the built-in ReAct loop.

```rust,no_run
# use async_trait::async_trait;
# use funera_core::chat::message::FuneraMessage;
# use funera_core::chat::session::FuneraSession;
# use funera_core::event_bus::env_state_bus::EnvStateEvent;
# use funera_core::middleware::{EventSenderFn, MiddlewareProcessor};
# use funera_core::re_act::ReActLoopConfig;
# use funera_orchestrate::{AgentEvent, AgentLoop};
# use std::sync::Arc;
# use tokio::sync::broadcast;
#[async_trait]
impl AgentLoop for MyLoop {
    async fn run(
        &self,
        session: &FuneraSession,
        init_msg: FuneraMessage,
        config: ReActLoopConfig,
        env_state_tx: broadcast::Sender<EnvStateEvent>,
        middleware: Option<Arc<dyn MiddlewareProcessor<AgentEvent>>>,
        event_sender: Option<EventSenderFn<AgentEvent>>,
    ) -> anyhow::Result<()> {
        // Must drive the three buses:
        // - TokenBus: config.turn_highway_handle.prepare_turn().await
        // - ReactBus: ReactEvent::TurnStart / TurnEnd
        // - EnvStateBus: env_state_tx lifecycle events
        Ok(())
    }
}
# struct MyLoop;
# impl funera_core::plugin::Plugin for MyLoop { fn name(&self) -> &str { "my-loop" } }
```

See `replace_loop` / `replace_react_loop` examples.
