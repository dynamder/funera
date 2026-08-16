//! Service keys for the typed service table.

use std::any::{Any, TypeId};
use std::sync::Arc;

/// Unique identifier of a plugin instance that provides services.
///
/// Provider ids are allocated monotonically by the plugin registry and are
/// never reused, so replacing a provider cannot be mistaken for the old one.
pub type ProviderId = u64;

/// Generation counter incremented on every service-table write.
pub type Generation = u64;

/// A key that identifies one slot in the typed service table.
///
/// The primary key for type `T` is `ServiceKey::primary::<T>()`; it carries no
/// name and keeps the existing `get::<T>()` / `provide::<T>()` fast path
/// allocation-free. Named keys (`ServiceKey::named::<T>("...")`) allow several
/// services of the same Rust type to coexist under different names.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ServiceKey {
    pub type_id: TypeId,
    pub name: Option<Arc<str>>,
}

impl ServiceKey {
    /// The unnamed primary slot for `T`.
    #[inline]
    pub fn primary<T: Send + Sync + 'static>() -> Self {
        Self {
            type_id: TypeId::of::<T>(),
            name: None,
        }
    }

    /// A named slot for `T`.
    #[inline]
    pub fn named<T: Send + Sync + 'static>(name: impl Into<Arc<str>>) -> Self {
        Self {
            type_id: TypeId::of::<T>(),
            name: Some(name.into()),
        }
    }
}

impl std::fmt::Debug for ServiceKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.name {
            Some(name) => write!(f, "ServiceKey({name} @ {:?})", self.type_id),
            None => write!(f, "ServiceKey({:?})", self.type_id),
        }
    }
}

/// A binding in the service table: the value together with the provider that
/// installed it and the write generation it was installed at.
pub struct ServiceBinding {
    pub provider: ProviderId,
    pub value: Arc<dyn Any + Send + Sync>,
    pub generation: Generation,
}

impl ServiceBinding {
    pub fn new(
        provider: ProviderId,
        value: Arc<dyn Any + Send + Sync>,
        generation: Generation,
    ) -> Self {
        Self {
            provider,
            value,
            generation,
        }
    }
}
