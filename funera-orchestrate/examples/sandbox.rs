//! Demo kernel sandboxing (Landlock on Linux, Seatbelt on macOS,
//! Write-Restricted Token on Windows).
//!
//! ```bash
//! cargo run --example sandbox --features sandbox,builtin-tools
//! ```
//!
//! The sandbox restricts the Shell tool subprocess to only the
//! paths explicitly allowed by the policy. All network access is
//! blocked by default.
//!
//! ## What this example demonstrates
//!
//! 1. Constructing a [`SandboxPolicy`] that limits the shell to
//!    reading/writing files only inside the current directory.
//! 2. Passing the policy to the runtime via
//!    [`AgentRuntimeBuilder::with_sandbox_policy`].
//! 3. The agent uses the shell tool to list files — the subprocess
//!    runs under Landlock (Linux 5.13+), Seatbelt (macOS), or
//!    Write-Restricted Token (Windows 8+) and cannot access paths
//!    outside the allowed set.
//! 4. The agent then ATTEMPTS operations that deliberately violate
//!    the sandbox policy:
//!    - Write a file outside the allowed directory
//!    - Access a network resource (`curl` / `ping`)
//!    - Read a file outside the allowed directory
//! 5. Each violation attempt is blocked by the sandbox, proving
//!    that the policy is enforced at the OS level (or, on platforms
//!    where the sandbox degrades gracefully, the user can observe
//!    the fallback behaviour).
//! 6. Audit events (`SandboxApplied`) are emitted for every tool
//!    call, visible when subscribing to the runtime audit bus.
//!
//! ## Platform notes
//!
//! | Platform | Mechanism |
//! |----------|-----------|
//! | Linux 5.13+ | Landlock via `nono` |
//! | macOS 10.5+ | Seatbelt via `nono` |
//! | Windows 8+ | Write-Restricted Token + synthetic SID + ACLs |
//!
//! ## Prerequisites
//!
//! - `OPENAI_API_KEY` environment variable

use funera_core::security::sandbox::SandboxPolicy;
use funera_orchestrate::{Agent, AgentRuntime, DeepSeekProvider};

#[tokio::main]
async fn main() {
    let api_key = match std::env::var("OPENAI_API_KEY") {
        Ok(k) => k,
        Err(_) => {
            eprintln!("error: OPENAI_API_KEY environment variable is required");
            eprintln!("usage: OPENAI_API_KEY=sk-... cargo run --example sandbox -p funera-orchestrate --features sandbox,builtin-tools");
            std::process::exit(1);
        }
    };

    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: cannot determine current directory: {e}");
            std::process::exit(1);
        }
    };

    // Build a sandbox policy that only allows access to the
    // current working directory and blocks all network traffic.
    let sandbox = SandboxPolicy {
        read_write_paths: vec![cwd],
        block_network: true,
        ..Default::default()
    };

    println!("Using sandbox policy: {}", sandbox.summary());

    let runtime = match AgentRuntime::<DeepSeekProvider>::builder()
        .api_key(&api_key)
        .base_url(std::env::var("OPENAI_BASE_URL").ok())
        .model(std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "deepseek-v4-flash".into()))
        .with_sandbox_policy(sandbox)
        .with_builtin_tools()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: failed to build runtime: {e}");
            std::process::exit(1);
        }
    };

    let agent = Agent::builder()
        .system_prompt(
            "You are a helpful assistant with shell access sandboxed to \
             the current directory. Network access is blocked and file \
             writes are restricted to the current directory only.\n\n\
             First, list the files using `ls -la` or `dir`.\n\
             Then, attempt THREE operations that SHOULD be blocked \
             by the sandbox:\n\n\
             1. Write a file OUTSIDE the allowed directory\n\
             2. Access a network resource\n\
             3. Read a file OUTSIDE the allowed directory\n\n\
             For each attempt, observe whether it succeeds or is \
             blocked, and explain what happened.",
        )
        .on_tool_call(|name, args| eprintln!("[tool] {name} {args}"))
        .build();

    match agent
        .fire(
            "Explore the current directory, then attempt operations \
             that violate the sandbox restrictions to verify the \
             sandbox is working.",
            &runtime,
        )
        .await
    {
        Ok(resp) => {
            println!("=== Agent Response ===\n{}", resp.content);
            println!("=== Completed in {} iterations ===", resp.iterations);
        }
        Err(e) => {
            eprintln!("error: agent request failed: {e}");
            std::process::exit(1);
        }
    }
}
