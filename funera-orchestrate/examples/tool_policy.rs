//! Demonstrates the **Tool Policy** security layer — controlling which tools
//! can run, which shell commands are allowed, and enforcing argument/timeout/workdir
//! limits.
//!
//! ```bash
//! cargo run --example tool_policy --features security,tool
//! ```
//!
//! ## What this example demonstrates
//!
//! 1. **Permissive vs Strict** — `ToolPolicy::permissive()` vs `ToolPolicy::strict()`
//! 2. **Denied tools** — blacklisting specific tool names
//! 3. **Allowed tools** — whitelisting only specific tool names
//! 4. **Shell command whitelisting** — `ShellPolicy::with_allowed()` only permits
//!    commands like `git`/`cargo`
//! 5. **Dangerous pattern blocking** — `ShellPolicy::strict()` auto-blocks
//!    `rm -rf`, `diskpart`, `reg add`, etc.
//! 6. **Argument size limit** — `max_args_size` rejects oversized tool arguments
//! 7. **Timeout bound** — `max_timeout_secs` caps tool execution time
//! 8. **Workdir restriction** — `allowed_workdirs` prevents access outside trusted paths
//! 9. **Audit event subscription** — observes `ToolDenied` events when policy blocks a call
//! 10. **GuardedToolRegistry integration** — wire everything together via `ToolRegistry`
//!
//! No API key required — this example only exercises the security primitives.

use funera_core::security::audit::AuditBus;
use funera_core::security::policy::{ShellPolicy, ToolPolicy};
use funera_core::security::registry::GuardedToolRegistry;
use serde_json::json;

fn main() {
    println!("====================================");
    println!("  Funera Tool Policy — Demo");
    println!("====================================\n");

    demo_permissive();
    demo_strict();
    demo_denied_tools();
    demo_allowed_tools();
    demo_shell_command_whitelist();
    demo_dangerous_patterns();
    demo_args_size_limit();
    demo_timeout_limit();
    demo_workdir_restriction();
    demo_guarded_registry();
    demo_audit_integration();

    println!("\nAll demos completed successfully.");
}

fn println_pass(msg: &str) {
    println!("  [PASS] {msg}");
}

fn println_fail(msg: &str) {
    println!("  [FAIL] {msg}");
}

// ──────────────────────────────────────────────────────────────
// 1. Permissive Policy
// ──────────────────────────────────────────────────────────────

fn demo_permissive() {
    println!("--- 1. Permissive Policy (default) ---");
    let policy = ToolPolicy::permissive();

    // Any tool is allowed
    assert!(policy.check_tool_allowed("any_tool").is_ok());
    assert!(policy.check_tool_allowed("shell").is_ok());
    assert!(policy.check_tool_allowed("random").is_ok());
    println_pass("all tools allowed by default");

    // Default arg size limit is 1 MiB
    assert!(policy.max_args_size == 1024 * 1024);
    println_pass("default max_args_size = 1 MiB");

    // Default timeout is 300s
    assert!(policy.max_timeout_secs == 300.0);
    println_pass("default max_timeout_secs = 300s");

    println!();
}

// ──────────────────────────────────────────────────────────────
// 2. Strict Policy
// ──────────────────────────────────────────────────────────────

fn demo_strict() {
    println!("--- 2. Strict Policy ---");
    let policy = ToolPolicy::strict();

    // Strict policy has an empty allowed_tools set — all tools denied
    let result = policy.check_tool_allowed("shell");
    assert!(result.is_err());
    println_fail(&format!("'shell' blocked by strict policy: {result:?}"));

    let result = policy.check_tool_allowed("read");
    assert!(result.is_err());
    println_fail(&format!("'read' blocked by strict policy: {result:?}"));

    // ShellPolicy inside strict blocks dangerous commands
    let sp = policy.shell_policy.as_ref().unwrap();
    assert!(sp.block_builtin_dangerous);
    println_pass("strict shell_policy has block_builtin_dangerous = true");

    println!();
}

// ──────────────────────────────────────────────────────────────
// 3. Denied Tools (Blacklist)
// ──────────────────────────────────────────────────────────────

fn demo_denied_tools() {
    println!("--- 3. Denied Tools (blacklist) ---");
    let mut policy = ToolPolicy::default();
    policy.denied_tools.insert("danger".into());
    policy.denied_tools.insert("shell".into());

    assert!(policy.check_tool_allowed("danger").is_err());
    assert!(policy.check_tool_allowed("shell").is_err());
    assert!(policy.check_tool_allowed("read").is_ok());

    println_fail("'danger' is denied");
    println_fail("'shell' is denied");
    println_pass("'read' is still allowed");
    println!();
}

// ──────────────────────────────────────────────────────────────
// 4. Allowed Tools (Whitelist)
// ──────────────────────────────────────────────────────────────

