use std::sync::Arc;

use serde_json::Value as JsonValue;
use tokio::sync::{broadcast, mpsc, oneshot};
use uuid::Uuid;

use crate::{
    chat::message::FuneraMessage,
    event_bus::env_state_bus::EnvStateEvent,
    middleware::{ErrorsEnabled, EventSenderFn, MiddlewareChain, MiddlewareEvent},
    re_act::{ReActLoop, ReActLoopConfig},
};

// ═══════════════════════════════════════════════════════════════
// SessionCmd — 通过 channel 与 SessionActor 通信的协议
// ═══════════════════════════════════════════════════════════════

pub enum SessionCmd {
    /// 向 session 末尾追加一批消息
    PushMessages { msgs: Vec<FuneraMessage> },
    /// 获取当前上下文（JSON 格式，用于构建 LLM 请求）
    FetchContext {
        respond: oneshot::Sender<Vec<JsonValue>>,
    },
    /// 获取当前消息列表（核心版本，用于检查/显示）
    GetMessages {
        respond: oneshot::Sender<Vec<FuneraMessage>>,
    },
    /// 清空 session
    Clear,
}

/// 启动一个长期运行的 SessionActor，返回其控制通道。
///
/// actor 内部维护 `Vec<FuneraMessage>`，通过 `SessionCmd` 处理
/// 推送、查询、清空操作。`ReActLoop` 通过克隆的 `session_tx`
/// 在 reactor 循环中读写 session。
///
/// # 示例
///
/// ```rust,no_run
/// # use funera_core::chat::session::spawn_session_actor;
/// let session_tx = spawn_session_actor();
/// ```
pub fn spawn_session_actor() -> mpsc::UnboundedSender<SessionCmd> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    // Actor runs until all sender handles are dropped; rx.recv() then
    // returns None and the task exits cleanly — no leak.
    tokio::spawn(async move {
        let mut msgs: Vec<FuneraMessage> = Vec::new();
        while let Some(cmd) = rx.recv().await {
            match cmd {
                SessionCmd::PushMessages { msgs: new } => msgs.extend(new),
                SessionCmd::FetchContext { respond } => {
                    let ctx: Vec<JsonValue> = msgs.iter().map(|m| m.format_json()).collect();
                    let _ = respond.send(ctx);
                }
                SessionCmd::GetMessages { respond } => {
                    let _ = respond.send(msgs.clone());
                }
                SessionCmd::Clear => msgs.clear(),
            }
        }
    });
    tx
}

/// Session 薄壳——通过 `session_tx` 与后端 Actor 通信。
///
/// 所有权始终由 `AgentRuntime` 持有（通过 `spawn_session_actor` 创建）。
/// `FuneraSession` 仅提供便利方法，不直接持有消息数据。
pub struct FuneraSession {
    id: Uuid,
    session_tx: mpsc::UnboundedSender<SessionCmd>,
}

impl FuneraSession {
    pub fn new(session_tx: mpsc::UnboundedSender<SessionCmd>) -> Self {
        Self {
            id: Uuid::new_v4(),
            session_tx,
        }
    }

    pub fn id(&self) -> Uuid {
        self.id
    }

    pub fn session_tx(&self) -> mpsc::UnboundedSender<SessionCmd> {
        self.session_tx.clone()
    }

    /// 向 session 追加一条消息。
    pub fn push_message(&self, msg: FuneraMessage) {
        let _ = self
            .session_tx
            .send(SessionCmd::PushMessages { msgs: vec![msg] });
    }

    /// 异步获取 session 上下文（JSON 格式）。
    pub async fn session_context(&self) -> Vec<JsonValue> {
        let (respond, rx) = oneshot::channel();
        let _ = self.session_tx.send(SessionCmd::FetchContext { respond });
        rx.await.unwrap_or_default()
    }

    /// 异步获取消息列表（不可变引用）。
    pub async fn get_messages(&self) -> Vec<FuneraMessage> {
        let (respond, rx) = oneshot::channel();
        let _ = self.session_tx.send(SessionCmd::GetMessages { respond });
        rx.await.unwrap_or_default()
    }

    /// 运行 ReAct 循环。
    ///
    /// `init_msg` 会通过 `session_tx` 推送到 session actor。
    /// 通过 `session_tx.clone()` 传递给 `ReActLoop`，使其可在循环期间
    /// 读写 session 而不移出所有权。
    ///
    /// 完成后返回 `()`。session 数据保留在 actor 中（外部通过 `session_context()` 查询）。
    pub async fn react_loop<P: crate::provider::ChatProvider, E: MiddlewareEvent>(
        &self,
        init_msg: FuneraMessage,
        mut config: ReActLoopConfig,
        env_state_tx: broadcast::Sender<EnvStateEvent>,
        middleware: Option<Arc<MiddlewareChain<E, ErrorsEnabled>>>,
        event_sender: Option<EventSenderFn<E>>,
    ) -> anyhow::Result<()> {
        let _ = env_state_tx.send(EnvStateEvent::SessionStart);

        // Push init message to session actor
        self.push_message(init_msg);

        config.session_tx = Some(self.session_tx.clone());
        let react_loop = ReActLoop::<P>::from_config(config);
        let loop_handle = react_loop.run::<E>(middleware, event_sender);

        loop_handle.task.await??;

        let _ = env_state_tx.send(EnvStateEvent::SessionClosed);
        Ok(())
    }
}
