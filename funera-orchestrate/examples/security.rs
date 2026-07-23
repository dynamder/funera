//! Demonstrates how to configure **security** for an agent runtime:
//! tool policies, shell restrictions, audit logging, and approval workflows.
//!
//! Uses [`ApprovalHandle`] — a lightweight cloneable handle obtained via
//! [`AgentRuntime::approval_handle`] — to approve tool calls from a
//! spawned background task.  This works with both `fire()` (one-shot)
//! and `send()` / `send_stream()` (multi-turn with persistent session).
//!
//! ```bash
//! cargo run -p funera-orchestrate --example security --features security,funera-builtin-tools
//! ```
//!
//! Requires `OPENAI_API_KEY` (or set via `.api_key()` in code).

use std::time::Duration;

use funera_core::security::audit::AuditEvent;
use funera_core::security::policy::{ShellPolicy, ToolPolicy};
use funera_orchestrate::{Agent, AgentRuntime, ApprovalHandle, DeepSeekProvider};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ── 1. Shell policy ─────────────────────────────────────────────
    let shell_policy = ShellPolicy::with_allowed(vec!["git".into(), "cargo".into()]);

    // ── 2. Tool policy ─────────────────────────────────────────────
    let tool_policy = ToolPolicy {
        allowed_tools: Some(
            ["read", "write", "edit"]
                .into_iter()
                .map(String::from)
                .collect(),
        ),
        denied_tools: ["shell".into()].into_iter().collect(),
        shell_policy: Some(shell_policy),
        max_args_size: 1024 * 1024,
        max_timeout_secs: 60.0,
        ..Default::default()
    };

    // ── 3. Approval channel: callback notifies background task ─────
    let (approval_tx, mut approval_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    // ── 4. API key ──────────────────────────────────────────────────
    let api_key = std::env::var("OPENAI_API_KEY")?;

    // ── 5. Build runtime ───────────────────────────────────────────
    let runtime = AgentRuntime::<DeepSeekProvider>::builder()
        .api_key(api_key)
        .model("deepseek-v4-flash")
        .with_builtin_tools()
        .with_tool_policy(tool_policy)
        .on_approval_required(move |call_id, tool, reason| {
            eprintln!("[approval] \"{tool}\" needs approval: {reason}");
            eprintln!("[approval] auto-approving");
            let _ = approval_tx.send(call_id.to_string());
        })
        .with_approval_timeout(Duration::from_secs(30))
        .build()?;

    // ── 6. ApprovalHandle – cloneable, move into spawned task ──────
    let approver: ApprovalHandle = runtime.approval_handle();
    tokio::spawn(async move {
        while let Some(call_id) = approval_rx.recv().await {
            if let Err(e) = approver.approve_tool_call(&call_id, true).await {
                eprintln!("[approval] failed: {e}");
            }
        }
    });

    // ── 7. Audit subscription ──────────────────────────────────────
    let mut audit_rx = runtime.subscribe_audit().await;
    tokio::spawn(async move {
        while let Ok(event) = audit_rx.recv().await {
            match event {
                AuditEvent::ToolExecuted {
                    tool_name,
                    duration_ms,
                    success,
                    ..
                } => {
                    let status = if success { "OK" } else { "ERR" };
                    eprintln!("[audit] {tool_name} [{status}] {duration_ms}ms");
                }
                AuditEvent::ToolDenied {
                    tool_name, reason, ..
                } => {
                    eprintln!("[audit] {tool_name} denied: {reason}");
                }
                other => eprintln!("[audit] {other:?}"),
            }
        }
    });

    // ── 8. Agent ───────────────────────────────────────────────────
    let agent = Agent::builder()
        .system_prompt("You can use read/write/edit tools.")
        .build();

    // `fire()` borrows the runtime — one-shot, stateless.  Works with
    // `approve_tool_call` because both go through the shared EnvActor.
    let resp = agent
        .fire("List all .rs files in the current directory", &runtime)
        .await?;
    println!("Agent: {}", resp.content);

    // For multi-turn conversations, clone the ApprovalHandle *before*
    // `send()` consumes the runtime:
    //   let approver = runtime.approval_handle();
    //   let (runtime, resp) = agent.send("hello", runtime).await?.await?;

    Ok(())
}
