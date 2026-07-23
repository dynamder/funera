use async_openai::config::OpenAIConfig;
use tokio::sync::{broadcast, mpsc, oneshot};

#[cfg(feature = "tool")]
use crate::event_bus::tool_bus::ToolBus;
#[cfg(feature = "skill")]
use crate::re_act::skills::Skill;
#[cfg(feature = "tool")]
use crate::re_act::tool::Tool;
#[cfg(feature = "tool")]
use crate::re_act::tool_executor::ToolExecutor;
#[cfg(feature = "security")]
use crate::security::audit::{AuditBus, AuditEvent};
#[cfg(feature = "security")]
use crate::security::policy::ToolPolicy;
#[cfg(feature = "sandbox")]
use crate::security::sandbox::SandboxPolicy;

use crate::env::{FuneraEnv, FuneraEnvWatcher};
use crate::event_bus::env_state_bus::EnvStateEvent;

// ═══════════════════════════════════════════════════════════════
// ReActConfig — bundled handles the ReAct loop needs each call
// ═══════════════════════════════════════════════════════════════

pub struct ReActConfig {
    pub env_watcher: FuneraEnvWatcher,
    #[cfg(feature = "tool")]
    pub tool_bus: ToolBus,
    pub max_iterations: usize,
    pub channel_buffer: usize,
}

// ═══════════════════════════════════════════════════════════════
// EnvCmd — commands sent to the EnvActor via mpsc
// ═══════════════════════════════════════════════════════════════

pub enum EnvCmd {
    // ── Mutation (fire-and-forget) ────────────────────────────
    SetModel(String),
    SetClient(async_openai::Client<OpenAIConfig>),
    #[cfg(feature = "tool")]
    AddTool(Box<dyn Tool>),
    #[cfg(feature = "tool")]
    RemoveTool(String),
    #[cfg(feature = "tool")]
    SetToolAvailability {
        name: String,
        available: bool,
    },
    #[cfg(feature = "skill")]
    AddSkill(Skill),
    #[cfg(feature = "skill")]
    RemoveSkill(String),
    #[cfg(feature = "skill")]
    ActivateSkill {
        name: String,
        respond: oneshot::Sender<bool>,
    },
    #[cfg(feature = "skill")]
    DeactivateSkill {
        name: String,
        respond: oneshot::Sender<bool>,
    },
    #[cfg(feature = "skill")]
    SetSkillPrompt(String),

    // ── Query (oneshot response) ──────────────────────────────
    #[cfg(feature = "skill")]
    GetSkillPrompt {
        respond: oneshot::Sender<String>,
    },
    SubscribeEnvState {
        respond: oneshot::Sender<broadcast::Receiver<EnvStateEvent>>,
    },
    GetReActConfig {
        respond: oneshot::Sender<ReActConfig>,
    },
    GetModel {
        respond: oneshot::Sender<String>,
    },
    #[cfg(feature = "tool")]
    GetToolNames {
        respond: oneshot::Sender<Vec<String>>,
    },
    #[cfg(feature = "sandbox")]
    GetSandboxPolicy {
        respond: oneshot::Sender<SandboxPolicy>,
    },
    #[cfg(all(feature = "tool", feature = "security"))]
    ApproveToolCall {
        call_id: String,
        approved: bool,
        respond: oneshot::Sender<Result<(), String>>,
    },
    #[cfg(feature = "security")]
    SubscribeAudit {
        respond: oneshot::Sender<broadcast::Receiver<AuditEvent>>,
    },
}

