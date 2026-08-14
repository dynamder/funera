# Changelog

All notable changes to funera are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- Unified `Plugin` abstraction (`name` / `inject` / `provides` / `apply`), with
  `Tool`, `ChatProvider`, `InspectorMiddleware`, and `MutatorMiddleware` as
  subtraits of it.
- `FuneraEnv` capability layer: reversible effects (`effect` / `dispose`) and
  typed services (`provide` / `get` / `contains`), plus `derive()` for child
  envs and a `ServiceObserver` notification primitive.
- Reactive `PluginRegistry`: mounts plugins as `PluginInstance`s on derived
  envs and drives the `Pending → Loading → Active → Unloading → Disposed/Failed`
  lifecycle, with partial-effect rollback on `apply` failure.
- Declarative `Loader`: reconciles a desired `PluginEntry` set against the
  registry with minimal mount/unmount operations, and hot-replaces an entry on
  a `revision` bump.
- `funera-orchestrate/examples/plugin_architecture.rs` — an end-to-end,
  no-LLM walkthrough of the reactive lifecycle.
- `impl_plugin!` macro for the one-line `Plugin` supertrait migration.

### Changed

- Re-export `Plugin`, `PluginRegistry`, `PluginInstance`, `InstanceState`,
  `Loader`, and `PluginEntry` from `funera-orchestrate`.
- Made `nono` an optional dependency, enabled only by the `sandbox` feature
  (it was previously pulled in unconditionally on non-Windows targets).

## [0.2.6] - 2026-07-25

Anchor release; see the GitHub release notes for this version.
