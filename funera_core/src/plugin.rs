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
use std::collections::HashMap;
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
    /// The derived env this instance runs against. Its effect accumulator is
    /// this instance's own, so [`FuneraEnv::dispose`] reverts only this
    /// instance's effects while its provided services remain shared.
    pub env: FuneraEnv,
}

impl PluginInstance {
    /// Create a fresh, not-yet-started instance around `plugin` and `env`.
    pub fn new(plugin: Arc<dyn Plugin>, env: FuneraEnv) -> Self {
        Self {
            plugin,
            state: InstanceState::Pending,
            env,
        }
    }
}

/// A registry that mounts [`Plugin`]s and drives their lifecycle reactively.
///
/// Each mounted plugin gets a derived [`FuneraEnv`]; the registry re-evaluates
/// every instance against the shared service table on [`refresh`](Self::refresh),
/// activating `Pending` instances whose `inject` requirements are now satisfied
/// and deactivating `Active` instances whose requirements are no longer met.
pub struct PluginRegistry {
    env: FuneraEnv,
    instances: HashMap<u64, PluginInstance>,
    next_id: u64,
}

impl PluginRegistry {
    /// Create a registry rooted at `env`.
    pub fn new(env: FuneraEnv) -> Self {
        Self {
            env,
            instances: HashMap::new(),
            next_id: 0,
        }
    }

    /// The root env (shared service table).
    pub fn env(&self) -> &FuneraEnv {
        &self.env
    }

