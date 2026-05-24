use tokio::{sync::mpsc, task::JoinHandle};

use crate::chat::message::FuneraMessage;
use anyhow::Result;

pub mod skills;
pub mod tool;

pub struct ReActLoop {
    buf_msg_rx: mpsc::Receiver<FuneraMessage>,
    buf_msg_tx: mpsc::Sender<FuneraMessage>,
    max_iteration: usize,
}
impl ReActLoop {
    pub fn new(buffer: usize, max_iteration: usize) -> Self {
        let (tx, rx) = mpsc::channel(buffer);
        Self {
            buf_msg_rx: rx,
            buf_msg_tx: tx,
            max_iteration,
        }
    }
    pub fn sender(&self) -> mpsc::Sender<FuneraMessage> {
        self.buf_msg_tx.clone()
    }
    pub fn run(mut self) -> JoinHandle<Result<()>> {
        tokio::spawn(async move {
            let self_tx = self.sender();
            while let Some(msg) = self.buf_msg_rx.recv().await {
                // TODO: 1. format msg and session context
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
        })
    }
}
