use funera_core::chat::session::{FuneraSession, Idle};
use funera_core::env::FuneraEnv;
use funera_core::event_bus::env_state_bus::EnvStateBus;
use funera_core::event_bus::tool_bus::ToolBus;
use funera_core::re_act::tool::ToolRegistry;
use funera_core::re_act::ReActLoopConfig;
use funera_core_tune::utils::fixtures::default_schema;
use funera_core_tune::utils::mock_tool::MockTool;

#[tokio::test]
async fn session_idle_to_running_transition() {
    let idle = FuneraSession::<Idle>::new();
    let id = idle.id();
    let running = idle.run();
    assert_eq!(running.id(), id);
}

#[tokio::test]
async fn session_running_to_idle_transition() {
    let idle = FuneraSession::<Idle>::new();
    let running = idle.run();
    let idle2 = running.idle();
    let _ = idle2.session_context();
}

#[tokio::test]
async fn session_idle_reuse_after_stop() {
    let idle1 = FuneraSession::<Idle>::new();
    let id = idle1.id();
    let running = idle1.run();
    let idle2 = running.idle();
    assert_eq!(idle2.id(), id);
    let _running2 = idle2.run();
}

#[tokio::test]
async fn session_withenv_creates_react_loop_config() {
    let (_state_bus, turn_highway_handle) = EnvStateBus::new();

    let mut registry = ToolRegistry::new();
    let tool = MockTool::new("echo", default_schema("echo"));
    registry.add_tool(Box::new(tool));
    let client = async_openai::Client::new();
    let (_env, env_watcher) = FuneraEnv::new(registry, client, "gpt-4o-mini");

    let (tool_bus, _exec_rx) = ToolBus::new();
    let (env_state_tx, _env_state_rx) = tokio::sync::broadcast::channel(20);

    let config = ReActLoopConfig::new(
        10,
        3,
        env_watcher,
        tool_bus,
        env_state_tx.clone(),
        turn_highway_handle,
    );

    assert_eq!(config.buffer, 10);
    assert_eq!(config.max_iteration, 3);
}

#[tokio::test]
async fn session_env_state_events_sent() {
    let (state_bus, _turn_highway_handle) = EnvStateBus::new();
    let mut rx = state_bus.subscribe();
    state_bus.send(funera_core::event_bus::env_state_bus::EnvStateEvent::SessionStart).ok();
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    assert!(matches!(
        rx.try_recv(),
        Ok(funera_core::event_bus::env_state_bus::EnvStateEvent::SessionStart)
    ));
}
