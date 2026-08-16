# Introduction

Funera is a Rust LLM agent framework built around the idea that **everything is a plugin**.

Tools, skills, middleware, providers, callbacks, and even the agent loop itself are `Plugin`
subtraits with one unified lifecycle. The framework provides:

- Reversible effects and typed service tables (`FuneraEnv`)
- Notification-driven plugin registry with typestate lifecycle
- Declarative loader with hot module replacement (`Loader`)
- Runtime plugin reconciliation (`AgentRuntime::reconcile_plugins`)
- Replaceable agent loop (`AgentLoop`)
- Service broker for multi-provider routing

## Design goals

- **Temporal composability**: unload a plugin and revert all of its effects.
- **Spatial composability**: plugins declare dependencies and reactively resolve them.
- **Performance**: plugin machinery lives at lifecycle boundaries, not in hot paths.
