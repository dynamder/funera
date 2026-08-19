# FuneraEnv

`FuneraEnv` is the shared runtime environment owned by the `EnvActor`:

- **Hot-reload watch channels** for the LLM client, model, tools, and skills — the `ReActLoop`
  picks changes up on the next iteration via `FuneraEnvWatcher`.
- **Tool / skill registries** with availability flags, mutated through `EnvCmd` commands.
- **A reversible-effects accumulator** (`effect` / `dispose`) — see
  [Reversible Effects](effects.md).

```rust,no_run
# use funera_core::env::FuneraEnv;
# use funera_core::env::Disposer;
# fn example() {
let (env, mut watcher) = FuneraEnv::new(async_openai::Client::new(), "gpt-4o");

// Register a reversible effect: its inverse runs on dispose (LIFO).
env.effect(|| {
    println!("acquiring resource");
    let resource = String::from("leased");
    let undo: Disposer = Box::new(move || println!("releasing {resource}"));
    undo
});

let model = watcher.watch_model(); // "gpt-4o"
# let _ = model;
# }
```

`FuneraEnv` derives `Clone`; a clone shares the registries and watch channels, so child envs
can be handed to tasks while the owner keeps the watcher.