fn demo_allowed_tools() {
    println!("--- 4. Allowed Tools (whitelist) ---");
    use std::collections::HashSet;
    let mut allowed = HashSet::new();
    allowed.insert("read".into());
    allowed.insert("write".into());

    let policy = ToolPolicy {
        allowed_tools: Some(allowed),
        ..Default::default()
    };

    assert!(policy.check_tool_allowed("read").is_ok());
    assert!(policy.check_tool_allowed("write").is_ok());
    assert!(policy.check_tool_allowed("shell").is_err());
    assert!(policy.check_tool_allowed("edit").is_err());

    println_pass("'read' and 'write' are allowed");
    println_fail("'shell' is not in the whitelist");
    println_fail("'edit' is not in the whitelist");
    println!();
}

// ──────────────────────────────────────────────────────────────
// 5. Shell Command Whitelisting
// ──────────────────────────────────────────────────────────────

fn demo_shell_command_whitelist() {
    println!("--- 5. Shell Command Whitelisting ---");
    let sp = ShellPolicy::with_allowed(vec!["git".into(), "cargo".into(), "npm".into()]);

    // Allowed commands pass
    assert!(sp.check_command("git status").is_ok());
    assert!(sp.check_command("cargo build").is_ok());
    assert!(sp.check_command("npm install").is_ok());
    println_pass("'git status' allowed");
    println_pass("'cargo build' allowed");
    println_pass("'npm install' allowed");

    // Unknown commands are rejected
    assert!(sp.check_command("ls -la").is_err());
    assert!(sp.check_command("mkdir foo").is_err());
    println_fail("'ls -la' not in allowed list");
    println_fail("'mkdir foo' not in allowed list");

    // Dangerous commands are still blocked (block_builtin_dangerous = true)
    assert!(sp.check_command("rm -rf /").is_err());
    println_fail("'rm -rf /' blocked as dangerous + not in allowed list");

    println!();
}

// ──────────────────────────────────────────────────────────────
// 6. Dangerous Pattern Blocking
// ──────────────────────────────────────────────────────────────

fn demo_dangerous_patterns() {
    println!("--- 6. Dangerous Pattern Blocking ---");
    let sp = ShellPolicy::strict();

    let dangerous_cmds = [
        ("rm -rf /", "recursive force remove"),
        ("sudo rm -rf /tmp", "sudo + dangerous remove"),
        ("diskpart", "disk partition tool"),
        ("format C:", "format drive"),
        ("reg add HKCU\\...", "registry modification"),
        ("shutdown /s", "system shutdown"),
    ];

    for (cmd, desc) in &dangerous_cmds {
        assert!(sp.check_command(cmd).is_err());
        println_fail(&format!("\"{cmd}\" blocked ({desc})"));
    }

    // Safe commands pass
    assert!(sp.check_command("ls -la").is_ok());
    assert!(sp.check_command("echo hello").is_ok());
    assert!(sp.check_command("git status").is_ok());
    println_pass("'ls -la' allowed");
    println_pass("'echo hello' allowed");
    println_pass("'git status' allowed");

    println!();
}

// ──────────────────────────────────────────────────────────────
// 7. Argument Size Limit
// ──────────────────────────────────────────────────────────────

fn demo_args_size_limit() {
    println!("--- 7. Argument Size Limit ---");
    let policy = ToolPolicy {
        max_args_size: 20,
        ..Default::default()
    };

    let small = json!({"a": 1});
    assert!(policy.check_args(&small).is_ok());
    println_pass("small args (few bytes) allowed");

    let large = json!({"data": "this is a large value that exceeds the size limit"});
    assert!(policy.check_args(&large).is_err());
    println_fail("large args rejected (exceeds max_args_size=20)");

    println!();
}

// ──────────────────────────────────────────────────────────────
// 8. Timeout Limit
// ──────────────────────────────────────────────────────────────

fn demo_timeout_limit() {
    println!("--- 8. Timeout Limit ---");
    let policy = ToolPolicy {
        max_timeout_secs: 10.0,
        ..Default::default()
    };

    assert!(policy.check_timeout(5.0).is_ok());
    assert!(policy.check_timeout(10.0).is_ok());
    println_pass("timeout 5s <= 10s max → allowed");
    println_pass("timeout 10s <= 10s max → allowed");

    assert!(policy.check_timeout(30.0).is_err());
    assert!(policy.check_timeout(999.0).is_err());
    println_fail("timeout 30s > 10s max → rejected");
    println_fail("timeout 999s > 10s max → rejected");

    println!();
}

// ──────────────────────────────────────────────────────────────
// 9. Workdir Restriction
// ──────────────────────────────────────────────────────────────

