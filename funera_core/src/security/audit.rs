use chrono::Utc;
use serde::Serialize;
use tokio::sync::broadcast;

#[derive(Debug, Clone, Serialize)]
pub enum AuditEvent {
    ToolExecuted {
        tool_name: String,
        duration_ms: u64,
        success: bool,
        error: Option<String>,
        timestamp: i64,
    },
    ToolDenied {
        tool_name: String,
        reason: String,
        timestamp: i64,
    },
    PolicyViolated {
        detail: String,
        timestamp: i64,
    },
}

impl AuditEvent {
    pub fn tool_executed(
        tool_name: &str,
        duration_ms: u64,
        success: bool,
        error: Option<String>,
    ) -> Self {
        Self::ToolExecuted {
            tool_name: tool_name.to_string(),
            duration_ms,
            success,
            error,
            timestamp: Utc::now().timestamp(),
        }
    }

    pub fn tool_denied(tool_name: &str, reason: String) -> Self {
        Self::ToolDenied {
            tool_name: tool_name.to_string(),
            reason,
            timestamp: Utc::now().timestamp(),
        }
    }

    pub fn policy_violated(detail: String) -> Self {
        Self::PolicyViolated {
            detail,
            timestamp: Utc::now().timestamp(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AuditBus {
    tx: broadcast::Sender<AuditEvent>,
}

impl AuditBus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    pub fn report(&self, event: AuditEvent) {
        let _ = self.tx.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<AuditEvent> {
        self.tx.subscribe()
    }

    pub fn sender(&self) -> broadcast::Sender<AuditEvent> {
        self.tx.clone()
    }
}

impl Default for AuditBus {
    fn default() -> Self {
        Self::new(1024)
    }
}
