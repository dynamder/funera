//! # Declarative plugin loader
//!
//! The core library gives *plugin authors* imperative primitives (`mount`,
//! `provide`, `effect`). The loader gives *orchestrators* a declarative layer:
//! you describe the desired plugin set as a list of [`PluginEntry`]s, and
//! [`Loader::reconcile`] translates the diff against the current set into the
//! minimal set of mount/unmount operations.
//!
//! A changed `revision` on an existing id is a hot replacement (HMR): the old
//! instance is unmounted (its effects reverted) and a fresh one is mounted.

use std::collections::HashMap;
use std::sync::Arc;

use crate::env::FuneraEnv;
use crate::plugin::{Plugin, PluginPhase, PluginRegistry};

/// A declarative entry describing one desired plugin instance.
#[derive(Clone)]
pub struct PluginEntry {
    /// Stable id, used as the reconciliation key.
    pub id: String,
    /// The plugin to load.
    pub plugin: Arc<dyn Plugin>,
    /// Administrative switch: disabled entries stay in config but are not loaded.
    pub disabled: bool,
    /// Opaque generation marker; a changed value triggers a reload (HMR).
    pub revision: u64,
}

impl PluginEntry {
    /// Create an enabled, revision-0 entry.
    pub fn new(id: impl Into<String>, plugin: Arc<dyn Plugin>) -> Self {
        Self {
            id: id.into(),
            plugin,
            disabled: false,
            revision: 0,
        }
    }

    /// Mark the entry administratively disabled (or enabled).
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Set the generation marker (a change triggers a reload).
    pub fn revision(mut self, revision: u64) -> Self {
        self.revision = revision;
        self
    }
}

/// Keeps a [`PluginRegistry`] in step with a declarative plugin set.
pub struct Loader {
    registry: PluginRegistry,
    /// entry id -> (instance id, revision).
    mounted: HashMap<String, (u64, u64)>,
}

impl Loader {
    /// Create a loader rooted at `env`.
    pub fn new(env: FuneraEnv) -> Self {
        Self {
            registry: PluginRegistry::new(env),
            mounted: HashMap::new(),
        }
    }

    /// The underlying registry.
    pub fn registry(&self) -> &PluginRegistry {
        &self.registry
    }

    /// Mutable access to the underlying registry.
    pub fn registry_mut(&mut self) -> &mut PluginRegistry {
        &mut self.registry
    }

    /// Number of currently mounted entries.
    pub fn mounted_count(&self) -> usize {
        self.mounted.len()
    }

    /// Lifecycle phase of the mounted entry with the given id.
    ///
    /// Returns `None` if the id is not currently mounted.
    pub fn entry_phase(&self, id: &str) -> Option<PluginPhase> {
        let (inst, _) = self.mounted.get(id)?;
        self.registry.phase(*inst)
    }

