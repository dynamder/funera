//! Plugin architecture: reactive lifecycle, dependencies, and hot reload.
//!
//! This example demonstrates the unified [`Plugin`] abstraction and the
//! declarative [`Loader`] without touching an LLM:
//!
//! - a `GreeterPlugin` **depends on** a `Config` service provided by a
//!   `ConfigPlugin`;
//! - the loader activates plugins reactively — the greeter stays `Pending`
//!   until `Config` appears, and deactivates when its provider is removed;
//! - bumping an entry's `revision` hot-replaces it in place, reverting the old
//!   instance's effects before applying the new one.
//!
//! ```bash
//! cargo run --example plugin_architecture
//! ```

use std::any::TypeId;
use std::sync::Arc;

use funera_core::env::FuneraEnv;
use funera_core::loader::{Loader, PluginEntry};
use funera_core::plugin::{InstanceState, Plugin, PluginError};

/// A shared configuration service.
#[derive(Clone)]
struct Config {
    greeting: String,
}

/// The service `GreeterPlugin` provides once its `Config` dependency is met.
#[derive(Clone)]
struct Greeter;

/// Provides [`Config`].
struct ConfigPlugin {
    greeting: String,
    provides: Vec<TypeId>,
}

impl ConfigPlugin {
    fn new(greeting: &str) -> Self {
        Self {
            greeting: greeting.to_string(),
            provides: vec![TypeId::of::<Config>()],
        }
    }
}

impl Plugin for ConfigPlugin {
    fn name(&self) -> &str {
        "config"
    }

    fn provides(&self) -> &[TypeId] {
        &self.provides
    }

    fn apply(&self, env: &FuneraEnv) -> Result<(), PluginError> {
        env.provide(Config {
            greeting: self.greeting.clone(),
        });
        println!("    [config] provided Config(greeting = {:?})", self.greeting);
        Ok(())
    }
}

/// Needs [`Config`], provides [`Greeter`].
struct GreeterPlugin {
    inject: Vec<TypeId>,
    provides: Vec<TypeId>,
}

impl GreeterPlugin {
    fn new() -> Self {
        Self {
            inject: vec![TypeId::of::<Config>()],
            provides: vec![TypeId::of::<Greeter>()],
        }
    }
}

impl Plugin for GreeterPlugin {
    fn name(&self) -> &str {
        "greeter"
    }

    fn inject(&self) -> &[TypeId] {
        &self.inject
    }

    fn provides(&self) -> &[TypeId] {
        &self.provides
    }

    fn apply(&self, env: &FuneraEnv) -> Result<(), PluginError> {
        let config = env.get::<Config>().expect("Config must be present");
        env.provide(Greeter);
        println!(
            "    [greeter] active; will greet {:?}",
            config.greeting
        );
        Ok(())
    }
}

fn state<'a>(loader: &'a Loader, id: &str) -> Option<&'a InstanceState> {
    loader.entry_state(id)
}

fn main() {
    let (env, _watcher) = FuneraEnv::new(async_openai::Client::new(), "demo");
    let mut loader = Loader::new(env);

    // 1. Mount only the consumer: its dependency is absent, so it stays Pending.
    println!("1. greeter alone (Config missing):");
    loader.reconcile(&[PluginEntry::new(
        "greeter",
        Arc::new(GreeterPlugin::new()),
    )]);
    println!("    greeter = {:?}", state(&loader, "greeter"));
    assert_eq!(state(&loader, "greeter"), Some(&InstanceState::Pending));

    // 2. Add the provider: both plugins activate reactively in one reconcile.
    println!("\n2. config + greeter (dependency satisfied):");
    loader.reconcile(&[
        PluginEntry::new("config", Arc::new(ConfigPlugin::new("hello"))),
        PluginEntry::new("greeter", Arc::new(GreeterPlugin::new())),
    ]);
    println!("    config  = {:?}", state(&loader, "config"));
    println!("    greeter = {:?}", state(&loader, "greeter"));
    assert_eq!(state(&loader, "config"), Some(&InstanceState::Active));
    assert_eq!(state(&loader, "greeter"), Some(&InstanceState::Active));
    assert!(loader.registry().env().contains::<Greeter>());

    // 3. Remove the provider: the consumer deactivates and its service is
    //    reverted.
    println!("\n3. config removed (greeter deactivates):");
    loader.reconcile(&[PluginEntry::new(
        "greeter",
        Arc::new(GreeterPlugin::new()),
    )]);
    println!("    greeter = {:?}", state(&loader, "greeter"));
    assert_eq!(state(&loader, "greeter"), Some(&InstanceState::Pending));
    assert!(!loader.registry().env().contains::<Greeter>());

    // 4. Hot replacement: bump the config revision to reload it in place; the
    //    greeter reactivates against the new Config value.
    println!("\n4. config hot-replaced (revision 0 -> 1):");
    loader.reconcile(&[
        PluginEntry::new("config", Arc::new(ConfigPlugin::new("bonjour"))).revision(1),
        PluginEntry::new("greeter", Arc::new(GreeterPlugin::new())),
    ]);
    println!("    config  = {:?}", state(&loader, "config"));
    println!("    greeter = {:?}", state(&loader, "greeter"));
    assert_eq!(state(&loader, "config"), Some(&InstanceState::Active));
    assert_eq!(state(&loader, "greeter"), Some(&InstanceState::Active));

    println!("\nplugin_architecture: all assertions passed");
}