    /// Mount a plugin, returning its instance id.
    ///
    /// The instance starts [`InstanceState::Pending`]; call
    /// [`refresh`](Self::refresh) to (re)evaluate it against the current
    /// services.
    pub fn mount(&mut self, plugin: Arc<dyn Plugin>) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let child = self.env.derive();
        self.instances.insert(id, PluginInstance::new(plugin, child));
        id
    }

    /// Re-evaluate every instance until no further transition fires.
    ///
    /// Activating an instance may provide services (and deactivating may remove
    /// them), which can satisfy or un-satisfy other instances; the loop runs a
    /// fixpoint so a single `refresh` settles the whole graph (bounded by the
    /// number of instances + 1 passes for acyclic graphs).
    pub fn refresh(&mut self) {
        let cap = self.instances.len() + 1;
        for _ in 0..cap {
            let mut changed = false;
            let ids: Vec<u64> = self.instances.keys().copied().collect();
            for id in ids {
                changed |= self.refresh_one(id);
            }
            if !changed {
                break;
            }
        }
    }

    /// Re-evaluate one instance; returns `true` if it transitioned.
    fn refresh_one(&mut self, id: u64) -> bool {
        let inject = self.instances[&id].plugin.inject().to_vec();
        let satisfied = inject.iter().all(|k| self.env.contains_typeid(*k));
        let state = self.instances[&id].state.clone();
        match (state, satisfied) {
            (InstanceState::Pending, true) => {
                self.activate(id);
                true
            }
            (InstanceState::Active, false) => {
                self.deactivate(id);
                true
            }
            _ => false,
        }
    }

    fn activate(&mut self, id: u64) {
        self.instances.get_mut(&id).unwrap().state = InstanceState::Loading;
        let result = {
            let inst = &self.instances[&id];
            let plugin = inst.plugin.clone();
            let env = inst.env.clone();
            plugin.apply(&env)
        };
        let next = match result {
            Ok(()) => InstanceState::Active,
            Err(e) => {
                // Roll back any effects the failed apply already registered.
                self.instances[&id].env.dispose();
                InstanceState::Failed(e.to_string())
            }
        };
        self.instances.get_mut(&id).unwrap().state = next;
    }

    fn deactivate(&mut self, id: u64) {
        self.instances.get_mut(&id).unwrap().state = InstanceState::Unloading;
        let env = self.instances[&id].env.clone();
        env.dispose();
        self.instances.get_mut(&id).unwrap().state = InstanceState::Pending;
    }

    /// Unmount an instance, reverting its effects and refreshing dependents.
    ///
    /// Returns the unmounted plugin, or `None` if `id` is unknown.
    pub fn unmount(&mut self, id: u64) -> Option<Arc<dyn Plugin>> {
        let inst = self.instances.remove(&id)?;
        if matches!(inst.state, InstanceState::Active) {
            inst.env.dispose();
        }
        self.refresh();
        Some(inst.plugin)
    }

    /// Current lifecycle state of an instance.
    pub fn state(&self, id: u64) -> Option<&InstanceState> {
        self.instances.get(&id).map(|i| &i.state)
    }

    /// Number of mounted instances.
    pub fn instance_count(&self) -> usize {
        self.instances.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

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
    impl Plugin for ProviderA {
        fn name(&self) -> &str {
            "provider-a"
        }
        fn provides(&self) -> &[TypeId] {
            &self.provides
        }
        fn apply(&self, env: &FuneraEnv) -> Result<(), PluginError> {
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
        fn apply(&self, env: &FuneraEnv) -> Result<(), PluginError> {
            assert!(
                env.contains::<ServiceA>(),
                "apply must run only once ServiceA is present"
            );
            env.provide(ServiceB);
            Ok(())
        }
    }

    #[test]
    fn mount_without_deps_stays_pending() {
        let mut reg = registry();
        let id = reg.mount(Arc::new(ConsumerOfA::new()));
        reg.refresh();
        assert_eq!(reg.state(id), Some(&InstanceState::Pending));
    }

    #[test]
    fn provider_satisfies_dependent() {
        let mut reg = registry();
        let provider = reg.mount(Arc::new(ProviderA::new()));
        let consumer = reg.mount(Arc::new(ConsumerOfA::new()));
        reg.refresh();
        assert_eq!(reg.state(provider), Some(&InstanceState::Active));
        assert_eq!(reg.state(consumer), Some(&InstanceState::Active));
        assert!(reg.env().contains::<ServiceB>());
    }

    #[test]
    fn removing_provider_deactivates_dependent() {
        let mut reg = registry();
        let provider = reg.mount(Arc::new(ProviderA::new()));
        let consumer = reg.mount(Arc::new(ConsumerOfA::new()));
        reg.refresh();
        assert_eq!(reg.state(consumer), Some(&InstanceState::Active));

        reg.unmount(provider);
        assert_eq!(reg.state(consumer), Some(&InstanceState::Pending));
        assert!(
            !reg.env().contains::<ServiceB>(),
            "deactivation must revert the dependent's provided services"
        );
    }

    #[test]
    fn unmount_reverts_effects() {
        static DROPS: AtomicUsize = AtomicUsize::new(0);

        struct EffectPlugin;
        impl Plugin for EffectPlugin {
            fn name(&self) -> &str {
                "effect"
            }
            fn apply(&self, env: &FuneraEnv) -> Result<(), PluginError> {
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
        reg.refresh();
        assert_eq!(reg.state(id), Some(&InstanceState::Active));
        assert_eq!(DROPS.load(Ordering::SeqCst), 1);

        reg.unmount(id);
        assert_eq!(
            DROPS.load(Ordering::SeqCst),
            0,
            "unmount must run the effect's inverse"
        );
    }

    #[test]
    fn failed_apply_rolls_back_and_marks_failed() {
        struct FailingPlugin;
        impl Plugin for FailingPlugin {
            fn name(&self) -> &str {
                "failing"
            }
            fn apply(&self, env: &FuneraEnv) -> Result<(), PluginError> {
                env.provide(String::from("partial"));
                Err("boom".into())
            }
        }

        let mut reg = registry();
        let id = reg.mount(Arc::new(FailingPlugin));
        reg.refresh();
        match reg.state(id) {
            Some(InstanceState::Failed(msg)) => assert_eq!(msg, "boom"),
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