    /// Reconcile the registry against `entries`.
    ///
    /// Applies the minimal set of changes: unmounts entries that vanished, were
    /// disabled, or changed `revision`; mounts entries that are desired, enabled,
    /// and not already mounted. Returns the ids that were (re)loaded.
    pub async fn reconcile(&mut self, entries: &[PluginEntry]) -> Vec<String> {
        let mut loaded = Vec::new();

        // Phase 1: drop entries that vanished, were disabled, or changed revision.
        let ids: Vec<String> = self.mounted.keys().cloned().collect();
        for id in ids {
            let current_rev = self.mounted[&id].1;
            let keep = entries
                .iter()
                .any(|e| e.id == id && !e.disabled && e.revision == current_rev);
            if !keep {
                let (inst, _) = self.mounted.remove(&id).unwrap();
                self.registry.unmount(inst).await;
            }
        }

        // Phase 2: mount entries that are desired, enabled, and not yet mounted.
        for entry in entries {
            if entry.disabled || self.mounted.contains_key(&entry.id) {
                continue;
            }
            let inst = self.registry.mount(entry.plugin.clone());
            self.mounted
                .insert(entry.id.clone(), (inst, entry.revision));
            loaded.push(entry.id.clone());
        }

        self.registry.refresh().await;
        loaded
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::PluginError;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn loader() -> Loader {
        let (env, _watcher) = FuneraEnv::new(async_openai::Client::new(), "test-model");
        Loader::new(env)
    }

    fn noop(name: &str) -> Arc<dyn Plugin> {
        struct P(String);
        impl Plugin for P {
            fn name(&self) -> &str {
                &self.0
            }
        }
        Arc::new(P(name.to_string()))
    }

    #[tokio::test]
    async fn reconcile_mounts_entries() {
        let mut l = loader();
        let loaded = l
            .reconcile(&[
                PluginEntry::new("a", noop("a")),
                PluginEntry::new("b", noop("b")),
            ])
            .await;
        assert_eq!(loaded.len(), 2);
        assert_eq!(l.registry().instance_count(), 2);
        assert_eq!(l.mounted_count(), 2);
    }

    #[tokio::test]
    async fn reconcile_is_idempotent() {
        let mut l = loader();
        let entries = vec![
            PluginEntry::new("a", noop("a")),
            PluginEntry::new("b", noop("b")),
        ];
        l.reconcile(&entries).await;
        let loaded_again = l.reconcile(&entries).await;
        assert!(loaded_again.is_empty(), "unchanged config loads nothing");
        assert_eq!(l.registry().instance_count(), 2);
    }

    #[tokio::test]
    async fn reconcile_removes_vanished_entries() {
        let mut l = loader();
        let a = PluginEntry::new("a", noop("a"));
        let b = PluginEntry::new("b", noop("b"));
        l.reconcile(&[a.clone(), b]).await;

        l.reconcile(&[a]).await;
        assert_eq!(l.registry().instance_count(), 1);
        assert_eq!(l.mounted_count(), 1);
    }

    #[tokio::test]
    async fn reconcile_skips_disabled_entries() {
        let mut l = loader();
        let a = PluginEntry::new("a", noop("a"));
        let b = PluginEntry::new("b", noop("b")).disabled(true);
        l.reconcile(&[a, b]).await;
        assert_eq!(l.registry().instance_count(), 1);
    }

    #[tokio::test]
    async fn reconcile_reloads_on_revision_change() {
        static APPLIES: AtomicUsize = AtomicUsize::new(0);

        struct Counting;
        #[async_trait]
        impl Plugin for Counting {
            fn name(&self) -> &str {
                "counting"
            }
            async fn apply(&self, _env: &FuneraEnv) -> Result<(), PluginError> {
                APPLIES.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }

        let mut l = loader();
        l.reconcile(&[PluginEntry::new("p", Arc::new(Counting)).revision(1)])
            .await;
        assert_eq!(APPLIES.load(Ordering::SeqCst), 1);

        // Same id, new revision → hot replacement.
        let reloaded = l
            .reconcile(&[PluginEntry::new("p", Arc::new(Counting)).revision(2)])
            .await;
        assert_eq!(reloaded, vec!["p".to_string()]);
        assert_eq!(
            APPLIES.load(Ordering::SeqCst),
            2,
            "reload must re-run apply"
        );
        assert_eq!(
            l.registry().instance_count(),
            1,
            "reload must not duplicate"
        );
    }

    #[tokio::test]
    async fn entry_phase_reports_lifecycle() {
        let mut l = loader();
        l.reconcile(&[PluginEntry::new("a", noop("a"))]).await;
        assert_eq!(l.entry_phase("a"), Some(PluginPhase::Active));
        assert!(l.entry_phase("missing").is_none());
    }

    #[tokio::test]
    async fn reload_reverts_old_effects() {
        static LIVE: AtomicUsize = AtomicUsize::new(0);

        struct Effectful;
        #[async_trait]
        impl Plugin for Effectful {
            fn name(&self) -> &str {
                "effectful"
            }
            async fn apply(&self, env: &FuneraEnv) -> Result<(), PluginError> {
                LIVE.fetch_add(1, Ordering::SeqCst);
                env.effect(|| {
                    Box::new(|| {
                        LIVE.fetch_sub(1, Ordering::SeqCst);
                    })
                });
                Ok(())
            }
        }

        let mut l = loader();
        l.reconcile(&[PluginEntry::new("p", Arc::new(Effectful)).revision(1)])
            .await;
        assert_eq!(LIVE.load(Ordering::SeqCst), 1);

        l.reconcile(&[PluginEntry::new("p", Arc::new(Effectful)).revision(2)])
            .await;
        assert_eq!(
            LIVE.load(Ordering::SeqCst),
            1,
            "old effects reverted, new applied"
        );
    }
}
