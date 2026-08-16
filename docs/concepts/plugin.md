# Plugin Lifecycle

`Plugin` is the unified abstraction:

```rust,no_run
# use funera_core::env::FuneraEnv;
# use funera_core::plugin::{Plugin, PluginConfig, PluginError};
# use async_trait::async_trait;
#[async_trait]
pub trait Plugin: Send + Sync {
    fn name(&self) -> &str;
    fn inject(&self) -> &[std::any::TypeId] { &[] }
    fn provides(&self) -> &[std::any::TypeId] { &[] }
    async fn apply(&self, env: &FuneraEnv, config: Option<&PluginConfig>) -> Result<(), PluginError>;
}
```

## States

- `Pending` — declared, dependencies not satisfied
- `Loading` — `apply` is running
- `Active` — live
- `Unloading` — disposers are running
- `Inactive` — torn down, can reactivate
- `Failed` — apply failed; effects rolled back

## HMR

Bump `revision` in a `PluginEntry` to hot-replace an instance. `HmrPolicy::Replace` mounts the
new instance first and only removes the old one after the new one is `Active`.
