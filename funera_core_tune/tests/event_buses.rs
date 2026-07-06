use funera_core::chat::message::{FuneraMessage, MsgVariant, Role, TextMessage};
use funera_core::event_bus::env_state_bus::{EnvStateBus, EnvStateEvent, TurnHighWayHandle};
use funera_core::event_bus::react_bus::{ReactBus, ReactEvent, ToolCallRequest, ToolCallResponse};
use funera_core::event_bus::token_bus::TokenEvent;
use funera_core::event_bus::tool_bus::ToolBus;
use serde_json::json;

// ===== ReactBus =====

#[test]
fn react_bus_new_and_subscribe() {
    let bus = ReactBus::new();
    let _rx = bus.subscribe();
    assert!(bus.send(ReactEvent::TurnStart).is_ok());
}

#[test]
fn react_bus_multiple_subscribers() {
    let bus = ReactBus::new();
    let mut rx1 = bus.subscribe();
    let mut rx2 = bus.subscribe();
    bus.send(ReactEvent::TurnStart).ok();
    assert!(rx1.try_recv().is_ok());
    assert!(rx2.try_recv().is_ok());
}

#[test]
fn react_bus_send_turn_start_end() {
    let bus = ReactBus::new();
    let mut rx = bus.subscribe();
    bus.send(ReactEvent::TurnStart).ok();
    bus.send(ReactEvent::TurnEnd).ok();
    assert!(matches!(rx.try_recv().unwrap(), ReactEvent::TurnStart));
    assert!(matches!(rx.try_recv().unwrap(), ReactEvent::TurnEnd));
}

#[test]
fn react_bus_send_message_queued() {
    let bus = ReactBus::new();
    let mut rx = bus.subscribe();
    let msg = FuneraMessage::new(Role::User, MsgVariant::Text(TextMessage { text: "test".into() }));
    bus.send(ReactEvent::MessageQueued(msg.clone())).ok();
    match rx.try_recv().unwrap() {
        ReactEvent::MessageQueued(m) => assert_eq!(m.id(), msg.id()),
        _ => panic!("expected MessageQueued"),
    }
}

#[test]
fn react_bus_send_tool_exec_request() {
    let bus = ReactBus::new();
    let mut rx = bus.subscribe();
    let req = ToolCallRequest {
        index: 0,
        call_id: "call_123".into(),
        name: "search".into(),
        args: json!({"q": "hello"}),
    };
    bus.send(ReactEvent::ToolExecRequest(req)).ok();
    match rx.try_recv().unwrap() {
        ReactEvent::ToolExecRequest(r) => {
            assert_eq!(r.call_id, "call_123");
            assert_eq!(r.name, "search");
        }
        _ => panic!("expected ToolExecRequest"),
    }
}

#[test]
fn react_bus_send_tool_exec_response() {
    let bus = ReactBus::new();
    let mut rx = bus.subscribe();
    let resp = ToolCallResponse {
        call_id: "call_456".into(),
        result: "ok".into(),
    };
    bus.send(ReactEvent::ToolExecResponse(Ok(resp))).ok();
    match rx.try_recv().unwrap() {
        ReactEvent::ToolExecResponse(Ok(r)) => {
            assert_eq!(r.call_id, "call_456");
        }
        _ => panic!("expected ToolExecResponse(Ok)"),
    }
}

#[test]
fn react_bus_send_tool_exec_error() {
    let bus = ReactBus::new();
    let mut rx = bus.subscribe();
    bus.send(ReactEvent::ToolExecResponse(Err("fail".into()))).ok();
    match rx.try_recv().unwrap() {
        ReactEvent::ToolExecResponse(Err(e)) => assert_eq!(e, "fail"),
        _ => panic!("expected ToolExecResponse(Err)"),
    }
}

#[test]
fn react_bus_clone() {
    let bus1 = ReactBus::new();
    let bus2 = bus1.clone();
    let mut rx = bus2.subscribe();
    bus1.send(ReactEvent::TurnStart).ok();
    assert!(rx.try_recv().is_ok());
}

// ===== TokenEvent =====

#[test]
fn token_event_text() {
    let event = TokenEvent::Text("hello".into());
    match event {
        TokenEvent::Text(t) => assert_eq!(t, "hello"),
        _ => panic!("expected Text"),
    }
}

