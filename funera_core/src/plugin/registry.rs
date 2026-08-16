//! Typestate-driven plugin registry.

use std::any::TypeId;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use parking_lot::{Mutex as StdMutex, RwLock as StdRwLock};

use crate::env::{FuneraEnv, ProviderId};
use crate::plugin::instance::{InstanceId, PluginInstance, TargetDigest, state};
use crate::plugin::{Plugin, PluginConfig};

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
/// table. A binding only counts while its provider is active (provider id `0`
/// is the root env and is always considered active).
fn resolve(
    env: &FuneraEnv,
    inject: &[TypeId],
    active_providers: &HashMap<ProviderId, InstanceId>,
) -> Option<TargetDigest> {
    let pairs: Vec<(&TypeId, ProviderId)> = inject
        .iter()
        .map(|type_id| {
            let binding = env.binding_by_typeid(*type_id)?;
            if binding.provider != 0 && !active_providers.contains_key(&binding.provider) {
                return None;
            }
            Some((type_id, binding.provider))
        })
        .collect::<Option<_>>()?;

    Some(TargetDigest::from_provider_pairs(
        pairs.iter().map(|(t, p)| (*t, *p)),
    ))
}

/// A registry that mounts [`Plugin`]s and drives their lifecycle reactively.
///
/// Instances are stored in per-state maps, so their state lives in the map key
/// type rather than in a runtime enum. `Loading` and `Unloading` are transient
/// states represented by the typestate instance during an `await`.
///
/// The registry subscribes to the env's service-change notifications and keeps
/// a reverse index (`inject TypeId -> instance ids`), so a `refresh` only
/// re-evaluates instances whose declared dependencies may have changed.
pub struct PluginRegistry {
    env: FuneraEnv,
    pending: HashMap<InstanceId, PluginInstance<state::Pending>>,
    active: HashMap<InstanceId, PluginInstance<state::Active>>,
    inactive: HashMap<InstanceId, PluginInstance<state::Inactive>>,
    failed: HashMap<InstanceId, FailedEntry>,
    next_id: InstanceId,
    next_provider_id: ProviderId,
    /// ProviderId -> InstanceId for providers that are currently `Active`.
    active_providers: HashMap<ProviderId, InstanceId>,
    /// Reverse index from an inject `TypeId` to every instance that requires it.
    dependents: Arc<StdRwLock<HashMap<TypeId, HashSet<InstanceId>>>>,
    /// Instance ids that need re-evaluation.
    dirty: Arc<StdMutex<HashSet<InstanceId>>>,
}

impl PluginRegistry {
    /// Create a registry rooted at `env`.
    pub fn new(env: FuneraEnv) -> Self {
        let dependents: Arc<StdRwLock<HashMap<TypeId, HashSet<InstanceId>>>> =
            Arc::new(StdRwLock::new(HashMap::new()));
        let dirty: Arc<StdMutex<HashSet<InstanceId>>> = Arc::new(StdMutex::new(HashSet::new()));

        // Subscribe before `env` is moved into the registry.
        {
            let dependents = Arc::clone(&dependents);
            let dirty = Arc::clone(&dirty);
            env.on_service_change(Arc::new(move |type_id, _present| {
                if let Some(ids) = dependents.read().get(&type_id) {
                    dirty.lock().extend(ids.iter().copied());
                }
            }));
        }

        Self {
            env,
            pending: HashMap::new(),
            active: HashMap::new(),
            inactive: HashMap::new(),
            failed: HashMap::new(),
            next_id: 0,
            next_provider_id: 0,
            active_providers: HashMap::new(),
            dependents,
            dirty,
        }
    }

    /// The root env (shared service table).
    pub fn env(&self) -> &FuneraEnv {
        &self.env
    }

    /// Mount a plugin with no config, returning its instance id.
    ///
    /// The instance starts in [`state::Pending`] and is marked dirty, so the
    /// next [`refresh`](Self::refresh) re-evaluates it.
    pub fn mount(&mut self, plugin: Arc<dyn Plugin>) -> InstanceId {
        self.mount_with_config(plugin, None)
    }

