use funera_core::chat::message::Role;
use funera_core::chat::session::{FuneraSession, Idle};
use funera_core::env::FuneraEnv;
use funera_core::event_bus::env_state_bus::TurnHighWayHandle;
use funera_core::event_bus::tool_bus::ToolBus;
use funera_core::re_act::tool::ToolRegistry;
use funera_core::re_act::tool_executor::ToolExecutor;
use funera_core::re_act::{ReActLoop, ReActLoopConfig};
use funera_core_tune::utils::env_config::default_model;
use funera_core_tune::utils::fixtures::{default_schema, text_message};
use funera_core_tune::utils::mock_tool::MockTool;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};

fn setup_env() -> (FuneraEnv, funera_core::env::FuneraEnvWatcher) {
    let mut registry = ToolRegistry::new();
    registry.add_tool(Box::new(MockTool::new("echo", default_schema("echo"))));
    let client = async_openai::Client::new();
    FuneraEnv::new(registry, client, default_model())
}

#[tokio::test]
async fn react_loop_config_new() {
    let (_env, env_watcher) = setup_env();
    let (bus, _rx) = ToolBus::new();
    let (tx, _rx) = broadcast::channel(20);
    let (tx1, rx1) = tokio::sync::mpsc::channel(5);
    let (tx2, _rx2) = tokio::sync::mpsc::channel(5);

    let handle = TurnHighWayHandle {
        turn_high_way_tx: tx1,
        turn_high_way_rx: _rx2,
    };

    let _ = tx2; // use the other end
    let _ = rx1;

    let config = ReActLoopConfig::new(10, 3, env_watcher, bus, tx, handle);
    assert_eq!(config.buffer, 10);
    assert_eq!(config.max_iteration, 3);
}

#[tokio::test]
async fn react_loop_new_and_sender() {
    let (_env, env_watcher) = setup_env();
    let (bus, _rx) = ToolBus::new();
    let (tx, _rx) = broadcast::channel(20);
    let (tx1, rx1) = tokio::sync::mpsc::channel(5);
    let (tx2, _rx2) = tokio::sync::mpsc::channel(5);

    let handle = TurnHighWayHandle {
        turn_high_way_tx: tx1,
        turn_high_way_rx: _rx2,
    };

    let _ = tx2;
    let _ = rx1;

    let session_msgs: Arc<parking_lot::RwLock<Vec<funera_core::chat::message::FuneraMessage>>> = Default::default();
    {
        let mut msgs = session_msgs.write();
        msgs.push(funera_core::chat::message::FuneraMessage::new(
            funera_core::chat::message::Role::User,
            funera_core::chat::message::MsgVariant::Text(funera_core::chat::message::TextMessage { text: "hello".into() }),
        ));
        msgs.push(funera_core::chat::message::FuneraMessage::new(
            funera_core::chat::message::Role::Assistant,
            funera_core::chat::message::MsgVariant::Text(funera_core::chat::message::TextMessage { text: "hi".into() }),
        ));
    }
    let loop_instance = ReActLoop::new(10, 5, session_msgs, env_watcher, bus, tx, handle);
    let sender = loop_instance.sender();
    let msg = text_message(Role::User, "test");
    sender.send(msg).await.ok();
}

#[tokio::test]
async fn react_loop_run_handle() {
    let (_env, env_watcher) = setup_env();
    let (bus, exec_rx) = ToolBus::new();

    let mut registry = ToolRegistry::new();
    let tool = MockTool::new("echo", default_schema("echo"));
    registry.add_tool(Box::new(tool));
    let tool_registry = Arc::new(RwLock::new(registry));
    tokio::spawn(ToolExecutor::new(tool_registry, exec_rx).run());

    let (state_tx, _state_rx) = broadcast::channel(20);
    let (tx1, _rx1) = tokio::sync::mpsc::channel(5);
    let (_tx2, rx2) = tokio::sync::mpsc::channel(5);

    let handle = TurnHighWayHandle {
        turn_high_way_tx: tx1,
        turn_high_way_rx: rx2,
    };

    let session_msgs: Arc<parking_lot::RwLock<Vec<funera_core::chat::message::FuneraMessage>>> = Default::default();
    session_msgs.write().push(funera_core::chat::message::FuneraMessage::new(
        funera_core::chat::message::Role::User,
        funera_core::chat::message::MsgVariant::Text(funera_core::chat::message::TextMessage { text: "hello".into() }),
    ));
    let loop_instance = ReActLoop::new(10, 2, session_msgs, env_watcher, bus, state_tx, handle);
    let _handle = loop_instance.run();

    // _handle.cancel_token.cancel() is available
    // _handle.sender can send messages
    // _handle.task is the JoinHandle
}

#[tokio::test]
async fn react_loop_cancel() {
    let (_env, env_watcher) = setup_env();
    let (bus, _exec_rx) = ToolBus::new();
    let (state_tx, _state_rx) = broadcast::channel(20);
    let (tx1, rx1) = tokio::sync::mpsc::channel(5);
    let (tx2, rx2) = tokio::sync::mpsc::channel(5);

    let handle = TurnHighWayHandle {
        turn_high_way_tx: tx1,
        turn_high_way_rx: rx2,
    };

    let _ = tx2;
    let _ = rx1;

    let session_msgs: Arc<parking_lot::RwLock<Vec<funera_core::chat::message::FuneraMessage>>> = Default::default();
    session_msgs.write().push(funera_core::chat::message::FuneraMessage::new(
        funera_core::chat::message::Role::User,
        funera_core::chat::message::MsgVariant::Text(funera_core::chat::message::TextMessage { text: "hello".into() }),
    ));
    let loop_instance = ReActLoop::new(10, 3, session_msgs, env_watcher, bus, state_tx, handle);
    let handle = loop_instance.run();

    handle.cancel_token.cancel();
}

#[tokio::test]
async fn session_creates_react_loop() {
    let session = FuneraSession::<Idle>::new();
    let _running = session.run();
}
