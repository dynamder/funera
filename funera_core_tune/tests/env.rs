use funera_core::env::FuneraEnv;
use funera_core::re_act::tool::ToolRegistry;
use funera_core_tune::utils::env_config::default_model;
use funera_core_tune::utils::fixtures::default_schema;
use funera_core_tune::utils::mock_tool::MockTool;

fn create_env() -> (FuneraEnv, funera_core::env::FuneraEnvWatcher) {
    let client = async_openai::Client::new();
    let registry = ToolRegistry::new();
    FuneraEnv::new(registry, client, default_model())
}

#[test]
fn env_new_creates_watcher() {
    let (_env, watcher) = create_env();
    assert!(!watcher.has_tool_changed());
    assert!(!watcher.has_client_changed());
    assert!(!watcher.has_model_changed());
}

#[tokio::test]
async fn env_add_tool_updates_watcher() {
    let (mut env, watcher) = create_env();
    assert!(!watcher.has_tool_changed());

    let tool = MockTool::new("ping", default_schema("ping"));
    env.add_tool(Box::new(tool)).await;
    assert!(watcher.has_tool_changed());
}

#[tokio::test]
async fn env_remove_tool_updates_watcher() {
    let (mut env, watcher) = create_env();
    let tool = MockTool::new("temp", default_schema("temp"));
    env.add_tool(Box::new(tool)).await;
    assert!(watcher.has_tool_changed());

    env.remove_tool("temp").await;
    assert!(watcher.has_tool_changed());
}

#[test]
fn watcher_watch_tool_returns_json() {
    let (mut _env, mut watcher) = create_env();
    let tools = watcher.watch_tool();
    assert!(tools.is_array());
}

#[test]
fn env_set_model() {
    let (mut env, mut watcher) = create_env();
    assert_eq!(watcher.watch_model(), default_model());
    env.set_model("gpt-4");
    assert_eq!(watcher.watch_model(), "gpt-4");
}

#[test]
fn env_set_client() {
    let (mut env, mut watcher) = create_env();
    assert!(!watcher.has_client_changed(), "initially unchanged");
    let new_client = async_openai::Client::new();
    env.set_client(new_client);
    assert!(watcher.has_client_changed(), "watcher should detect client change");
    let _ = watcher.watch_client();
}

#[test]
fn env_watcher_has_tool_changed_false_initially() {
    let (_env, watcher) = create_env();
    assert!(!watcher.has_tool_changed());
    assert!(!watcher.has_client_changed());
    assert!(!watcher.has_model_changed());
}

#[tokio::test]
async fn watcher_async_changed_awaits_update() {
    let (mut env, watcher) = create_env();
    let mut tool_watcher = watcher.clone();
    let handle = tokio::spawn(async move {
        tokio::time::timeout(std::time::Duration::from_millis(500), tool_watcher.tool_changed())
            .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let tool = MockTool::new("late", default_schema("late"));
    env.add_tool(Box::new(tool)).await;
    let result = handle.await.unwrap();
    assert!(result.is_ok());
}

#[test]
fn env_watcher_clone_is_independent() {
    let (_env, watcher) = create_env();
    let watcher2 = watcher.clone();
    assert!(!watcher2.has_tool_changed());
}

#[tokio::test]
async fn env_multiple_tools() {
    let (mut env, mut watcher) = create_env();
    for i in 0..5 {
        let tool = MockTool::new(format!("tool_{}", i), default_schema(&format!("tool_{}", i)));
        env.add_tool(Box::new(tool)).await;
    }
    assert!(watcher.has_tool_changed());
    let tools = watcher.watch_tool();
    assert_eq!(tools.as_array().unwrap().len(), 5);
}

// ── Skill management tests ──────────────────────────────────────

#[tokio::test]
async fn env_add_skill_updates_watcher() {
    let (mut env, mut watcher) = create_env();
    assert!(!watcher.has_skill_changed());
    let skill = funera_core::re_act::skills::Skill::new("test-skill", "A test", "Do something");
    env.add_skill(skill).await;
    assert!(watcher.has_skill_changed());
}

#[tokio::test]
async fn env_add_skill_reflects_in_prompt() {
    let (mut env, watcher) = create_env();
    let skill = funera_core::re_act::skills::Skill::new("helper", "Helper skill", "You are a helper.");
    env.add_skill(skill).await;
    let prompt = env.skill_prompt_now();
    // Skills are inactive by default, so they won't appear in the prompt
    assert!(prompt.is_empty(), "inactive skills should not appear in prompt");
}

#[tokio::test]
async fn env_activate_skill_adds_to_prompt() {
    let (mut env, _watcher) = create_env();
    let skill = funera_core::re_act::skills::Skill::new("helper", "Helper skill", "You are a helper.");
    env.add_skill(skill).await;
    assert!(env.activate_skill("helper").await);
    let prompt = env.skill_prompt_now();
    assert!(prompt.contains("You are a helper"), "activated skill content should be in prompt");
}

#[tokio::test]
async fn env_deactivate_skill_removes_from_prompt() {
    let (mut env, _watcher) = create_env();
    let skill = funera_core::re_act::skills::Skill::new("helper", "Helper skill", "You are a helper.");
    env.add_skill(skill).await;
    env.activate_skill("helper").await;
    assert!(env.deactivate_skill("helper").await);
    assert!(env.skill_prompt_now().is_empty(), "deactivated skill should vanish from prompt");
}

#[tokio::test]
async fn env_activate_nonexistent_skill_returns_false() {
    let (mut env, _watcher) = create_env();
    assert!(!env.activate_skill("ghost").await);
    assert!(!env.deactivate_skill("ghost").await);
}

#[tokio::test]
async fn env_remove_skill_works() {
    let (mut env, _watcher) = create_env();
    let skill = funera_core::re_act::skills::Skill::new("removable", "Will be removed", "content");
    env.add_skill(skill).await;
    env.remove_skill("removable").await;
    // The skill is removed from the registry (verify by trying to activate it)
    assert!(!env.activate_skill("removable").await);
}
