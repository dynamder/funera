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
use crate::plugin::{Plugin, PluginConfig, PluginPhase, PluginRegistry};

/// How the loader hot-replaces an entry whose `revision` changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum HmrPolicy {
    /// Mount the new revision first; only after it reaches `Active` is the old
    /// instance unmounted. If the new revision fails, it is rolled back and the
    /// old instance stays in place.
    #[default]
    Replace,
    /// Unmount the old revision first, then mount the new one. This has a
    /// small availability window but is simpler and always consistent.
    Swap,
}

/// A declarative entry describing one desired plugin instance.
#[derive(Clone)]
pub struct PluginEntry {
    /// Stable id, used as the reconciliation key.
    pub id: String,
    /// The plugin to load.
    pub plugin: Arc<dyn Plugin>,
    /// Optional config passed to [`Plugin::apply`](Plugin::apply).
    pub config: Option<Arc<PluginConfig>>,
    /// Administrative switch: disabled entries stay in config but are not loaded.
    pub disabled: bool,
    /// Opaque generation marker; a changed value triggers a reload (HMR).
    pub revision: u64,
    /// Hot-replacement strategy used when `revision` changes.
    pub hmr: HmrPolicy,
}

impl PluginEntry {
    /// Create an enabled, revision-0 entry.
    pub fn new(id: impl Into<String>, plugin: Arc<dyn Plugin>) -> Self {
        Self {
            id: id.into(),
            plugin,
            config: None,
            disabled: false,
            revision: 0,
            hmr: HmrPolicy::Replace,
        }
    }

    /// Set the hot-replacement strategy (default: [`HmrPolicy::Replace`]).
    pub fn hmr(mut self, hmr: HmrPolicy) -> Self {
        self.hmr = hmr;
        self
    }

