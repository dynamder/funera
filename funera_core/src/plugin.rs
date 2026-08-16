//! # Plugin — the unified loadable/unloadable unit
//!
//! A [`Plugin`] declares what it reads from the environment
//! ([`inject`](Plugin::inject)), what it writes to it
//! ([`provides`](Plugin::provides)), and what it does on load
//! ([`apply`](Plugin::apply)).
//!
//! The capability traits — `Tool`, `ChatProvider`,
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
//! ```rust,no_run
//! use funera_core::plugin::Plugin;
//!
//! struct MyTool;
//!
//! impl Plugin for MyTool {
//!     fn name(&self) -> &str { "my_tool" }
//! }
//! ```

use std::any::TypeId;

use async_trait::async_trait;

use crate::env::FuneraEnv;

pub mod instance;
pub mod registry;

pub use instance::{InstanceId, PluginInstance, TargetDigest, state};
pub use registry::{FailedEntry, PluginPhase, PluginRegistry};

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
#[async_trait]
pub trait Plugin: Send + Sync {
    /// Unique name for this plugin (e.g. `"read"`, `"deepseek"`).
    fn name(&self) -> &str;

    /// Services (by [`TypeId`]) this plugin requires from the environment.
    ///
    /// The plugin stays [`state::Pending`](state::Pending) until every one is
    /// provided.
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
    async fn apply(&self, _env: &FuneraEnv) -> Result<(), PluginError> {
        Ok(())
    }
}

/// Declare that a type is a [`Plugin`] with the given name.
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn registry() -> PluginRegistry {
        let (env, _watcher) = FuneraEnv::new(async_openai::Client::new(), "test-model");
        PluginRegistry::new(env)
    }

    struct ServiceA;
    struct ServiceB;

    /// Provides `ServiceA`.
    struct ProviderA {
        provides: Vec<TypeId>,
    }
    impl ProviderA {
        fn new() -> Self {
            Self {
                provides: vec![TypeId::of::<ServiceA>()],
            }
        }
    }
    #[async_trait]
    impl Plugin for ProviderA {
        fn name(&self) -> &str {
            "provider-a"
        }
        fn provides(&self) -> &[TypeId] {
            &self.provides
        }
        async fn apply(&self, env: &FuneraEnv) -> Result<(), PluginError> {
            env.provide(ServiceA);
            Ok(())
        }
    }

    /// Needs `ServiceA`, provides `ServiceB`.
    struct ConsumerOfA {
        inject: Vec<TypeId>,
        provides: Vec<TypeId>,
    }
    impl ConsumerOfA {
        fn new() -> Self {
            Self {
                inject: vec![TypeId::of::<ServiceA>()],
                provides: vec![TypeId::of::<ServiceB>()],
            }
        }
    }
    #[async_trait]
    impl Plugin for ConsumerOfA {
        fn name(&self) -> &str {
            "consumer-of-a"
        }
        fn inject(&self) -> &[TypeId] {
            &self.inject
        }
        fn provides(&self) -> &[TypeId] {
            &self.provides
        }
        async fn apply(&self, env: &FuneraEnv) -> Result<(), PluginError> {
            assert!(
                env.contains::<ServiceA>(),
                "apply must run only once ServiceA is present"
            );
            env.provide(ServiceB);
            Ok(())
        }
    }

    #[tokio::test]
    async fn mount_without_deps_stays_pending() {
        let mut reg = registry();
        let id = reg.mount(Arc::new(ConsumerOfA::new()));
        reg.refresh().await;
        assert_eq!(reg.phase(id), Some(PluginPhase::Pending));
    }

    #[tokio::test]
    async fn provider_satisfies_dependent() {
        let mut reg = registry();
        let provider = reg.mount(Arc::new(ProviderA::new()));
        let consumer = reg.mount(Arc::new(ConsumerOfA::new()));
        reg.refresh().await;
        assert_eq!(reg.phase(provider), Some(PluginPhase::Active));
        assert_eq!(reg.phase(consumer), Some(PluginPhase::Active));
        assert!(reg.env().contains::<ServiceB>());
    }

    #[tokio::test]
    async fn removing_provider_deactivates_dependent() {
        let mut reg = registry();
        let provider = reg.mount(Arc::new(ProviderA::new()));
        let consumer = reg.mount(Arc::new(ConsumerOfA::new()));
        reg.refresh().await;
        assert_eq!(reg.phase(consumer), Some(PluginPhase::Active));

        reg.unmount(provider).await;
        assert_eq!(reg.phase(consumer), Some(PluginPhase::Inactive));
        assert!(
            !reg.env().contains::<ServiceB>(),
            "deactivation must revert the dependent's provided services"
        );
    }

    #[tokio::test]
    async fn unmount_reverts_effects() {
        static DROPS: AtomicUsize = AtomicUsize::new(0);

        struct EffectPlugin;
        #[async_trait]
        impl Plugin for EffectPlugin {
            fn name(&self) -> &str {
                "effect"
            }
            async fn apply(&self, env: &FuneraEnv) -> Result<(), PluginError> {
                DROPS.fetch_add(1, Ordering::SeqCst);
                env.effect(|| {
                    Box::new(|| {
                        DROPS.fetch_sub(1, Ordering::SeqCst);
                    })
                });
                Ok(())
            }
        }

        let mut reg = registry();
        let id = reg.mount(Arc::new(EffectPlugin));
        reg.refresh().await;
        assert_eq!(reg.phase(id), Some(PluginPhase::Active));
        assert_eq!(DROPS.load(Ordering::SeqCst), 1);

        reg.unmount(id).await;
        assert_eq!(
            DROPS.load(Ordering::SeqCst),
            0,
            "unmount must run the effect's inverse"
        );
    }

    #[tokio::test]
    async fn failed_apply_rolls_back_and_marks_failed() {
        struct FailingPlugin;
        #[async_trait]
        impl Plugin for FailingPlugin {
            fn name(&self) -> &str {
                "failing"
            }
            async fn apply(&self, env: &FuneraEnv) -> Result<(), PluginError> {
                env.provide(String::from("partial"));
                Err("boom".into())
            }
        }

        let mut reg = registry();
        let id = reg.mount(Arc::new(FailingPlugin));
        reg.refresh().await;
        match reg.phase(id) {
            Some(PluginPhase::Failed(msg)) => assert_eq!(msg, "boom"),
            other => panic!("expected Failed, got {other:?}"),
        }
        assert!(
            !reg.env().contains::<String>(),
            "partial effects must be rolled back"
        );
    }

    #[test]
    fn service_change_observer_fires_on_provide_and_remove() {
        let (env, _watcher) = FuneraEnv::new(async_openai::Client::new(), "test-model");
        let events: Arc<Mutex<Vec<(TypeId, bool)>>> = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&events);
        env.on_service_change(Arc::new(move |key, present| {
            recorded.lock().unwrap().push((key, present));
        }));

        env.provide(String::from("x"));
        env.dispose();

        let events = events.lock().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].0, TypeId::of::<String>());
        assert!(events[0].1);
        assert_eq!(events[1].0, TypeId::of::<String>());
        assert!(!events[1].1);
    }
}
