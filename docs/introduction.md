# Introduction

Funera is a security-oriented LLM agent framework for Rust. It composes a ReAct execution
loop, pluggable LLM providers, tools, skills, and a middleware pipeline — all behind a small,
explicit core.

The framework provides:

- A **ReAct loop** with hot-reload: model/client/tools/skills can change mid-flight via watch
  channels, and the loop picks the changes up on the next iteration.
- **Pluggable traits** — `ChatProvider`, `Tool`, `Skill`, `InspectorMiddleware` /
  `MutatorMiddleware` — extended the way you would expect from plain Rust traits.
- **Reversible effects** (`FuneraEnv::effect` / `dispose`): every registration can be paired
  with its inverse, and a single teardown call runs all inverses in reverse (LIFO) order, so
  memory and services never leak.
- **Multi-layered security** — tool policies, path allowlisting, approval callbacks, audit
  logging, and a kernel-backed sandbox on supported platforms.

## Design goals

- **Simple core, broad extensibility**: the runtime is a thin channel wrapper around an
  `EnvActor` that owns all state; extension happens through traits, not through a meta-layer.
- **Leak-safety by construction**: effects are reversible by design; tearing down the runtime
  tears down everything it registered.
- **Security as a first-class concern**: policies are enforced at the registry boundary and
  every tool call is auditable.
