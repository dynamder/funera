//! # Plugin — the unified loadable/unloadable unit
//!
//! A [`Plugin`] declares what it reads from the environment
//! ([`inject`](Plugin::inject)), what it writes to it
//! ([`provides`](Plugin::provides)), and what it does on load
//! ([`apply`](Plugin::apply)).
//!
//! The existing capability traits — `Tool`, `ChatProvider`,
//! `InspectorMiddleware`, `MutatorMiddleware` — are **subtraits** of `Plugin`,
//! so anything that can do work is also a loadable plugin and shares one
//! mount/unmount path.
//!
//! ```text
//! Plugin                 "it can be loaded / unloaded"
//!  ├── Tool              "…and it is a callable tool"
//!  ├── ChatProvider      "…and it is an LLM backend"
//!  ├── InspectorMiddleware "…and it observes events"
//!  └── MutatorMiddleware   "…and it rewrites/block events"
//! ```
//!
//! This keeps existing code compiling with one extra declaration per type:
//!
//! ```rust,no_run
//! use funera_core::plugin::Plugin;
//!
//! struct MyTool;
//!
//! impl Plugin for MyTool {
//!     fn name(&self) -> &str { "my_tool" }
//! }
//! // `impl Tool for MyTool { ... }` continues to work as before, minus the
//! // old `name()` method (now inherited from `Plugin`).
//! ```

use std::any::TypeId;
use std::sync::Arc;

use crate::env::FuneraEnv;

/// Boxed error returned by [`Plugin::apply`].
pub type PluginError = Box<dyn std::error::Error + Send + Sync>;

/// The unified abstraction over every loadable capability.
///
/// This is the umbrella trait over the capability traits (`Tool`,
/// `ChatProvider`, …). The four methods map directly onto what a component
/// needs to participate in dynamic composition:
///
/// - [`name`](Plugin::name) — stable identity;
/// - [`inject`](Plugin::inject) — what the plugin reads from the environment
///   (its dependencies);
/// - [`provides`](Plugin::provides) — what the plugin may write to the
///   environment (the services it offers);
/// - [`apply`](Plugin::apply) — the load hook, where the plugin acquires
///   resources and registers capabilities through the environment.
///
/// `inject`, `provides`, and `apply` have defaults so a capability-only type
/// only has to supply `name()` to satisfy the supertrait.
pub trait Plugin: Send + Sync {
    /// Unique name for this plugin (e.g. `"read"`, `"deepseek"`).
    fn name(&self) -> &str;

    /// Services (by [`TypeId`]) this plugin requires from the environment.
    ///
    /// The plugin stays [`InstanceState::Pending`] until every one is provided.
    fn inject(&self) -> &[TypeId] {
        &[]
    }

    /// Services (by [`TypeId`]) this plugin may provide to the environment.
    fn provides(&self) -> &[TypeId] {
        &[]
    }

    /// Load hook.
    ///
    /// Acquire resources and register capabilities here through the
    /// environment (`env.effect`, `env.provide`, `env.get`, …). The default is
    /// a no-op; capability subtraits are registered by the framework (which
    /// still knows their concrete type) rather than registering themselves
    /// from inside `apply`.
    fn apply(&self, _env: &FuneraEnv) -> Result<(), PluginError> {
        Ok(())
    }
}

/// Declare that a type is a [`Plugin`] with the given name.
///
/// This is the migration bridge for existing capability impls: it supplies the
/// one method the new supertrait requires, so a type implementing `Tool` /
/// `ChatProvider` / etc. only needs this single line on top.
///
/// ```rust,no_run
/// struct Calculator;
/// funera_core::impl_plugin!(Calculator, "calculator");
/// ```
#[macro_export]
macro_rules! impl_plugin {
    ($ty:ty, $name:expr) => {
        impl $crate::plugin::Plugin for $ty {
            fn name(&self) -> &str {
                $name
            }
        }
    };
}

/// Lifecycle state of a mounted plugin instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstanceState {
    /// Declared, but a required service is not yet available.
    Pending,
    /// [`Plugin::apply`] is running.
    Loading,
    /// `apply` completed; the plugin is live and its capabilities are active.
    Active,
    /// Disposers are running.
    Unloading,
    /// Everything is torn down.
    Disposed,
    /// `apply` (or config validation) failed; effects were rolled back.
    Failed(String),
}

/// One mounted plugin: the plugin itself plus its lifecycle state.
///
/// This is the runtime instantiation of a [`Plugin`]; the environment it runs
/// in and the set of committed dependencies are attached here as the lifecycle
/// machinery lands.
pub struct PluginInstance {
    /// The plugin this instance runs.
    pub plugin: Arc<dyn Plugin>,
    /// Where in its lifecycle this instance currently stands.
    pub state: InstanceState,
}

impl PluginInstance {
    /// Create a fresh, not-yet-started instance around `plugin`.
    pub fn new(plugin: Arc<dyn Plugin>) -> Self {
        Self {
            plugin,
            state: InstanceState::Pending,
        }
    }
}
