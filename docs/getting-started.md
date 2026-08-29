# Getting Started

Add the crates you need:

```toml
[dependencies]
funera = "0.2"
```

## Minimal agent

```rust,no_run
use funera::{Agent, AgentRuntime, DeepSeekProvider};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = AgentRuntime::<DeepSeekProvider>::builder()
        .api_key(std::env::var("DEEPSEEK_API_KEY")?)
        .model("deepseek-v4-flash")
        .build()?;

    let agent = Agent::builder()
        .system_prompt("You are a helpful assistant.")
        .build();

    let resp = agent.fire("Hello!", &runtime).await?.await?.await?;
    println!("{}", resp.content);
    Ok(())
}
```

## Run an example

```bash
cargo run -p funera-orchestrate --example minimal
```

Examples that do not require an LLM API key:

```bash
cargo run -p funera-orchestrate --example reversible_effects
cargo run -p funera-orchestrate --example tool_policy --features security
```
