use thiserror::Error;

#[derive(Debug, Error)]
pub enum OrchestrateError {
    #[error("agent not configured: {0}")]
    Config(String),

    #[error("session error: {0}")]
    Session(#[from] anyhow::Error),

    #[error("LLM error: {0}")]
    Llm(String),

    #[error("cancelled")]
    Cancelled,
}