fn demo_workdir_restriction() {
    println!("--- 9. Workdir Restriction ---");
    use std::collections::HashSet;
    let mut dirs = HashSet::new();
    dirs.insert("/workspace".into());
    dirs.insert("/tmp".into());

    let policy = ToolPolicy {
        allowed_workdirs: dirs,
        ..Default::default()
    };

    assert!(policy.check_workdir("/workspace").is_ok());
    assert!(policy.check_workdir("/workspace/project").is_ok());
    assert!(policy.check_workdir("/tmp/build").is_ok());
    println_pass("'/workspace/project' is under allowed dir");

    assert!(policy.check_workdir("/etc").is_err());
    assert!(policy.check_workdir("/home/user").is_err());
    println_fail("'/etc' not in allowed workdirs");
    println_fail("'/home/user' not in allowed workdirs");

    // Empty allowed_workdirs means no restriction
    let open = ToolPolicy::default();
    assert!(open.check_workdir("/anywhere").is_ok());
    println_pass("empty allowed_workdirs → anywhere allowed");

    println!();
}

// ──────────────────────────────────────────────────────────────
// 10. GuardedToolRegistry — Full Policy Enforcement Pipeline
// ──────────────────────────────────────────────────────────────

fn demo_guarded_registry() {
    println!("--- 10. GuardedToolRegistry Integration ---");

    // Build a registry with a policy that:
    //  - allows only "read" and "write"
    //  - denies "shell"
    //  - limits args to 50 bytes
    use std::collections::HashSet;
    let mut allowed = HashSet::new();
    allowed.insert("read".into());
    allowed.insert("write".into());

    let mut denied = HashSet::new();
    denied.insert("shell".into());

    let policy = ToolPolicy {
        allowed_tools: Some(allowed),
        denied_tools: denied,
        max_args_size: 50,
        ..Default::default()
    };

    let registry = GuardedToolRegistry::new_from_policy(policy);
    let stored_policy = registry.policy();

    assert!(stored_policy.max_args_size == 50);
    println_pass("policy flows into registry");

    assert!(stored_policy.check_tool_allowed("read").is_ok());
    assert!(stored_policy.check_tool_allowed("shell").is_err());
    println_pass("tools filtered by policy via registry");

    // Mutable access to policy
    let mut mut_registry = GuardedToolRegistry::new();
    mut_registry.policy_mut().denied_tools.insert("evil".into());
    mut_registry.policy_mut().max_args_size = 100;
    assert!(mut_registry.policy().check_tool_allowed("evil").is_err());
    assert!(mut_registry.policy().max_args_size == 100);
    println_pass("policy can be mutated via policy_mut()");

    // From<ToolPolicy> conversion
    let mut all_allowed = ToolPolicy::default();
    all_allowed.denied_tools.insert("blocked".into());
    let from_registry: GuardedToolRegistry = all_allowed.into();
    assert!(
        from_registry
            .policy()
            .check_tool_allowed("blocked")
            .is_err()
    );
    println_pass("GuardedToolRegistry can be created via From<ToolPolicy>");

    println!();
}

// ──────────────────────────────────────────────────────────────
// 11. Audit Bus — Observing Policy Denials
// ──────────────────────────────────────────────────────────────

fn demo_audit_integration() {
    println!("--- 11. Audit Bus Integration ---");
    use async_trait::async_trait;
    use funera_core::re_act::tool::{Tool, ToolCallError};
    use serde_json::Value as JsonValue;

    struct DummyTool;

    #[async_trait]
    impl Tool for DummyTool {
        fn name(&self) -> &str {
            "blocked_tool"
        }
        fn description(&self) -> &str {
            "a tool that is blocked by policy"
        }
        fn schema(&self) -> JsonValue {
            json!({})
        }
        async fn execute(&self, _args: JsonValue) -> Result<String, ToolCallError> {
            Ok("ok".into())
        }
    }

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        // Policy that denies "blocked_tool"
        let mut policy = ToolPolicy::default();
        policy.denied_tools.insert("blocked_tool".into());

        // Set up audit bus
        let bus = AuditBus::new(16);
        let mut rx = bus.subscribe();

        let mut registry = GuardedToolRegistry::new_from_policy(policy);
        registry.set_audit_bus(bus);
        registry.add_tool(Box::new(DummyTool));

        // Attempt to call the denied tool
        let result = registry.call_tool("blocked_tool", json!({})).await;
        assert!(result.is_err());
        println_fail("'blocked_tool' call rejected by registry");

        // Consume the audit event
        let event = rx.try_recv().expect("expected ToolDenied audit event");
        match event {
            funera_core::security::audit::AuditEvent::ToolDenied {
                ref tool_name,
                ref reason,
                ..
            } => {
                assert_eq!(tool_name, "blocked_tool");
                assert!(reason.contains("denied by policy"));
                println_pass(&format!("audit bus received ToolDenied: \"{reason}\""));
            }
            e => panic!("expected ToolDenied, got {e:?}"),
        }
    });

    println!();
}
