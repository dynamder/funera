use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use serde_json::Value as JsonValue;
use tokio::sync::oneshot;

use crate::re_act::tool::{RawToolRegistry, Tool, ToolCallError, ToolRegistryEntry};
use crate::security::audit::{AuditBus, AuditEvent};
use crate::security::boundary::BoundaryDecision;
use crate::security::path_guard::PathGuard;
use crate::security::policy::ToolPolicy;

/// Callback signature for tool approval requests.
pub type ApprovalCallback = Arc<
    dyn Fn(&str, &str, &str, &[PathBuf]) + Send + Sync,
>;

pub struct GuardedToolRegistry {
    inner: RawToolRegistry,
    policy: ToolPolicy,
    path_guard: Option<PathGuard>,
    audit_bus: Option<AuditBus>,
    #[cfg(feature = "sandbox")]
    sandbox_paths: (Vec<PathBuf>, Vec<PathBuf>),
    pending_approvals:
        std::sync::Arc<std::sync::Mutex<HashMap<String, oneshot::Sender<bool>>>>,
    approval_timeout: Option<std::time::Duration>,
    approval_callback: Option<ApprovalCallback>,
}

impl GuardedToolRegistry {
    pub fn new() -> Self {
        Self {
            inner: RawToolRegistry::new(),
            policy: ToolPolicy::default(),
            path_guard: None,
            audit_bus: None,
            #[cfg(feature = "sandbox")]
            sandbox_paths: (vec![], vec![]),
            pending_approvals: std::sync::Arc::new(std::sync::Mutex::new(HashMap::new())),
            approval_timeout: None,
            approval_callback: None,
        }
    }

    pub fn with_policy(policy: ToolPolicy) -> Self {
        Self {
            inner: RawToolRegistry::new(),
            policy,
            path_guard: None,
            audit_bus: None,
            #[cfg(feature = "sandbox")]
            sandbox_paths: (vec![], vec![]),
            pending_approvals: std::sync::Arc::new(std::sync::Mutex::new(HashMap::new())),
            approval_timeout: None,
            approval_callback: None,
        }
    }

    pub fn with_path_guard(mut self, guard: PathGuard) -> Self {
        self.path_guard = Some(guard);
        self
    }

    pub fn with_audit(mut self, bus: AuditBus) -> Self {
        self.audit_bus = Some(bus);
        self
    }

    pub fn set_audit_bus(&mut self, bus: AuditBus) {
        self.audit_bus = Some(bus);
    }

    pub fn set_path_guard(&mut self, guard: PathGuard) {
        self.path_guard = Some(guard);
    }

    pub fn path_guard(&self) -> Option<&PathGuard> {
        self.path_guard.as_ref()
    }

    pub fn policy(&self) -> &ToolPolicy {
        &self.policy
    }

    pub fn policy_mut(&mut self) -> &mut ToolPolicy {
        &mut self.policy
    }

    /// Set sandbox path boundaries (read_paths, read_write_paths)
    /// used by the boundary check when the sandbox feature is enabled.
    #[cfg(feature = "sandbox")]
    pub fn set_sandbox_paths(&mut self, read_paths: Vec<PathBuf>, read_write_paths: Vec<PathBuf>) {
        self.sandbox_paths = (read_paths, read_write_paths);
    }

    /// Set how long to wait for user approval of a tool call.
    /// `None` means wait indefinitely.
    pub fn set_approval_timeout(&mut self, timeout: Option<std::time::Duration>) {
        self.approval_timeout = timeout;
    }

    /// Set a callback that fires when a tool requires user approval.
    /// The callback receives (call_id, tool_name, reason, paths).
    pub fn set_approval_callback(&mut self, cb: ApprovalCallback) {
        self.approval_callback = Some(cb);
    }