#[test]
fn token_event_tool_delta_full() {
    let event = TokenEvent::ToolDelta {
        index: 0,
        call_id: Some("call_1".into()),
        name: Some("search".into()),
        args_chunk: Some("{\"q\":".into()),
    };
    match event {
        TokenEvent::ToolDelta { index, call_id, name, args_chunk } => {
            assert_eq!(index, 0);
            assert_eq!(call_id.unwrap(), "call_1");
            assert_eq!(name.unwrap(), "search");
            assert_eq!(args_chunk.unwrap(), "{\"q\":");
        }
        _ => panic!("expected ToolDelta"),
    }
}

#[test]
fn token_event_tool_delta_partial() {
    let event = TokenEvent::ToolDelta {
        index: 1,
        call_id: None,
        name: None,
        args_chunk: None,
    };
    assert!(matches!(event, TokenEvent::ToolDelta { index: 1, call_id: None, name: None, args_chunk: None }));
}

#[test]
fn token_event_debug() {
    let event = TokenEvent::Text("hello".into());
    let debug = format!("{:?}", event);
    assert!(debug.contains("hello"));
}

// ===== ToolBus =====

#[tokio::test]
async fn tool_bus_execute_and_reply() {
    let (bus, mut rx) = ToolBus::new();
    let handle = tokio::spawn(async move {
        if let Some(cmd) = rx.recv().await {
            assert_eq!(cmd.name, "ping");
            cmd.resp_tx.send(Ok("pong".to_string())).ok();
        }
    });
    let result = bus.execute("call_1".into(), "ping".into(), json!({})).await;
    assert_eq!(result.unwrap(), "pong");
    handle.await.unwrap();
}

#[tokio::test]
async fn tool_bus_execute_no_receiver() {
    let (bus, _rx) = ToolBus::new();
    drop(_rx);
    let result = bus.execute("call_1".into(), "ping".into(), json!({})).await;
    assert!(result.is_err());
}

// ===== EnvStateBus =====

#[test]
fn env_state_bus_new_and_send() {
    let (bus, _handle) = EnvStateBus::new();
    let mut rx = bus.subscribe();
    bus.send(EnvStateEvent::SessionStart).ok();
    assert!(matches!(rx.try_recv(), Ok(EnvStateEvent::SessionStart)));
}

#[test]
fn env_state_bus_multiple_events() {
    let (bus, _handle) = EnvStateBus::new();
    let mut rx = bus.subscribe();
    bus.send(EnvStateEvent::SessionStart).ok();
    bus.send(EnvStateEvent::LlmChanged("gpt-4".into())).ok();
    assert!(matches!(rx.try_recv(), Ok(EnvStateEvent::SessionStart)));
    assert!(matches!(rx.try_recv(), Ok(EnvStateEvent::LlmChanged(_))));
}

#[test]
fn env_state_bus_tool_events() {
    let (bus, _handle) = EnvStateBus::new();
    let mut rx = bus.subscribe();
    bus.send(EnvStateEvent::ToolAdded("calc".into())).ok();
    bus.send(EnvStateEvent::ToolRemoved("calc".into())).ok();
    bus.send(EnvStateEvent::ToolAvailability("calc".into(), false)).ok();
    assert!(matches!(rx.try_recv(), Ok(EnvStateEvent::ToolAdded(_))));
    assert!(matches!(rx.try_recv(), Ok(EnvStateEvent::ToolRemoved(_))));
    assert!(matches!(rx.try_recv(), Ok(EnvStateEvent::ToolAvailability(_, false))));
}

#[test]
fn env_state_bus_new_turn_highway_handle() {
    let (_bus, handle) = EnvStateBus::new();
    let TurnHighWayHandle { turn_high_way_tx, .. } = handle;
    drop(turn_high_way_tx);
}

#[tokio::test]
async fn env_state_bus_session_closed() {
    let (bus, _handle) = EnvStateBus::new();
    let mut rx = bus.subscribe();
    bus.send(EnvStateEvent::SessionClosed).ok();
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    assert!(matches!(rx.try_recv(), Ok(EnvStateEvent::SessionClosed)));
}
