# Reversible Effects (LIFO Teardown)

Every registration that outlives its owner is a potential leak: a tool that is never removed,
a listener that is never unsubscribed, a connection that is never closed. Funera's answer is a
small, generic primitive on `FuneraEnv`:

```rust,ignore
env.effect(|| {
    let resource = acquire();              // setup: the effect
    Box::new(move || release(resource))    // teardown: its inverse
});
```

## Semantics

- `effect(body)` runs `body` immediately and pushes the returned
  [`Disposer`](https://docs.rs/funera-core/latest/funera_core/env/type.Disposer.html) onto the
  env's accumulator.
- `dispose()` runs every disposer in **reverse registration order** (LIFO), so later effects —
  which may depend on earlier ones — are undone first.
- Disposal is **idempotent**: the accumulator is drained, a second `dispose()` is a no-op.
- Disposal is **panic-isolated**: a panicking disposer is caught and logged while the remaining
  disposers still run.

## When does disposal run?

`EnvActor` owns the live `FuneraEnv` and calls `dispose()` automatically once every `EnvCmd`
sender is gone — i.e. when the runtime is dropped. You can also call `env.dispose()` manually
to tear down an env you created directly.

## Leak-safe tool registration

The safe inverse of `add_tool` is `remove_tool_if_same`: it removes the tool only if the
registered entry is the *same* `Arc` value, so a stale teardown can never delete a replacement
tool that reuses the same name:

```rust,ignore
let tool: Arc<dyn Tool> = Arc::new(MyTool);
env.add_tool(Arc::clone(&tool)).await;

let registry = env.tool_registry.clone();
env.effect(move || {
    let registry = Arc::clone(&registry);
    let tool = Arc::clone(&tool);
    Box::new(move || {
        if let Some(mut guard) = registry.try_write() {
            guard.remove_tool_if_same(tool.name(), &tool);
        }
    })
});
```

When the env is disposed, exactly this tool is removed — never a later replacement.

## Run the example

```bash
cargo run -p funera-orchestrate --example reversible_effects
```
