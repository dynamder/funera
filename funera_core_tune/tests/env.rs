use funera_core::env::FuneraEnv;
use funera_core::re_act::tool::ToolRegistry;
use funera_core_tune::utils::fixtures::default_schema;
use funera_core_tune::utils::mock_tool::MockTool;

fn create_env() -> (FuneraEnv, funera_core::env::FuneraEnvWatcher) {
    let client = async_openai::Client::new();
    let registry = ToolRegistry::new();
    FuneraEnv::new(registry, client, "gpt-4o-mini")
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
    assert_eq!(watcher.watch_model(), "gpt-4o-mini");
    env.set_model("gpt-4");
    assert_eq!(watcher.watch_model(), "gpt-4");
}

#[test]
fn env_set_client() {
    let (mut env, mut watcher) = create_env();
    let new_client = async_openai::Client::new();
    env.set_client(new_client);
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