    /// Mount a plugin with an optional config, returning its instance id.
    pub fn mount_with_config(
        &mut self,
        plugin: Arc<dyn Plugin>,
        config: Option<Arc<PluginConfig>>,
    ) -> InstanceId {
        let id = self.next_id;
        self.next_id += 1;
        let provider_id = self.next_provider_id;
        self.next_provider_id += 1;

        let inject = plugin.inject().to_vec();
        let child = self.env.derive_with_provider(provider_id);
        let inst = PluginInstance::create(id, plugin, config, child);
        self.pending.insert(id, inst);

        // Reverse index for notification-driven refresh.
        for type_id in &inject {
            self.dependents
                .write()
                .entry(*type_id)
                .or_default()
                .insert(id);
        }
        self.dirty.lock().insert(id);
        id
    }

    /// Re-evaluate dirty instances until no further transition fires.
    ///
    /// Each transition is a method on a concrete typestate, so invalid
    /// transitions do not compile. Service changes made during `apply` /
    /// `dispose` mark affected instances dirty automatically through the
    /// service-change observer.
    pub async fn refresh(&mut self) {
        // Safety bound: each pass drains the dirty set; transitions add new
        // dirty ids at most once per instance state change.
        let cap = self.instance_count() * 4 + 4;
        for _ in 0..cap {
            let ids: Vec<InstanceId> = self.dirty.lock().drain().collect();
            if ids.is_empty() {
                break;
            }

            for id in ids {
                if let Some(inst) = self.pending.remove(&id) {
                    let target = resolve(&self.env, inst.plugin.inject(), &self.active_providers);
                    if target.is_none() {
                        self.pending.insert(id, inst);
                        continue;
                    }
                    let inst = inst.with_target(target).begin_load();
                    let plugin = inst.plugin.clone();
                    let config = inst.config.clone();
                    let env = inst.env.clone();
                    let result = plugin.apply(&env, config.as_deref()).await;
                    match result {
                        Ok(()) => {
                            let provider_id = env.provider_id();
                            self.active_providers.insert(provider_id, id);
                            self.active.insert(id, inst.finish_load());
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
                    continue;
                }

                if let Some(inst) = self.active.remove(&id) {
                    let target = resolve(&self.env, inst.plugin.inject(), &self.active_providers);
                    if target.is_some() {
                        self.active.insert(id, inst.with_target(target));
                        continue;
                    }
                    let provider_id = inst.env.provider_id();
                    self.active_providers.remove(&provider_id);
                    let inst = inst.begin_unload();
                    let env = inst.env.clone();
                    env.dispose();
                    self.inactive.insert(id, inst.finish_unload());
                    continue;
                }

                if let Some(inst) = self.inactive.remove(&id) {
                    let target = resolve(&self.env, inst.plugin.inject(), &self.active_providers);
                    if target.is_none() {
                        self.inactive.insert(id, inst);
                        continue;
                    }
                    // Re-enter pending; the next dirty drain will activate it.
                    self.dirty.lock().insert(id);
                    self.pending.insert(id, inst.to_pending());
                    continue;
                }

                // Failed instances are not automatically retried yet.
            }
        }
    }

    /// Unmount an instance, reverting its effects and refreshing dependents.
    ///
    /// Returns the unmounted plugin, or `None` if `id` is unknown.
    pub async fn unmount(&mut self, id: InstanceId) -> Option<Arc<dyn Plugin>> {
        // Remove the instance from the reverse index.
        let inject = self
            .pending
            .get(&id)
            .map(|i| i.plugin.inject().to_vec())
            .or_else(|| self.active.get(&id).map(|i| i.plugin.inject().to_vec()))
            .or_else(|| self.inactive.get(&id).map(|i| i.plugin.inject().to_vec()))
            .or_else(|| {
                self.failed
                    .get(&id)
                    .map(|f| f.inst.plugin.inject().to_vec())
            });
        if let Some(inject) = inject {
            let mut deps = self.dependents.write();
            for type_id in &inject {
                if let Some(ids) = deps.get_mut(type_id) {
                    ids.remove(&id);
                }
            }
        }

        let removed = if let Some(inst) = self.pending.remove(&id) {
            Some(inst.plugin.clone())
        } else if let Some(inst) = self.inactive.remove(&id) {
            Some(inst.plugin.clone())
        } else if let Some(entry) = self.failed.remove(&id) {
            Some(entry.inst.plugin.clone())
        } else if let Some(inst) = self.active.remove(&id) {
            let provider_id = inst.env.provider_id();
            self.active_providers.remove(&provider_id);
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
