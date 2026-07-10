use funera_core::chat::session::{FuneraSession, Idle};

use funera_core::env::FuneraEnv;
use funera_core::event_bus::env_state_bus::EnvStateBus;
use funera_core::event_bus::tool_bus::ToolBus;
use funera_core::re_act::tool::ToolRegistry;
use funera_core::re_act::ReActLoopConfig;
use funera_core_tune::utils::env_config::default_model;
use funera_core_tune::utils::fixtures::default_schema;
use funera_core_tune::utils::mock_tool::MockTool;

fn idle_session() -> FuneraSession<Idle> {
    FuneraSession::new()
}

#[test]
fn session_new_creates_idle_session() {
    let session = idle_session();
    let context = session.session_context();
    assert!(context.is_empty());
}

#[test]
fn session_id_is_unique() {
    let s1 = FuneraSession::<Idle>::new();
    let s2 = FuneraSession::<Idle>::new();
    assert_ne!(s1.id(), s2.id());
}

#[test]
fn idle_to_running_transition() {
    let idle = idle_session();
    let id = idle.id();
    let running = idle.run();
    assert_eq!(running.id(), id);
}

#[test]
fn running_to_idle_transition() {
    let idle = FuneraSession::<Idle>::new();
    let id = idle.id();
    let running = idle.run();
    let idle2 = running.idle();
    assert_eq!(idle2.id(), id);
}

#[test]
fn session_state_transitions_identity() {
    let idle = FuneraSession::<Idle>::new();
    let id_orig = idle.id();
    let running = idle.run();
    let idle2 = running.idle();
    assert_eq!(idle2.id(), id_orig);
}

#[test]
fn session_idle_reuse_after_stop() {
    let idle1 = FuneraSession::<Idle>::new();
    let id = idle1.id();
    let running = idle1.run();
    let idle2 = running.idle();
    assert_eq!(idle2.id(), id);
    let _running2 = idle2.run();
}

#[test]
fn session_push_message() {
    let idle = FuneraSession::<Idle>::new();
    let msg = funera_core::chat::message::FuneraMessage::new(
        funera_core::chat::message::Role::System,
        funera_core::chat::message::MsgVariant::Text(
            funera_core::chat::message::TextMessage {
                text: "system prompt".into(),
                reasoning_content: None,
            },
        ),
    );
    idle.push_message(msg);
    let ctx = idle.session_context();
    assert_eq!(ctx.len(), 1);
    assert_eq!(ctx[0]["role"], "system");
}

#[tokio::test]
async fn session_withenv_creates_react_loop_config() {
    let (_state_bus, turn_highway_handle) = EnvStateBus::new();

    let mut registry = ToolRegistry::new();
    let tool = MockTool::new("echo", default_schema("echo"));
    registry.add_tool(Box::new(tool));
    let client = async_openai::Client::new();
    let (_env, env_watcher) = FuneraEnv::new(registry, client, default_model());

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
