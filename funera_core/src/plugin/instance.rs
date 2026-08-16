//! Typestate plugin instance and lifecycle state markers.

use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::sync::Arc;

use crate::env::{FuneraEnv, ProviderId};
use crate::plugin::Plugin;

/// Runtime id of a mounted plugin instance.
pub type InstanceId = u64;

/// A digest of the currently resolved dependencies of an instance.
///
/// Two instances with the same target digest resolve to the same set of
/// services. For now this is a simple hash over the satisfied `TypeId`s; once
/// provider identity is tracked it will be a digest over provider ids.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct TargetDigest(u64);

impl TargetDigest {
    pub(crate) fn from_provider_pairs<'a>(
        iter: impl Iterator<Item = (&'a std::any::TypeId, ProviderId)>,
    ) -> Self {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for (type_id, provider_id) in iter {
            type_id.hash(&mut hasher);
            provider_id.hash(&mut hasher);
        }
        Self(hasher.finish())
    }
}

/// Lifecycle state markers (zero-sized).
pub mod state {
    /// Declared but one or more required services are missing.
    pub struct Pending;
    /// [`Plugin::apply`](crate::plugin::Plugin::apply) is running.
    pub struct Loading;
    /// `apply` completed; the plugin is live.
    pub struct Active;
    /// Disposers are running.
    pub struct Unloading;
    /// Torn down, can be reactivated when dependencies appear.
    pub struct Inactive;
    /// `apply` failed; effects were rolled back.
    pub struct Failed;
}

/// A plugin instance whose lifecycle state is carried in the type parameter.
///
/// The state marker is a ZST, so it costs nothing at runtime. Transitions are
/// methods on the concrete state (`begin_load`, `finish_load`, ...); invalid
/// transitions simply do not compile.
pub struct PluginInstance<S> {
    pub id: InstanceId,
    pub plugin: Arc<dyn Plugin>,
    pub env: FuneraEnv,
    pub target: Option<TargetDigest>,
    pub committed: Option<TargetDigest>,
    pub _state: PhantomData<S>,
}

impl<S> PluginInstance<S> {
    fn new(
        id: InstanceId,
        plugin: Arc<dyn Plugin>,
        env: FuneraEnv,
        target: Option<TargetDigest>,
        committed: Option<TargetDigest>,
    ) -> Self {
        Self {
            id,
            plugin,
            env,
            target,
            committed,
            _state: PhantomData,
        }
    }

    fn with_state<T>(self) -> PluginInstance<T> {
        PluginInstance {
            id: self.id,
            plugin: self.plugin,
            env: self.env,
            target: self.target,
            committed: self.committed,
            _state: PhantomData,
        }
    }

    pub(crate) fn with_target(mut self, target: Option<TargetDigest>) -> Self {
        self.target = target;
        self
    }

    /// The plugin this instance runs.
    pub fn plugin(&self) -> &Arc<dyn Plugin> {
        &self.plugin
    }

    /// The env this instance runs against.
    pub fn env(&self) -> &FuneraEnv {
        &self.env
    }
}

impl PluginInstance<state::Pending> {
    pub(crate) fn create(id: InstanceId, plugin: Arc<dyn Plugin>, env: FuneraEnv) -> Self {
        Self::new(id, plugin, env, None, None)
    }

    /// Move into [`state::Loading`], committing the currently resolved target.
    pub fn begin_load(self) -> PluginInstance<state::Loading> {
        let committed = self.target;
        self.with_state::<state::Loading>()
            .with_committed(committed)
    }
}

impl PluginInstance<state::Loading> {
    fn with_committed(mut self, committed: Option<TargetDigest>) -> Self {
        self.committed = committed;
        self
    }

    /// Mark the load successful.
    pub fn finish_load(self) -> PluginInstance<state::Active> {
        self.with_state()
    }

    /// Mark the load failed. The caller is responsible for rolling back the
    /// instance's effects before calling this.
    pub fn fail(self) -> PluginInstance<state::Failed> {
        self.with_state()
    }

    /// Target changed while loading: chain into unloading.
    pub fn chain_unload(self) -> PluginInstance<state::Unloading> {
        self.with_state()
    }
}

impl PluginInstance<state::Active> {
    /// Move into [`state::Unloading`] (used before effects are disposed).
    pub fn begin_unload(self) -> PluginInstance<state::Unloading> {
        self.with_state()
    }
}

impl PluginInstance<state::Unloading> {
    /// Move into [`state::Inactive`] once effects have been disposed.
    pub fn finish_unload(self) -> PluginInstance<state::Inactive> {
        self.with_state()
    }

    /// Target reappeared while unloading: chain back into loading.
    pub fn chain_load(self) -> PluginInstance<state::Loading> {
        self.with_state()
    }
}

impl PluginInstance<state::Inactive> {
    /// Move back to [`state::Pending`] so the next refresh re-evaluates it.
    pub fn to_pending(self) -> PluginInstance<state::Pending> {
        self.with_state()
    }
}

impl PluginInstance<state::Failed> {
    /// Move back to [`state::Pending`] for a retry.
    pub fn to_pending(self) -> PluginInstance<state::Pending> {
        self.with_state()
    }
}
