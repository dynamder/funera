# FuneraEnv and Services

`FuneraEnv` is the runtime environment:

- A typed service table keyed by `ServiceKey`
- Per-env reversible effect accumulator (`effect` / `dispose`)
- Hot-reload watch channels for model/client/tools/skills

## Services

```rust,no_run
# use funera_core::env::FuneraEnv;
# use std::sync::Arc;
# async fn example(env: &FuneraEnv) {
#[derive(Clone)]
struct Config { url: String }

env.provide(Config { url: "db://example".into() });
let cfg: Option<Arc<Config>> = env.get::<Config>();
# }
```

Named services allow multiple implementations of the same type to coexist.