    /// Attach a config that is passed to the plugin's `apply` hook.
    pub fn config(mut self, config: PluginConfig) -> Self {
        self.config = Some(Arc::new(config));
        self
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

/// The result of a [`Loader::reconcile`] pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReconcileReport {
    /// Ids mounted for the first time.
    pub loaded: Vec<String>,
    /// Ids that were removed because they vanished or were disabled.
    pub unloaded: Vec<String>,
    /// Ids whose `revision` changed and were hot-replaced.
    pub reloaded: Vec<String>,
    /// Ids that could not be reconciled, with an error string.
    pub failed: Vec<(String, String)>,
    /// Ids skipped (disabled entries that remain disabled).
    pub skipped: Vec<String>,
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
    /// Applies the minimal set of changes: unmounts entries that vanished or
    /// were disabled; hot-replaces entries whose `revision` changed; mounts
    /// entries that are desired, enabled, and not already mounted.
    pub async fn reconcile(&mut self, entries: &[PluginEntry]) -> ReconcileReport {
        let mut report = ReconcileReport::default();

        // Phase 1: drop entries that vanished or were disabled. Revision
        // changes are left mounted and handled by phase 3 (HMR).
        let ids: Vec<String> = self.mounted.keys().cloned().collect();
        for id in ids {
            let desired = entries.iter().find(|e| e.id == id);
            let vanished_or_disabled = desired.is_none_or(|e| e.disabled);
            if vanished_or_disabled {
                let (inst, _) = self.mounted.remove(&id).unwrap();
                self.registry.unmount(inst).await;
                report.unloaded.push(id.clone());
            }
        }

        // Phase 2: mount entries that are desired, enabled, and not yet mounted.
        for entry in entries {
            if entry.disabled {
                if !self.mounted.contains_key(&entry.id) {
                    report.skipped.push(entry.id.clone());
                }
                continue;
            }
            if self.mounted.contains_key(&entry.id) {
                continue;
            }
            let inst = self
                .registry
                .mount_with_config(entry.plugin.clone(), entry.config.clone());
            self.mounted
                .insert(entry.id.clone(), (inst, entry.revision));
            report.loaded.push(entry.id.clone());
        }

        // Phase 3: hot-replace entries whose revision changed.
        let changed: Vec<String> = self
            .mounted
            .iter()
            .filter_map(|(id, (_, rev))| {
                let desired = entries.iter().find(|e| e.id == *id)?;
                if desired.revision != *rev {
                    Some(id.clone())
                } else {
                    None
                }
            })
            .collect();

        for id in changed {
            let entry = entries.iter().find(|e| e.id == id).unwrap().clone();
            let (old_inst, old_rev) = self.mounted.remove(&id).unwrap();

            match entry.hmr {
                HmrPolicy::Swap => {
                    self.registry.unmount(old_inst).await;
                    let new_inst = self
                        .registry
                        .mount_with_config(entry.plugin.clone(), entry.config.clone());
                    self.mounted.insert(id.clone(), (new_inst, entry.revision));
                    report.reloaded.push(id.clone());
                }
                HmrPolicy::Replace => {
                    let new_inst = self
                        .registry
                        .mount_with_config(entry.plugin.clone(), entry.config.clone());
                    self.registry.refresh().await;
                    if self.registry.phase(new_inst) == Some(PluginPhase::Active) {
                        // New revision is live; now retire the old one.
                        self.registry.unmount(old_inst).await;
                        self.mounted.insert(id.clone(), (new_inst, entry.revision));
                        report.reloaded.push(id.clone());
                    } else {
                        // Roll back: discard the new revision, keep the old one.
                        let new_phase = self.registry.phase(new_inst);
                        self.registry.unmount(new_inst).await;
                        self.mounted.insert(id.clone(), (old_inst, old_rev));
                        report.failed.push((
                            id.clone(),
                            format!("new revision failed to activate: {new_phase:?}"),
                        ));
                    }
                }
            }
        }

        self.registry.refresh().await;
        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::PluginError;
    use async_trait::async_trait;
    use serde_json::json;
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
        let report = l
            .reconcile(&[
                PluginEntry::new("a", noop("a")),
                PluginEntry::new("b", noop("b")),
            ])
            .await;
        assert_eq!(report.loaded.len(), 2);
        assert!(report.unloaded.is_empty());
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
        let report = l.reconcile(&entries).await;
        assert!(report.loaded.is_empty(), "unchanged config loads nothing");
        assert_eq!(l.registry().instance_count(), 2);
    }

    #[tokio::test]
    async fn reconcile_removes_vanished_entries() {
        let mut l = loader();
        let a = PluginEntry::new("a", noop("a"));
        let b = PluginEntry::new("b", noop("b"));
        l.reconcile(&[a.clone(), b]).await;

        let report = l.reconcile(&[a]).await;
        assert_eq!(report.unloaded, vec!["b".to_string()]);
        assert_eq!(l.registry().instance_count(), 1);
        assert_eq!(l.mounted_count(), 1);
    }

    #[tokio::test]
    async fn reconcile_skips_disabled_entries() {
        let mut l = loader();
        let a = PluginEntry::new("a", noop("a"));
        let b = PluginEntry::new("b", noop("b")).disabled(true);
        let report = l.reconcile(&[a, b]).await;
        assert_eq!(report.loaded, vec!["a".to_string()]);
        assert_eq!(report.skipped, vec!["b".to_string()]);
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
            async fn apply(
                &self,
                _env: &FuneraEnv,
                _config: Option<&PluginConfig>,
            ) -> Result<(), PluginError> {
                APPLIES.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }

        let mut l = loader();
        l.reconcile(&[PluginEntry::new("p", Arc::new(Counting)).revision(1)])
            .await;
        assert_eq!(APPLIES.load(Ordering::SeqCst), 1);

        // Same id, new revision → hot replacement.
        let report = l
            .reconcile(&[PluginEntry::new("p", Arc::new(Counting)).revision(2)])
            .await;
        assert_eq!(report.reloaded, vec!["p".to_string()]);
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
            async fn apply(
                &self,
                env: &FuneraEnv,
                _config: Option<&PluginConfig>,
            ) -> Result<(), PluginError> {
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

    #[tokio::test]
    async fn replace_hmr_rolls_back_failed_revision() {
        struct MaybeFail;
        #[async_trait]
        impl Plugin for MaybeFail {
            fn name(&self) -> &str {
                "maybe_fail"
            }
            async fn apply(
                &self,
                _env: &FuneraEnv,
                config: Option<&PluginConfig>,
            ) -> Result<(), PluginError> {
                let fail = config
                    .and_then(|c| c.value().get("fail"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if fail {
                    Err("configured to fail".into())
                } else {
                    Ok(())
                }
            }
        }

        let mut l = loader();
        l.reconcile(&[PluginEntry::new("p", Arc::new(MaybeFail))
            .revision(1)
            .config(PluginConfig::new(json!({"fail": false})))])
            .await;
        assert_eq!(l.entry_phase("p"), Some(PluginPhase::Active));

        let report = l
            .reconcile(&[PluginEntry::new("p", Arc::new(MaybeFail))
                .revision(2)
                .config(PluginConfig::new(json!({"fail": true})))])
            .await;

        assert_eq!(report.failed.len(), 1, "new revision should fail");
        assert_eq!(report.reloaded.len(), 0);
        assert_eq!(l.entry_phase("p"), Some(PluginPhase::Active));
        assert_eq!(l.registry().instance_count(), 1, "old instance must stay");
    }

    #[tokio::test]
    async fn config_is_passed_to_apply() {
        static SEEN: AtomicUsize = AtomicUsize::new(0);

        struct ConfigReader;
        #[async_trait]
        impl Plugin for ConfigReader {
            fn name(&self) -> &str {
                "config_reader"
            }
            async fn apply(
                &self,
                _env: &FuneraEnv,
                config: Option<&PluginConfig>,
            ) -> Result<(), PluginError> {
                if let Some(cfg) = config {
                    if cfg.value().get("k").and_then(|v| v.as_u64()) == Some(7) {
                        SEEN.fetch_add(1, Ordering::SeqCst);
                    }
                }
                Ok(())
            }
        }

        let mut l = loader();
        l.reconcile(&[PluginEntry::new("p", Arc::new(ConfigReader))
            .config(PluginConfig::new(json!({"k": 7})))])
            .await;
        assert_eq!(SEEN.load(Ordering::SeqCst), 1);
    }
}
