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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_config() {
        let err = OrchestrateError::Config("missing key".into());
        assert_eq!(err.to_string(), "agent not configured: missing key");
    }

    #[test]
    fn error_display_session() {
        let inner = anyhow::anyhow!("network failure");
        let err = OrchestrateError::Session(inner);
        assert!(err.to_string().starts_with("session error: "));
    }

    #[test]
    fn error_display_llm() {
        let err = OrchestrateError::Llm("rate limited".into());
        assert_eq!(err.to_string(), "LLM error: rate limited");
    }

    #[test]
    fn error_display_cancelled() {
        let err = OrchestrateError::Cancelled;
        assert_eq!(err.to_string(), "cancelled");
    }

    #[test]
    fn error_from_anyhow() {
        let inner = anyhow::anyhow!("something went wrong");
        let err: OrchestrateError = inner.into();
        assert!(err.to_string().starts_with("session error: "));
    }
}
