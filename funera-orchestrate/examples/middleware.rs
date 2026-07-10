//! 演示 middleware 管道：Inspector（异步日志） + Mutator（敏感词过滤 + 工具拦截）。
//!
//! 本示例展示了完整的 middleware 使用流程：
//! 1. 定义自定义[`InspectorMiddleware`]和[`MutatorMiddleware`]
//! 2. 使用 [`MiddlewareChain`] 构建多层管道
//! 3. 通过 [`MiddlewareBundle`] 注入到 [`AgentRuntime`]
//!
//! ## 运行
//!
//! ```bash
//! cargo run --example middleware --features funera-orchestrate/middleware
//! ```
//!
//! ## 示例中的 middleware
//!
//! | Middleware | 类型 | 作用 |
//! |------------|------|------|
//! | `EventLogger` | Inspector | 异步打印所有事件到 stderr |
//! | `Censor` | Mutator | 替换 token 中的敏感词为 `***` |
//! | `BlockTool` | Mutator | 阻止指定名称的工具调用 |
//! | `TurnCounter` | Inspector | 统计并打印 turn 次数（演示带状态的 inspector） |

use std::sync::atomic::{AtomicUsize, Ordering};

use funera_orchestrate::middleware::*;
use funera_orchestrate::middleware_bundle::MiddlewareBundle;
use funera_orchestrate::{Agent, AgentEvent, AgentRuntime, DeepSeekProvider};

// ═══════════════════════════════════════════════════════════════
// Inspector: 事件日志
// ═══════════════════════════════════════════════════════════════

/// 异步打印所有事件的 inspector。
///
/// 由于 inspector 在后台并行执行，此日志不会阻塞正常的事件流。
struct EventLogger;

impl InspectorMiddleware<AgentEvent> for EventLogger {
    fn name(&self) -> &str {
        "event_logger"
    }

    fn inspect(&self, event: &AgentEvent) -> Result<(), InspectorError> {
        match event {
            AgentEvent::Text(t) => eprintln!("[log] token: {t}"),
            AgentEvent::Reasoning(r) => eprintln!("[log] reasoning: {r}"),
            AgentEvent::ToolCallRequest { name, args, .. } => {
                eprintln!("[log] tool_call: {name}({args})")
            }
            AgentEvent::ToolCallResult { name, result, .. } => {
                let status = match result {
                    Ok(_) => "ok",
                    Err(_) => "err",
                };
                eprintln!("[log] tool_result: {name} ({status})");
            }
            AgentEvent::TurnStart => eprintln!("[log] --- turn start ---"),
            AgentEvent::TurnEnd { .. } => eprintln!("[log] --- turn end ---"),
            AgentEvent::Done => eprintln!("[log] done"),
            AgentEvent::Error(e) => eprintln!("[log] error: {e}"),
        }
        Ok(())
    }
}

// ═══════════════════════════════════════════════════════════════
// Inspector: Turn 计数器（演示带状态的 inspector）
// ═══════════════════════════════════════════════════════════════

/// 统计并打印 ReAct turn 次数。
struct TurnCounter {
    count: AtomicUsize,
}

impl InspectorMiddleware<AgentEvent> for TurnCounter {
    fn name(&self) -> &str {
        "turn_counter"
    }

    fn inspect(&self, event: &AgentEvent) -> Result<(), InspectorError> {
        if matches!(event, AgentEvent::TurnStart) {
            let n = self.count.fetch_add(1, Ordering::Relaxed) + 1;
            eprintln!("[counter] turn #{n}");
        }
        Ok(())
    }
}

// ═══════════════════════════════════════════════════════════════
// Mutator: 敏感词过滤
// ═══════════════════════════════════════════════════════════════

/// 将 LLM 输出中的敏感词替换为 `***`。
struct Censor {
    words: Vec<&'static str>,
}

impl MutatorMiddleware<AgentEvent> for Censor {
    fn name(&self) -> &str {
        "censor"
    }

    fn process(&self, event: AgentEvent) -> MutatorAction<AgentEvent> {
        match event {
            AgentEvent::Text(t) => {
                let orig = t.clone();
                let mut result = t;
                for w in &self.words {
                    result = result.replace(w, "***");
                }
                if result != orig {
                    eprintln!("[censor] modified token (sensitive word detected)");
                    MutatorAction::Modify(AgentEvent::Text(result))
                } else {
                    MutatorAction::Pass
                }
            }
            _e => MutatorAction::Pass,
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// Mutator: 工具调用拦截
// ═══════════════════════════════════════════════════════════════

/// 阻止指定名称的工具被调用。
struct BlockTool {
    tool_name: String,
}

impl MutatorMiddleware<AgentEvent> for BlockTool {
    fn name(&self) -> &str {
        "block_tool"
    }

    fn process(&self, event: AgentEvent) -> MutatorAction<AgentEvent> {
        if let AgentEvent::ToolCallRequest { name, .. } = &event {
            if self.tool_name.eq_ignore_ascii_case(name) {
                eprintln!("[block_tool] blocked tool call: {name}");
                return MutatorAction::Block {
                    reason: format!("tool '{}' is blocked", name),
                };
            }
        }
        MutatorAction::Pass
    }
}

// ═══════════════════════════════════════════════════════════════
// main
// ═══════════════════════════════════════════════════════════════

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 构建 middleware 链：
    //
    //   Layer 0 (Inspector)   : EventLogger + TurnCounter  ← 并行
    //   Layer 1 (Mutator)     : Censor                     ← 敏感词替换
    //   Layer 2 (Inspector)   : (仅用于演示分层)            ← 新的并行层
    //   Layer 3 (Mutator)     : BlockTool                  ← 工具拦截
    let bundle = MiddlewareBundle::from_chain(
        MiddlewareChain::<AgentEvent>::new()
            // --- Layer 0: Inspector (并行) ---
            .with_inspectors((
                EventLogger,
                TurnCounter {
                    count: AtomicUsize::new(0),
                },
            ))
            // --- Layer 1: Mutator (顺序) ---
            .with_mutator(Censor {
                words: vec!["secret", "password"],
            })
            // --- Layer 3: Mutator (顺序) ---
            .with_mutator(BlockTool {
                tool_name: "shell".into(),
            }),
    );

    let mut runtime = AgentRuntime::<DeepSeekProvider>::builder()
        .api_key(std::env::var("OPENAI_API_KEY")?)
        .with_middleware_bundle(bundle)
        .build()?;

    let agent = Agent::builder()
        .system_prompt("You are a helpful assistant. You have access to shell and other tools.")
        .build();

    // 测试发送
    let response = agent
        .send(
            "Say hello! Also, my password is 'my_secret_123'.",
            &mut runtime,
        )
        .await?;
    println!("Response: {}", response.content);

    Ok(())
}
