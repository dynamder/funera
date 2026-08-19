# Architecture

```text
┌──────────────────────────────────────────────────────────────┐
│  funera-orchestrate    Agent, AgentRuntime                   │
│                        callbacks · streaming · middleware    │
├──────────────────────────────────────────────────────────────┤
│  funera_core           FuneraEnv (hot-reload + reversible    │
│                        effects) · EnvActor · ReActLoop ·     │
│                        SessionActor · EventBus · Middleware  │
│                        · Provider · Tools · Skills · Security│
├──────────────────────────────────────────────────────────────┤
│  funera_builtin_tools  Read, Write, Edit, Shell              │
└──────────────────────────────────────────────────────────────┘
```

## Runtime communication

- `AgentRuntime` is a thin channel wrapper.
- `EnvActor` owns the `FuneraEnv` and processes `EnvCmd`; it also owns the `ToolExecutor`.
- `SessionActor` owns conversation history.
- `ReActLoop` runs one turn, hot-reloading model/client/tools/skills from the env watcher.

## Reversible effects

`FuneraEnv` keeps a per-env accumulator of registered effects. `env.effect(body)` runs `body`
now (setup) and pushes the returned `Disposer` (the inverse) onto the accumulator. When the
env is torn down:

- `EnvActor` calls `FuneraEnv::dispose()` automatically once every `EnvCmd` sender is gone
  (i.e. the runtime is dropped);
- every disposer runs in **reverse registration order** (LIFO);
- disposal is idempotent and panic-isolated — one failing disposer never blocks the rest.

This guarantees that anything registered against the env — tools, listeners, connections — is
reverted on teardown, preventing memory and service leaks. See
[Reversible Effects](concepts/effects.md) for the pattern.

## Cancellation

Every agent call returns a cancellable handle — `FireHandle` (one-shot `fire`),
`FireStreamHandle`, `SendHandle`, `SendStreamHandle`. `cancel()` — or simply
dropping the handle — cancels the call's `CancellationToken`. The `ReActLoop`
exits cooperatively at its next blocking point (stream consumption, the initial
provider request, in-flight tool execution) and:

- stops receiving output immediately;
- abandons in-flight tool executions (the shared `ToolExecutor` workers survive);
- emits `AgentEvent::Cancelled` through the middleware chain and to event
  subscribers — **never** into session history — so middleware holding external
  services can clean up.

Awaiting a cancelled handle yields `OrchestrateError::Cancelled`; `recv()` on a
streaming handle returns `None` once cancelled.