//! Typestate-driven plugin registry.

use std::any::TypeId;
use std::collections::HashMap;
use std::sync::Arc;

use crate::env::FuneraEnv;
use crate::plugin::Plugin;
use crate::plugin::instance::{InstanceId, PluginInstance, TargetDigest, state};

/// A failed instance: the instance itself plus its failure record.
pub struct FailedEntry {
    pub inst: PluginInstance<state::Failed>,
    pub error: String,
}

/// Read-only view of a plugin instance's lifecycle phase.
///
/// This is only used for diagnostics and tests; transitions are not selected
/// on this enum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginPhase {
    Pending,
    Loading,
    Active,
    Unloading,
    Inactive,
    Failed(String),
}

/// Resolves an instance's `inject` requirements against the shared service
/// table. `Some(target)` means every required service is currently provided.
fn resolve(env: &FuneraEnv, inject: &[TypeId]) -> Option<TargetDigest> {
    if inject.is_empty() {
        return Some(TargetDigest::from_type_ids(std::iter::empty()));
    }
    for type_id in inject {
        if !env.contains_typeid(*type_id) {
            return None;
        }
    }
    Some(TargetDigest::from_type_ids(inject.iter()))
}

/// A registry that mounts [`Plugin`]s and drives their lifecycle reactively.
///
/// Instances are stored in per-state maps, so their state lives in the map key
/// type rather than in a runtime enum. `Loading` and `Unloading` are transient
/// states represented by the typestate instance during an `await`.
pub struct PluginRegistry {
    env: FuneraEnv,
    pending: HashMap<InstanceId, PluginInstance<state::Pending>>,
    active: HashMap<InstanceId, PluginInstance<state::Active>>,
    inactive: HashMap<InstanceId, PluginInstance<state::Inactive>>,
    failed: HashMap<InstanceId, FailedEntry>,
    next_id: InstanceId,
}

impl PluginRegistry {
    /// Create a registry rooted at `env`.
    pub fn new(env: FuneraEnv) -> Self {
        Self {
            env,
            pending: HashMap::new(),
            active: HashMap::new(),
            inactive: HashMap::new(),
            failed: HashMap::new(),
            next_id: 0,
        }
    }

    /// The root env (shared service table).
    pub fn env(&self) -> &FuneraEnv {
        &self.env
    }

    /// Mount a plugin, returning its instance id.
    ///
    /// The instance starts in [`state::Pending`]; call
    /// [`refresh`](Self::refresh) to (re)evaluate it against the current
    /// services.
    pub fn mount(&mut self, plugin: Arc<dyn Plugin>) -> InstanceId {
        let id = self.next_id;
        self.next_id += 1;
        let child = self.env.derive();
        let inst = PluginInstance::create(id, plugin, child);
        self.pending.insert(id, inst);
        id
    }

    /// Re-evaluate every instance until no further transition fires.
    ///
    /// This is the async, typestate version of the previous `refresh`. Each
    /// transition is a method on a concrete state, so invalid transitions do
    /// not compile.
    pub async fn refresh(&mut self) {
        // Bound by the number of instances plus one; cycles cannot loop
        // forever because a deactivated instance goes through `Inactive`
        // before becoming `Pending` again.
        let cap = self.pending.len() + self.active.len() + self.inactive.len() + 1;
        for _ in 0..cap {
            let mut changed = false;

            // Pending -> Loading -> Active / Failed
            let pending_ids: Vec<InstanceId> = self.pending.keys().copied().collect();
            for id in pending_ids {
                let target = {
                    let inst = &self.pending[&id];
                    resolve(&self.env, inst.plugin.inject())
                };
                if target.is_none() {
                    self.pending.get_mut(&id).unwrap().target = None;
                    continue;
                }
                changed = true;
                let inst = self.pending.remove(&id).unwrap().with_target(target);
                let inst = inst.begin_load();
                let plugin = inst.plugin.clone();
                let env = inst.env.clone();
                let result = plugin.apply(&env).await;
                match result {
                    Ok(()) => {
                        let inst = inst.finish_load();
                        self.active.insert(id, inst);
                    }
                    Err(e) => {
                        // Roll back any effects the failed apply already
                        // registered, then record the failure.
                        env.dispose();
                        self.failed.insert(
                            id,
                            FailedEntry {
                                inst: inst.fail(),
                                error: e.to_string(),
                            },
                        );
                    }
                }
            }

            // Active -> Unloading -> Inactive
            let active_ids: Vec<InstanceId> = self.active.keys().copied().collect();
            for id in active_ids {
                let target = {
                    let inst = &self.active[&id];
                    resolve(&self.env, inst.plugin.inject())
                };
                if target.is_some() {
                    self.active.get_mut(&id).unwrap().target = target;
                    continue;
                }
                changed = true;
                let inst = self.active.remove(&id).unwrap();
                let inst = inst.begin_unload();
                let env = inst.env.clone();
                env.dispose();
                let inst = inst.finish_unload();
                self.inactive.insert(id, inst);
            }

            // Inactive -> Pending (dependency may have reappeared)
            let inactive_ids: Vec<InstanceId> = self.inactive.keys().copied().collect();
            for id in inactive_ids {
                let target = {
                    let inst = &self.inactive[&id];
                    resolve(&self.env, inst.plugin.inject())
                };
                if target.is_none() {
                    self.inactive.get_mut(&id).unwrap().target = None;
                    continue;
                }
                changed = true;
                let inst = self.inactive.remove(&id).unwrap().with_target(target);
                self.pending.insert(id, inst.to_pending());
            }

            if !changed {
                break;
            }
        }
    }

    /// Unmount an instance, reverting its effects and refreshing dependents.
    ///
    /// Returns the unmounted plugin, or `None` if `id` is unknown.
    pub async fn unmount(&mut self, id: InstanceId) -> Option<Arc<dyn Plugin>> {
        let removed = if let Some(inst) = self.pending.remove(&id) {
            Some(inst.plugin.clone())
        } else if let Some(inst) = self.inactive.remove(&id) {
            Some(inst.plugin.clone())
        } else if let Some(entry) = self.failed.remove(&id) {
            Some(entry.inst.plugin.clone())
        } else if let Some(inst) = self.active.remove(&id) {
            let inst = inst.begin_unload();
            let env = inst.env.clone();
            env.dispose();
            Some(inst.plugin.clone())
        } else {
            None
        };

        if removed.is_some() {
            self.refresh().await;
        }
        removed
    }

    /// Current lifecycle phase of an instance.
    ///
    /// This is a diagnostic view; it does not drive transitions.
    pub fn phase(&self, id: InstanceId) -> Option<PluginPhase> {
        if self.pending.contains_key(&id) {
            return Some(PluginPhase::Pending);
        }
        if self.active.contains_key(&id) {
            return Some(PluginPhase::Active);
        }
        if self.inactive.contains_key(&id) {
            return Some(PluginPhase::Inactive);
        }
        if let Some(entry) = self.failed.get(&id) {
            return Some(PluginPhase::Failed(entry.error.clone()));
        }
        None
    }

    /// Number of mounted instances.
    pub fn instance_count(&self) -> usize {
        self.pending.len() + self.active.len() + self.inactive.len() + self.failed.len()
    }
}