    /// Approve or reject a tool call that is awaiting approval.
    ///
    /// This is the mechanism for external code (callbacks, middleware)
    /// to respond to a [`ToolCallError::ApprovalRequired`] error.
    /// Existing callbacks or middleware handling the `ReactEvent::ToolApprovalRequired`
    /// event can call this method to answer.
    pub fn approve_tool_call(&self, call_id: &str, approved: bool) -> Result<(), String> {
        let mut map = self.pending_approvals.lock().map_err(|e| e.to_string())?;
        if let Some(tx) = map.remove(call_id) {
            let _ = tx.send(approved);
            Ok(())
        } else {
            Err(format!("no pending approval for call_id {call_id}"))
        }
    }

    pub fn add_tool(&mut self, tool: Box<dyn Tool>) {
        self.inner.add_tool(tool);
    }

    pub fn get_tool(&self, name: &str) -> Option<&ToolRegistryEntry> {
        self.inner.get_tool(name)
    }

    pub fn remove_tool(&mut self, name: &str) {
        self.inner.remove_tool(name);
    }

    pub fn tool_exists(&self, name: &str) -> bool {
        self.inner.tool_exists(name)
    }

    pub fn tool_count(&self) -> usize {
        self.inner.tool_count()
    }

    pub fn get_all_tools(&self) -> &HashMap<String, ToolRegistryEntry> {
        self.inner.get_all_tools()
    }

    pub fn available_tools_json(&self) -> JsonValue {
        self.inner.available_tools_json()
    }

    pub async fn call_tool(&self, name: &str, args: JsonValue) -> Result<String, ToolCallError> {
        let start = Instant::now();

        let policy_result = (|| -> Result<(), ToolCallError> {
            self.policy.check_tool_allowed(name)?;
            self.policy.check_args(&args)?;
            if let Some(timeout) = args.get("timeout").and_then(|v| v.as_f64()) {
                self.policy.check_timeout(timeout)?;
            }
            if let Some(workdir) = args.get("workdir").and_then(|v| v.as_str()) {
                self.policy.check_workdir(workdir)?;
            }
            self.policy.check_shell_command(name, &args)?;
            #[cfg(feature = "sandbox")]
            self.audit_sandbox(name);
            Ok(())
        })();

        if let Err(e) = policy_result {
            let _duration = start.elapsed();
            self.audit(AuditEvent::tool_denied(name, e.to_string()));
            return Err(ToolCallError::ToolUnavailable(e.to_string()));
        }

        // ── Sandbox boundary check ──────────────────────────────────
        let paths = extract_paths_from_args(&args);
        #[cfg(feature = "sandbox")]
        let boundary_decision = {
            let (read_paths, read_write_paths) = &self.sandbox_paths;
            let all_paths: Vec<PathBuf> = read_paths
                .iter()
                .chain(read_write_paths.iter())
                .cloned()
                .collect();
            crate::security::boundary::check_boundary(
                name,
                &paths,
                self.path_guard.as_ref(),
                self.policy.sandbox.enabled,
                |p: &PathBuf| {
                    all_paths.iter().any(|root| {
                        let canon = p.canonicalize().unwrap_or_else(|_| p.clone());
                        let root_canon = root.canonicalize().unwrap_or_else(|_| root.clone());
                        canon.starts_with(&root_canon)
                    })
                },
            )
        };
        #[cfg(not(feature = "sandbox"))]
        let boundary_decision = {
            crate::security::boundary::check_boundary(name, &paths, self.path_guard.as_ref())
        };

        match boundary_decision {
            BoundaryDecision::Rejected { reason, .. } => {
                return Err(ToolCallError::Rejected { reason });
            }
            BoundaryDecision::RequiresApproval {
                reason, paths, ..
            } => {
                // If no approval callback is registered, auto-deny
                // to avoid hanging the ReAct loop.
                if self.approval_callback.is_none() {
                    return Err(ToolCallError::Rejected {
                        reason: format!("tool call requires approval but no handler registered: {reason}"),
                    });
                }
                let call_id = uuid::Uuid::new_v4().to_string();
                let (tx, rx) = oneshot::channel();
                {
                    let mut map = self.pending_approvals.lock().unwrap();
                    map.insert(call_id.clone(), tx);
                }
                // Notify the approval callback, if registered.
                if let Some(ref cb) = self.approval_callback {
                    cb(&call_id, name, &reason, &paths);
                }
                // Await the approval response.
                let approved = match self.approval_timeout {
                    Some(timeout) => tokio::time::timeout(timeout, rx)
                        .await
                        .unwrap_or(Ok(false))
                        .unwrap_or(false),
                    None => rx.await.unwrap_or(false),
                };
                if !approved {
                    return Err(ToolCallError::Rejected {
                        reason: "tool call rejected by user".into(),
                    });
                }
                // Approved — fall through to execute
            }
            BoundaryDecision::AutoApproved => {}
        }

        let result = self.inner.call_tool(name, args).await;

        let duration = start.elapsed();
        let duration_ms = duration.as_millis() as u64;

        match &result {
            Ok(_) => {
                self.audit(AuditEvent::tool_executed(name, duration_ms, true, None));
            }
            Err(e) => {
                let error_str = e.to_string();
                self.audit(AuditEvent::tool_executed(name, duration_ms, false, Some(error_str)));
            }
        }

        result
    }

