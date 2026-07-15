//! Demonstrates how to configure **security** for an agent runtime:
//! tool policies, shell restrictions, audit logging, and approval workflows.
//!
//! ```bash
//! cargo run -p funera-orchestrate --example security --features security,builtin-tools
//! ```
//!
//! Requires `OPENAI_API_KEY` (or set via `.api_key()` in code).

use std::time::Duration;

use funera_core::security::audit::AuditEvent;
use funera_core::security::policy::{ShellPolicy, ToolPolicy};
use funera_orchestrate::{Agent, AgentRuntime, DeepSeekProvider};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ── 1. 定义 Shell 策略 ──────────────────────────────────────────
    let shell_policy = ShellPolicy::with_allowed(vec!["git".into(), "cargo".into()]);

    // ── 2. 定义工具策略 ────────────────────────────────────────────
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

    // ── 3. 审批通道：同步回调 → 异步批准 ──────────────────────────
    let (approval_tx, mut approval_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    // ── 4. API key ──────────────────────────────────────────────────
    let api_key = std::env::var("OPENAI_API_KEY")?;

    // ── 5. 构建运行时 ──────────────────────────────────────────────
    let runtime = AgentRuntime::<DeepSeekProvider>::builder()
        .api_key(api_key)
        .model("deepseek-v4-flash")
        .with_builtin_tools()
        .with_tool_policy(tool_policy)
        .with_approval_handler(move |call_id, tool, reason| {
            eprintln!("[approval] \"{tool}\" 需要审批: {reason}");
            eprintln!("[approval] 自动批准");
            let _ = approval_tx.send(call_id.to_string());
        })
        .with_approval_timeout(Duration::from_secs(30))
        .build()?;

    // ── 6. 审批后台 ────────────────────────────────────────────────
    let tool_registry = runtime.tool_registry();
    tokio::spawn(async move {
        while let Some(call_id) = approval_rx.recv().await {
            let reg = tool_registry.read().await;
            if let Err(e) = reg.approve_tool_call(&call_id, true) {
                eprintln!("[approval] 批准失败: {e}");
            }
        }
    });

    // ── 7. 审计订阅 ────────────────────────────────────────────────
    let mut audit_rx = runtime.subscribe_audit();
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
                    eprintln!("[audit] {tool_name} 被拒绝: {reason}");
                }
                other => eprintln!("[audit] {other:?}"),
            }
        }
    });

    // ── 8. Agent ───────────────────────────────────────────────────
    let agent = Agent::builder()
        .system_prompt("你可以使用 read 工具来读取文件、write 来写入文件、edit 来修改文件。")
        .build();

    let resp = agent
        .fire("列出当前目录下的所有 .rs 文件", &runtime)
        .await?;
    println!("Agent: {}", resp.content);

    Ok(())
}
