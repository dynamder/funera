use std::collections::HashMap;
use std::time::Instant;

use serde_json::Value as JsonValue;

use crate::re_act::tool::{RawToolRegistry, Tool, ToolCallError, ToolRegistryEntry};
use crate::security::audit::{AuditBus, AuditEvent};
use crate::security::path_guard::PathGuard;
use crate::security::policy::ToolPolicy;

pub struct GuardedToolRegistry {
    inner: RawToolRegistry,
    policy: ToolPolicy,
    path_guard: Option<PathGuard>,
    audit_bus: Option<AuditBus>,
}

impl GuardedToolRegistry {
    pub fn new() -> Self {
        Self {
            inner: RawToolRegistry::new(),
            policy: ToolPolicy::default(),
            path_guard: None,
            audit_bus: None,
        }
    }

    pub fn with_policy(policy: ToolPolicy) -> Self {
        Self {
            inner: RawToolRegistry::new(),
            policy,
            path_guard: None,
            audit_bus: None,
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
            Ok(())
        })();

        if let Err(e) = policy_result {
            let _duration = start.elapsed();
            self.audit(AuditEvent::tool_denied(name, e.to_string()));
            return Err(ToolCallError::ToolUnavailable(e.to_string()));
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

    fn audit(&self, event: AuditEvent) {
        if let Some(ref bus) = self.audit_bus {
            bus.report(event);
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
        let event = rx.try_recv();
        assert!(event.is_ok());
        match event.unwrap() {
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
}
