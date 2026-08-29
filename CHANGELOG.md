# Changelog

All notable changes to funera are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- Cancellation — every entry point (`fire` → `FireHandle`, `fire_stream`,
  `send`, `send_stream`) carries a per-call `CancellationToken`; `cancel()`
  or dropping the handle immediately stops receiving output, abandons
  in-flight tool executions (workers survive), and notifies middleware via
  `AgentEvent::Cancelled` (never written to session history). `fire` now
  returns a `FireHandle` — `await` it for the `ChatResponse`.
- `serde` feature on `funera-core` — gated `Serialize`/`Deserialize` derives
  for the chat message types (`FuneraMessage`, `MsgVariant`, `Role`,
  `TextMessage`, `ToolRequestMessage`, `ToolResponseMessage`).
- `AgentRuntime::session_id` — stable conversation id reused by `send` /
  `send_stream`; `fire` / `fire_stream` keep a fresh one-shot id (fork
  semantics).
- Token usage tracking — `TokenUsage` + `TokenEvent::Usage`; requests ask for
  `stream_options.include_usage`; `ChatResponse.usage` carries the last turn's
  usage. Cost computation is left to callers.
- Parallel tool execution — the tool bus fans out to `max_concurrent_tools`
  (default 4) workers via `AgentRuntimeBuilder::max_concurrent_tools`, so
  multiple tool calls in one turn run concurrently.
- Reasoning levels — `ReasoningLevel`
  (`Off`/`Minimal`/`Low`/`Medium`/`High`/`XHigh`/`Max`, default `Medium`) with
  hot-reload via `AgentRuntime::set_reasoning_level`; providers interpret it
  (DeepSeek mirrors the official harness: `thinking` + `reasoning_effort` for
  `high`/`max`; OpenAI maps to `reasoning_effort`, clamping `xhigh`/`max` to
  `high`).
- `Tool::is_shell_tool` marker (default `false`) — tools that execute shell
  commands opt in explicitly, and `ShellPolicy` scrutiny is now driven by the
  marker instead of a hard-coded tool-name list
  (`shell`/`bash`/`sh`/`cmd`/`powershell`). `ToolPolicy::check_shell_command`
  now takes the tool itself.
- `FuneraEnv::effect` / `dispose` — reversible effects with LIFO teardown: every
  registration can be paired with its inverse, and disposal runs all inverses in
  reverse registration order (idempotent and panic-isolated). `EnvActor` calls
  `dispose()` automatically when the runtime is dropped, so registrations never
  leak memory or services.
- `remove_tool_if_same` on the tool registries and `FuneraEnv` — the safe
  inverse of `add_tool` for a disposer (removes a tool only if the registered
  entry is the same `Arc`, so a stale teardown cannot delete a replacement that
  reuses the same name).
- `funera-orchestrate/examples/reversible_effects.rs` — a no-LLM walkthrough of
  LIFO teardown and leak-safe registration.
- Open-source project infrastructure: CI and audit workflows, issue/PR
  templates, `CODEOWNERS`, code of conduct, contributing guide, security
  policy, changelog, mdBook documentation skeleton, `deny.toml` / `release.toml`
  tooling config, and MSRV 1.88 declaration.

### Changed

- Tools are stored and executed behind `Arc<dyn Tool>`; without the `security`
  feature the executor runs a tool outside the registry lock, and with it the
  guarded registry is cloned so policy/audit run against a snapshot. Slow tools
  no longer block dynamic add/remove/availability changes.
- Made `nono` an optional dependency, enabled only by the `sandbox` feature
  (it was previously pulled in unconditionally on non-Windows targets).
- `AgentRuntime::with_tool_instance` / `add_tool` now take `Arc<dyn Tool>`
  instead of `Box<dyn Tool>`.

### Removed

- The unified `Plugin` abstraction and its machinery: `PluginRegistry`,
  `PluginInstance`, typestate lifecycle, `Loader` with declarative config and
  hot module replacement, `ServiceBroker`, `MiddlewareProcessor` type erasure,
  and `AgentLoop` as a plugin subtrait. The core is back to plain extensible
  traits (`Tool`, `ChatProvider`, `InspectorMiddleware`, `MutatorMiddleware`)
  with the actor-based runtime as the single mutation owner.

## [0.2.6] - 2026-07-25

Anchor release; see the GitHub release notes for this version.
