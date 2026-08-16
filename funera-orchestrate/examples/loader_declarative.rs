//! Declarative plugin loading with [`PluginFactory`] and JSON config.
//!
//! ```bash
//! cargo run --example loader_declarative
//! ```

use async_trait::async_trait;
use funera_core::env::FuneraEnv;
use funera_core::loader::Loader;
use funera_core::loader::config::{PluginFactory, load_entries_from_json};
use funera_core::plugin::{Plugin, PluginConfig, PluginError, PluginPhase};

struct HelloPlugin;

#[async_trait]
impl Plugin for HelloPlugin {
    fn name(&self) -> &str {
        "hello"
    }

    async fn apply(
        &self,
        _env: &FuneraEnv,
        config: Option<&PluginConfig>,
    ) -> Result<(), PluginError> {
        let prefix = config
            .and_then(|c| c.value().get("prefix"))
            .and_then(|v| v.as_str())
            .unwrap_or("(no prefix)");
        println!("    [hello] applied with prefix = {prefix}");
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut factory = PluginFactory::new();
    factory.register("hello", |_cfg: Option<PluginConfig>| HelloPlugin);

    let json = r#"[
        {"id": "greeting", "plugin": "hello", "revision": 1, "config": {"prefix": "hi"}},
        {"id": "farewell", "plugin": "hello", "revision": 1, "config": {"prefix": "bye"}}
    ]"#;
    let entries = load_entries_from_json(json, &factory)?;

    let (env, _watcher) = FuneraEnv::new(async_openai::Client::new(), "demo");
    let mut loader = Loader::new(env);

    let report = loader.reconcile(&entries).await;
    println!("loaded: {:?}", report.loaded);
    assert_eq!(loader.entry_phase("greeting"), Some(PluginPhase::Active));
    assert_eq!(loader.entry_phase("farewell"), Some(PluginPhase::Active));

    // HMR: bump `greeting`'s revision and change its config.
    let json_v2 = r#"[
        {"id": "greeting", "plugin": "hello", "revision": 2, "config": {"prefix": "bonjour"}},
        {"id": "farewell", "plugin": "hello", "revision": 1, "config": {"prefix": "bye"}}
    ]"#;
    let entries_v2 = load_entries_from_json(json_v2, &factory)?;
    let report = loader.reconcile(&entries_v2).await;
    println!("reloaded: {:?}", report.reloaded);
    assert_eq!(report.reloaded, vec!["greeting".to_string()]);

    println!("loader_declarative: all assertions passed");
    Ok(())
}
