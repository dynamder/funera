use anyhow::{Context, Result};
use async_openai::config::OpenAIConfig;

#[derive(Debug, Clone)]
pub struct LlmConfig {
    pub api_key: String,
    pub api_base: String,
    pub model: String,
}

impl LlmConfig {
    pub fn from_env() -> Result<Self> {
        dotenvy::dotenv().ok();
        Ok(Self {
            api_key: std::env::var("OPENAI_API_KEY")
                .context("OPENAI_API_KEY not set in environment or .env file")?,
            api_base: std::env::var("OPENAI_API_BASE")
                .unwrap_or_else(|_| "https://api.openai.com/v1".into()),
            model: std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into()),
        })
    }

    pub fn build_client(&self) -> async_openai::Client<OpenAIConfig> {
        let config = OpenAIConfig::new()
            .with_api_key(&self.api_key)
            .with_api_base(&self.api_base);
        async_openai::Client::with_config(config)
    }
}

/// Reads the model name from environment / `.env`, falling back to `"gpt-4o-mini"`.
/// Does **not** require `OPENAI_API_KEY` — safe to use in mock-based tests.
pub fn default_model() -> String {
    dotenvy::dotenv().ok();
    std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into())
}
