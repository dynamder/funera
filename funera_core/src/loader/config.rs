//! Declarative plugin configuration loading.

use std::collections::HashMap;
use std::sync::Arc;

use serde::Deserialize;

use crate::loader::{HmrPolicy, PluginEntry};
use crate::plugin::{Plugin, PluginConfig};

/// An error produced while loading declarative plugin configuration.
#[derive(Debug, thiserror::Error)]
pub enum LoaderConfigError {
    #[error("unknown plugin name: {0}")]
    UnknownPlugin(String),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("invalid hmr policy: {0}")]
    InvalidHmrPolicy(String),
}

type PluginConstructor = Box<dyn Fn(Option<PluginConfig>) -> Arc<dyn Plugin> + Send + Sync>;

/// A registry of plugin constructors used by declarative configuration.
///
/// Register a constructor for each plugin name that may appear in a JSON
/// plugin set, then call [`load_entries_from_json`] to build [`PluginEntry`]s.
///
/// ```rust,no_run
/// use funera_core::loader::config::PluginFactory;
/// use funera_core::plugin::{Plugin, PluginConfig};
///
/// struct MyPlugin;
/// impl Plugin for MyPlugin { fn name(&self) -> &str { "my_plugin" } }
///
/// let mut factory = PluginFactory::new();
/// factory.register("my_plugin", |_cfg| MyPlugin);
/// ```
pub struct PluginFactory {
    constructors: HashMap<String, PluginConstructor>,
}

impl Default for PluginFactory {
    fn default() -> Self {
        Self::new()
    }
}

impl PluginFactory {
    pub fn new() -> Self {
        Self {
            constructors: HashMap::new(),
        }
    }

    /// Register a constructor for a named plugin.
    pub fn register<F, P>(&mut self, name: impl Into<String>, constructor: F) -> &mut Self
    where
        F: Fn(Option<PluginConfig>) -> P + Send + Sync + 'static,
        P: Plugin + 'static,
    {
        let name = name.into();
        self.constructors
            .insert(name, Box::new(move |config| Arc::new(constructor(config))));
        self
    }

    /// Create a plugin by name, or `None` if no constructor is registered.
    pub fn create(&self, name: &str, config: Option<PluginConfig>) -> Option<Arc<dyn Plugin>> {
        self.constructors
            .get(name)
            .map(|constructor| constructor(config))
    }
}

#[derive(Debug, Deserialize)]
struct RawEntry {
    id: String,
    plugin: String,
    #[serde(default)]
    disabled: bool,
    #[serde(default)]
    revision: u64,
    #[serde(default)]
    hmr: Option<String>,
    #[serde(default)]
    config: Option<serde_json::Value>,
}

fn raw_to_entry(raw: RawEntry, factory: &PluginFactory) -> Result<PluginEntry, LoaderConfigError> {
    let config = raw.config.map(PluginConfig::new);
    let plugin = factory
        .create(&raw.plugin, config.clone())
        .ok_or_else(|| LoaderConfigError::UnknownPlugin(raw.plugin.clone()))?;
    let hmr = match raw.hmr.as_deref() {
        None | Some("replace") => HmrPolicy::Replace,
        Some("swap") => HmrPolicy::Swap,
        Some(other) => return Err(LoaderConfigError::InvalidHmrPolicy(other.to_string())),
    };

    Ok(PluginEntry {
        id: raw.id,
        plugin,
        config: config.map(Arc::new),
        disabled: raw.disabled,
        revision: raw.revision,
        hmr,
    })
}

/// Parse a JSON array of plugin entries and instantiate them via `factory`.
pub fn load_entries_from_json(
    json: &str,
    factory: &PluginFactory,
) -> Result<Vec<PluginEntry>, LoaderConfigError> {
    let raw: Vec<RawEntry> = serde_json::from_str(json)?;
    raw.into_iter()
        .map(|raw| raw_to_entry(raw, factory))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::PluginError;
    use async_trait::async_trait;
    use serde_json::json;

    struct HelloPlugin {
        prefix: String,
    }

    #[async_trait]
    impl Plugin for HelloPlugin {
        fn name(&self) -> &str {
            "hello"
        }

        async fn apply(
            &self,
            _env: &crate::env::FuneraEnv,
            _config: Option<&PluginConfig>,
        ) -> Result<(), PluginError> {
            Ok(())
        }
    }

    #[test]
    fn factory_creates_registered_plugin() {
        let mut factory = PluginFactory::new();
        factory.register("hello", |cfg: Option<PluginConfig>| HelloPlugin {
            prefix: cfg
                .and_then(|c| c.value().get("prefix").cloned())
                .and_then(|v| v.as_str().map(String::from))
                .unwrap_or_default(),
        });

        let plugin = factory.create("hello", Some(PluginConfig::new(json!({"prefix": "hi"}))));
        assert!(plugin.is_some());
        assert_eq!(plugin.unwrap().name(), "hello");
    }

    #[test]
    fn load_entries_parses_json() {
        let mut factory = PluginFactory::new();
        factory.register("hello", |_cfg| HelloPlugin { prefix: "x".into() });

        let entries = load_entries_from_json(
            r#"[
                {"id": "a", "plugin": "hello", "revision": 1, "hmr": "swap"},
                {"id": "b", "plugin": "hello", "config": {"prefix": "hi"}}
            ]"#,
            &factory,
        )
        .unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].hmr, HmrPolicy::Swap);
        assert_eq!(entries[0].revision, 1);
        assert!(entries[1].config.is_some());
    }

    #[test]
    fn load_entries_reports_unknown_plugin() {
        let factory = PluginFactory::new();
        let err =
            load_entries_from_json(r#"[{"id": "a", "plugin": "ghost"}]"#, &factory).unwrap_err();
        assert!(matches!(err, LoaderConfigError::UnknownPlugin(name) if name == "ghost"));
    }
}
