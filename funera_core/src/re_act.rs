use tokio::{sync::mpsc, task::JoinHandle};
use tokio_util::sync::CancellationToken;

use crate::chat::message::FuneraMessage;
use anyhow::Result;
use serde_json::Value as JsonValue;

pub mod skills;
pub mod tool;

pub struct ReActLoopConfig {
    pub buffer: usize,
    pub max_iteration: usize,
}
impl ReActLoopConfig {
    pub fn new(buffer: usize, max_iteration: usize) -> Self {
        Self {
            buffer,
            max_iteration,
        }
    }
}

pub struct ReActLoop {
    buf_msg_rx: mpsc::Receiver<FuneraMessage>,
    buf_msg_tx: mpsc::Sender<FuneraMessage>,
    history_msg: Vec<JsonValue>,
    max_iteration: usize,
}
impl ReActLoop {
    pub fn new(buffer: usize, max_iteration: usize, history_msg: Vec<JsonValue>) -> Self {
        let (tx, rx) = mpsc::channel(buffer);
        Self {
            buf_msg_rx: rx,
            buf_msg_tx: tx,
            history_msg,
            max_iteration,
        }
    }
    pub fn from_config(config: ReActLoopConfig, history_msg: Vec<JsonValue>) -> Self {
        Self::new(config.buffer, config.max_iteration, history_msg)
    }
    pub fn sender(&self) -> mpsc::Sender<FuneraMessage> {
        self.buf_msg_tx.clone()
    }
    pub fn run(mut self) -> ReActLoopHandle {
        let token = CancellationToken::new();
        let token_clone = token.clone();

        let sender = self.sender();
        let self_tx = sender.clone();

        let task = tokio::spawn(async move {
            let loop_self_tx = self_tx;
            while let Some(msg) = self.buf_msg_rx.recv().await {
                // TODO: 1. format msg and session context, FuneraEnv
                // TODO: 2. send formatted msg to llm api
                // TODO: 3. receive response from llm api, execute tools(optional)
                // TODO: 4. add tool reponse to buffered message channel
                // TODO: 5. exit if channel is empty
                // the message channel sender can be self_tx, or user queued message.

                if self.buf_msg_rx.is_empty() {
                    break;
                }
            }
            Ok(())
        });
        ReActLoopHandle {
            cancel_token: token_clone,
            task,
            sender,
        }
    }
}

#[derive(Debug)]
pub struct ReActLoopHandle {
    pub cancel_token: CancellationToken,
    pub task: JoinHandle<Result<()>>,
    pub sender: mpsc::Sender<FuneraMessage>,
}
