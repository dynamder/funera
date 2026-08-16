//! Service broker for multi-provider service multiplexing.

use std::any::TypeId;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use parking_lot::RwLock as StdRwLock;

use crate::env::FuneraEnv;
use crate::plugin::{Plugin, PluginConfig, PluginError};

/// A round-robin broker for services of type `T`.
///
/// Providers register their service with the broker; consumers inject the
/// broker from the env and call [`next`](ServiceBroker::next) to obtain a
/// provider. The broker is `Clone` and cheap to share.
pub struct ServiceBroker<T> {
    inner: Arc<BrokerInner<T>>,
}

struct BrokerInner<T> {
    providers: StdRwLock<Vec<Arc<T>>>,
    next: AtomicUsize,
}

impl<T: Send + Sync + 'static> Default for ServiceBroker<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Send + Sync + 'static> ServiceBroker<T> {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(BrokerInner {
                providers: StdRwLock::new(Vec::new()),
                next: AtomicUsize::new(0),
            }),
        }
    }

    /// Register a provider service.
    pub fn register(&self, provider: Arc<T>) {
        self.inner.providers.write().push(provider);
    }

    /// Remove a previously registered provider (matched by pointer).
    pub fn unregister(&self, provider: &Arc<T>) {
        let mut providers = self.inner.providers.write();
        if let Some(pos) = providers.iter().position(|p| Arc::ptr_eq(p, provider)) {
            providers.swap_remove(pos);
        }
    }

    /// Return the next provider in round-robin order.
    pub fn next(&self) -> Option<Arc<T>> {
        let providers = self.inner.providers.read();
        if providers.is_empty() {
            return None;
        }
        let idx = self.inner.next.fetch_add(1, Ordering::Relaxed) % providers.len();
        Some(Arc::clone(&providers[idx]))
    }

    /// Number of registered providers.
    pub fn len(&self) -> usize {
        self.inner.providers.read().len()
    }

    /// Whether no provider is registered.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<T: Send + Sync + 'static> Clone for ServiceBroker<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

/// Provides a [`ServiceBroker`] for type `T` to the env.
pub struct BrokerPlugin<T: Send + Sync + 'static> {
    name: String,
    broker: ServiceBroker<T>,
}

impl<T: Send + Sync + 'static> BrokerPlugin<T> {
    pub fn new(broker: ServiceBroker<T>) -> Self {
        Self {
            name: format!("broker:{}", std::any::type_name::<T>()),
            broker,
        }
    }
}

#[async_trait]
impl<T: Send + Sync + 'static> Plugin for BrokerPlugin<T> {
    fn name(&self) -> &str {
        &self.name
    }

    async fn apply(
        &self,
        env: &FuneraEnv,
        _config: Option<&PluginConfig>,
    ) -> Result<(), PluginError> {
        env.provide(self.broker.clone());
        Ok(())
    }
}

/// Registers a service with a [`ServiceBroker`] provided by the env.
pub struct BrokerProvider<T: Send + Sync + 'static> {
    name: String,
    service: Arc<T>,
    inject: Vec<TypeId>,
}

impl<T: Send + Sync + 'static> BrokerProvider<T> {
    pub fn new(name: impl Into<String>, service: Arc<T>) -> Self {
        Self {
            name: name.into(),
            service,
            inject: vec![TypeId::of::<ServiceBroker<T>>()],
        }
    }
}

#[async_trait]
impl<T: Send + Sync + 'static> Plugin for BrokerProvider<T> {
    fn name(&self) -> &str {
        &self.name
    }

    fn inject(&self) -> &[TypeId] {
        &self.inject
    }

    async fn apply(
        &self,
        env: &FuneraEnv,
        _config: Option<&PluginConfig>,
    ) -> Result<(), PluginError> {
        let broker = env
            .get::<ServiceBroker<T>>()
            .ok_or_else(|| format!("{} requires a ServiceBroker service", self.name))?;
        broker.register(Arc::clone(&self.service));

        let teardown_broker = ServiceBroker::clone(&broker);
        let teardown_service = Arc::clone(&self.service);
        env.effect(|| {
            Box::new(move || {
                teardown_broker.unregister(&teardown_service);
            })
        });

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::PluginRegistry;
    use crate::plugin::registry::PluginPhase;

    #[tokio::test]
    async fn broker_round_robins_providers() {
        let (env, _watcher) = FuneraEnv::new(async_openai::Client::new(), "test-model");
        let mut reg = PluginRegistry::new(env);

        let broker = ServiceBroker::<String>::new();
        assert!(broker.is_empty());

        let broker_plugin = BrokerPlugin::new(broker.clone());
        assert_eq!(
            broker_plugin.name(),
            format!("broker:{}", std::any::type_name::<String>())
        );
        let provider_a = BrokerProvider::new("a", Arc::new("a".to_string()));
        assert_eq!(provider_a.name(), "a");
        let provider_b = BrokerProvider::new("b", Arc::new("b".to_string()));
        assert_eq!(provider_b.name(), "b");

        reg.mount(Arc::new(broker_plugin));
        reg.mount(Arc::new(provider_a));
        reg.mount(Arc::new(provider_b));
        reg.refresh().await;

        assert!(!broker.is_empty());
        assert_eq!(broker.len(), 2);
        let first = broker.next().unwrap();
        let second = broker.next().unwrap();
        let third = broker.next().unwrap();
        assert_ne!(first, second);
        assert_eq!(third, first);
    }

    #[tokio::test]
    async fn provider_unmount_unregisters_from_broker() {
        let (env, _watcher) = FuneraEnv::new(async_openai::Client::new(), "test-model");
        let mut reg = PluginRegistry::new(env);

        let broker = ServiceBroker::<String>::new();
        reg.mount(Arc::new(BrokerPlugin::new(broker.clone())));
        let provider = reg.mount(Arc::new(BrokerProvider::new(
            "a",
            Arc::new("a".to_string()),
        )));
        reg.refresh().await;
        assert_eq!(broker.len(), 1);

        reg.unmount(provider).await;
        assert_eq!(broker.len(), 0);
    }
}