    /// Check whether a tool call with the given call_id is awaiting
    /// user approval. Returns `None` if unknown, or `Some(true/false)`
    /// if the caller has already responded.
    pub fn is_pending_approval(&self, call_id: &str) -> Option<bool> {
        let map = self.pending_approvals.lock().ok()?;
        if map.contains_key(call_id) {
            Some(false) // still pending, not yet answered
        } else {
            None // unknown or resolved
        }
    }

    fn audit(&self, event: AuditEvent) {
        if let Some(ref bus) = self.audit_bus {
            bus.report(event);
        }
    }

    /// Record a sandbox audit event for the given tool.
    #[cfg(feature = "sandbox")]
    fn audit_sandbox(&self, tool_name: &str) {
        let sandbox = &self.policy.sandbox;
        let summary = sandbox.summary();
        if sandbox.enabled {
            self.audit(AuditEvent::sandbox_applied(tool_name, &summary));
        } else {
            self.audit(AuditEvent::sandbox_skipped(tool_name));
        }
    }
}

impl Default for GuardedToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for GuardedToolRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GuardedToolRegistry")
            .field("tool_count", &self.inner.tool_count())
            .field("policy", &self.policy)
            .finish()
    }
}

impl From<ToolPolicy> for GuardedToolRegistry {
    fn from(policy: ToolPolicy) -> Self {
        Self::with_policy(policy)
    }
}

