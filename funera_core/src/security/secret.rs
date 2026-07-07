use secrecy::{ExposeSecret, SecretString};
use zeroize::ZeroizeOnDrop;

use async_openai::config::OpenAIConfig;

#[derive(ZeroizeOnDrop)]
pub struct SecureApiKey(SecretString);

impl SecureApiKey {
    pub fn new(key: impl Into<String>) -> Self {
        Self(SecretString::new(key.into().into_boxed_str()))
    }

    pub fn expose_secret(&self) -> &str {
        self.0.expose_secret()
    }
}

impl std::fmt::Debug for SecureApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecureApiKey").finish_non_exhaustive()
    }
}

impl From<String> for SecureApiKey {
    fn from(key: String) -> Self {
        Self::new(key)
    }
}

impl From<&str> for SecureApiKey {
    fn from(key: &str) -> Self {
        Self::new(key.to_string())
    }
}

pub struct SecureClientBuilder {
    api_key: SecureApiKey,
    base_url: Option<String>,
}

impl SecureClientBuilder {
    pub fn new(api_key: impl Into<SecureApiKey>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: None,
        }
    }

    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = Some(url.into());
        self
    }

    pub fn build(self) -> OpenAIConfig {
        let mut config =
            OpenAIConfig::default().with_api_key(self.api_key.expose_secret().to_string());
        if let Some(url) = self.base_url {
            config = config.with_api_base(url);
        }
        config
    }
}

impl std::fmt::Debug for SecureClientBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecureClientBuilder")
            .field("api_key", &self.api_key)
            .field("base_url", &self.base_url)
            .finish()
    }
}