/// Spawn a long-running EnvActor that owns all environment state.
///
/// The actor is the **single source of truth** for all environment
/// configuration and mutation. It:
/// - Owns [`FuneraEnv`] (model, client, watch senders, registries)
/// - Spawns and manages the [`ToolExecutor`] internally
/// - Atomically broadcasts [`EnvStateEvent`] on every mutation
/// - Provides read-snapshot queries via oneshot channels
///
/// When all [`EnvCmd`] senders are dropped, the actor and its
/// ToolExecutor exit cleanly.
pub fn spawn_env_actor(
    env: FuneraEnv,
    env_watcher: FuneraEnvWatcher,
    max_iterations: usize,
    channel_buffer: usize,
    #[cfg(feature = "tool")] tool_bus: ToolBus,
    #[cfg(feature = "tool")] exec_rx: mpsc::Receiver<crate::event_bus::tool_bus::ToolExecCommand>,
    #[cfg(feature = "sandbox")] sandbox_policy: SandboxPolicy,
    #[cfg(feature = "security")] _tool_policy: ToolPolicy,
    #[cfg(feature = "security")] audit_bus: AuditBus,
) -> mpsc::UnboundedSender<EnvCmd> {
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<EnvCmd>();
    let (state_tx, _) = broadcast::channel::<EnvStateEvent>(32);

    let mut env = env;

    // ── Spawn ToolExecutor internally ─────────────────────────
    #[cfg(feature = "tool")]
    let _executor_handle = {
        let reg = env.tool_registry.clone();
        tokio::spawn(async move {
            ToolExecutor::new(reg, exec_rx).run().await;
        })
    };

    tokio::spawn(async move {
        // ── Broadcast initial state ─────────────────────────────
        #[cfg(feature = "tool")]
        if let Ok(guard) = env.tool_registry.try_read() {
            for name in guard.get_all_tools().keys() {
                let _ = state_tx.send(EnvStateEvent::ToolAdded(name.clone()));
            }
        }
        #[cfg(feature = "skill")]
        if let Ok(guard) = env.skill_registry.try_read() {
            for name in guard.all_skills().keys() {
                let _ = state_tx.send(EnvStateEvent::SkillAdded(name.clone()));
            }
        }

        // ── Command loop ───────────────────────────────────────
        while let Some(cmd) = cmd_rx.recv().await {
            match cmd {
                // ── Mutation ──────────────────────────────────
                EnvCmd::SetModel(model) => {
                    env.set_model(&model);
                    let _ = state_tx.send(EnvStateEvent::LlmChanged(model));
                }
                EnvCmd::SetClient(client) => {
                    env.set_client(client);
                }
                #[cfg(feature = "tool")]
                EnvCmd::AddTool(tool) => {
                    let name = tool.name().to_string();
                    env.add_tool(tool).await;
                    let _ = state_tx.send(EnvStateEvent::ToolAdded(name));
                }
                #[cfg(feature = "tool")]
                EnvCmd::RemoveTool(name) => {
                    env.remove_tool(&name).await;
                    let _ = state_tx.send(EnvStateEvent::ToolRemoved(name));
                }
                #[cfg(feature = "tool")]
                EnvCmd::SetToolAvailability { name, available } => {
                    env.set_tool_availability(&name, available).await;
                    let _ = state_tx.send(EnvStateEvent::ToolAvailability(name, available));
                }
                #[cfg(feature = "skill")]
                EnvCmd::AddSkill(skill) => {
                    let name = skill.name.clone();
                    env.add_skill(skill).await;
                    let _ = state_tx.send(EnvStateEvent::SkillAdded(name));
                }
                #[cfg(feature = "skill")]
                EnvCmd::RemoveSkill(name) => {
                    env.remove_skill(&name).await;
                    let _ = state_tx.send(EnvStateEvent::SkillRemoved(name.clone()));
                }
                #[cfg(feature = "skill")]
                EnvCmd::ActivateSkill { name, respond } => {
                    let ok = env.activate_skill(&name).await;
                    let _ = respond.send(ok);
                    if ok {
                        let _ = state_tx.send(EnvStateEvent::SkillActivated(name));
                    }
                }
                #[cfg(feature = "skill")]
                EnvCmd::DeactivateSkill { name, respond } => {
                    let ok = env.deactivate_skill(&name).await;
                    let _ = respond.send(ok);
                    if ok {
                        let _ = state_tx.send(EnvStateEvent::SkillDeactivated(name));
                    }
                }
                #[cfg(feature = "skill")]
                EnvCmd::SetSkillPrompt(prompt) => {
                    env.set_skill_prompt(prompt);
                }

                // ── Query ─────────────────────────────────────
                #[cfg(feature = "skill")]
                EnvCmd::GetSkillPrompt { respond } => {
                    let _ = respond.send(env.skill_prompt_now());
                }
                EnvCmd::SubscribeEnvState { respond } => {
                    let _ = respond.send(state_tx.subscribe());
                }
                EnvCmd::GetReActConfig { respond } => {
                    let _ = respond.send(ReActConfig {
                        env_watcher: env_watcher.clone(),
                        #[cfg(feature = "tool")]
                        tool_bus: tool_bus.clone(),
                        max_iterations,
                        channel_buffer,
                    });
                }
                EnvCmd::GetModel { respond } => {
                    let _ = respond.send(env.model().to_string());
                }
                #[cfg(feature = "tool")]
                EnvCmd::GetToolNames { respond } => {
                    let names = if let Ok(guard) = env.tool_registry.try_read() {
                        guard.get_all_tools().keys().cloned().collect()
                    } else {
                        Vec::new()
                    };
                    let _ = respond.send(names);
                }
                #[cfg(feature = "sandbox")]
                EnvCmd::GetSandboxPolicy { respond } => {
                    let _ = respond.send(sandbox_policy.clone());
                }
                #[cfg(all(feature = "tool", feature = "security"))]
                EnvCmd::ApproveToolCall {
                    call_id,
                    approved,
                    respond,
                } => {
                    let result = env
                        .tool_registry
                        .blocking_read()
                        .approve_tool_call(&call_id, approved);
                    let _ = respond.send(result);
                }
                #[cfg(feature = "security")]
                EnvCmd::SubscribeAudit { respond } => {
                    let _ = respond.send(audit_bus.subscribe());
                }
            }
        }
    });

    cmd_tx
}