/// Extract file paths from tool arguments.
///
/// Many tools accept a `filePath` parameter; this helper collects them
/// for the sandbox boundary check.
fn extract_paths_from_args(args: &JsonValue) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(fp) = args.get("filePath").and_then(|v| v.as_str()) {
        paths.push(std::path::PathBuf::from(fp));
    }
    if let Some(_command) = args.get("command").and_then(|v| v.as_str()) {
        // For shell commands, the sandbox operates on the process level
        // via pre_exec / CreateProcessAsUserW.  The boundary check is
        // advisory — we extract the command string for reference.
        paths.push(std::path::PathBuf::from("shell-command"));
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::json;

    struct OkTool;

    #[async_trait]
    impl Tool for OkTool {
        fn name(&self) -> &str { "ok_tool" }
        fn description(&self) -> &str { "always succeeds" }
        fn schema(&self) -> JsonValue {
            json!({"type": "function", "function": {"name": "ok_tool", "parameters": {"type": "object", "properties": {}}}})
        }
        async fn execute(&self, _args: JsonValue) -> Result<String, ToolCallError> {
            Ok("done".into())
        }
    }

    #[tokio::test]
    async fn allowed_tool_works() {
        let mut registry = GuardedToolRegistry::new();
        registry.add_tool(Box::new(OkTool));
        let result = registry.call_tool("ok_tool", json!({})).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "done");
    }

    #[tokio::test]
    async fn denied_tool_blocked() {
        let mut policy = ToolPolicy::default();
        policy.denied_tools.insert("danger".into());
        let mut registry = GuardedToolRegistry::with_policy(policy);
        registry.add_tool(Box::new(OkTool));
        let result = registry.call_tool("danger", json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn audit_events_fired() {
        let bus = AuditBus::new(16);
        let mut rx = bus.subscribe();
        let mut registry = GuardedToolRegistry::new();
        registry.set_audit_bus(bus);
        registry.add_tool(Box::new(OkTool));

        registry.call_tool("ok_tool", json!({})).await.unwrap();

        // Consume the SandboxApplied event (if sandbox feature is on)
        #[cfg(feature = "sandbox")]
        {
            let event = rx.try_recv().expect("expected SandboxApplied");
            match event {
                AuditEvent::SandboxApplied { ref tool_name, .. } => {
                    assert_eq!(tool_name, "ok_tool");
                }
                e => panic!("expected SandboxApplied, got {e:?}"),
            }
        }

        // The actual ToolExecuted event
        let event = rx.try_recv().expect("expected ToolExecuted");
        match event {
            AuditEvent::ToolExecuted { ref tool_name, success, .. } => {
                assert_eq!(tool_name, "ok_tool");
                assert!(success);
            }
            e => panic!("expected ToolExecuted, got {e:?}"),
        }
    }

    #[tokio::test]
    async fn policy_violation_triggers_denied_audit() {
        let mut policy = ToolPolicy::default();
        policy.denied_tools.insert("evil".into());
        let bus = AuditBus::new(16);
        let mut rx = bus.subscribe();
        let mut registry = GuardedToolRegistry::with_policy(policy);
        registry.set_audit_bus(bus);
        registry.add_tool(Box::new(OkTool));

        registry.call_tool("evil", json!({})).await.err();
        let event = rx.try_recv();
        assert!(event.is_ok());
        match event.unwrap() {
            AuditEvent::ToolDenied { ref tool_name, .. } => {
                assert_eq!(tool_name, "evil");
            }
            e => panic!("expected ToolDenied, got {e:?}"),
        }
    }

    // ── sandbox audit tests (sandbox feature only) ─────────────────

    #[cfg(feature = "sandbox")]
    #[tokio::test]
    async fn sandbox_audit_event_contains_policy_summary() {
        use crate::security::sandbox::SandboxPolicy;

        let policy = ToolPolicy {
            sandbox: SandboxPolicy {
                read_write_paths: vec!["/workspace".into()],
                block_network: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let bus = AuditBus::new(16);
        let mut rx = bus.subscribe();
        let mut registry = GuardedToolRegistry::with_policy(policy);
        registry.set_audit_bus(bus);
        registry.add_tool(Box::new(OkTool));

        registry.call_tool("ok_tool", json!({})).await.unwrap();

        let event = rx.try_recv().expect("expected SandboxApplied");
        match event {
            AuditEvent::SandboxApplied {
                ref tool_name,
                ref policy_summary,
                ..
            } => {
                assert_eq!(tool_name, "ok_tool");
                assert!(
                    policy_summary.contains("rw:/workspace"),
                    "summary should describe policy: {policy_summary}"
                );
                assert!(
                    policy_summary.contains("no-net"),
                    "summary should mention blocked network: {policy_summary}"
                );
            }
            e => panic!("expected SandboxApplied, got {e:?}"),
        }
    }

    #[cfg(feature = "sandbox")]
    #[tokio::test]
    async fn sandbox_skipped_when_disabled() {
        use crate::security::sandbox::SandboxPolicy;

        let policy = ToolPolicy {
            sandbox: SandboxPolicy::disabled(),
            ..Default::default()
        };
        let bus = AuditBus::new(16);
        let mut rx = bus.subscribe();
        let mut registry = GuardedToolRegistry::with_policy(policy);
        registry.set_audit_bus(bus);
        registry.add_tool(Box::new(OkTool));

        registry.call_tool("ok_tool", json!({})).await.unwrap();

        let event = rx.try_recv().expect("expected SandboxSkipped");
        match event {
            AuditEvent::SandboxSkipped { ref tool_name, .. } => {
                assert_eq!(tool_name, "ok_tool");
            }
            e => panic!("expected SandboxSkipped, got {e:?}"),
        }
    }
}
