#![cfg(feature = "real-llm")]

use funera_core::chat::message::{FuneraMessage, MsgVariant, Role, TextMessage};
use funera_core::chat::session::{FuneraSession, Idle};
use funera_core::env::FuneraEnv;
use funera_core::event_bus::env_state_bus::EnvStateBus;
use funera_core::event_bus::tool_bus::ToolBus;
use funera_core::re_act::tool::{Tool, ToolCallError, ToolRegistry, ToolType};
use funera_core::re_act::tool_executor::ToolExecutor;
use funera_core::re_act::ReActLoopConfig;
use funera_core_tune::utils::env_config::LlmConfig;
use std::sync::Arc;
use tokio::sync::RwLock;

/// A simple weather-lookup tool for real LLM integration testing.
struct WeatherTool;

#[async_trait::async_trait]
impl Tool for WeatherTool {
    fn name(&self) -> &str {
        "get_weather"
    }
    fn description(&self) -> &str {
        "Get the current weather for a city"
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "get_weather",
                "description": "Get the current weather for a city",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "city": {
                            "type": "string",
                            "description": "The city name"
                        }
                    },
                    "required": ["city"]
                }
            }
        })
    }
    fn get_type(&self) -> ToolType {
        ToolType::Function
    }
    async fn execute(&self, args: serde_json::Value) -> std::result::Result<String, ToolCallError> {
        let city = args["city"].as_str().unwrap_or("unknown");
        Ok(format!("The weather in {} is 22°C and sunny.", city))
    }
}

#[tokio::test]
async fn real_llm_simple_chat() {
    let config = LlmConfig::from_env().expect("Failed to load LLM config from environment");
    let client = config.build_client();

    let registry = ToolRegistry::new();
    let (_env, _env_watcher) = FuneraEnv::new(registry, client, &config.model);

    // Verify the client can reach the API with a simple chat completion
    let request = async_openai::types::chat::CreateChatCompletionRequestArgs::default()
        .model(&config.model)
        .messages([async_openai::types::chat::ChatCompletionRequestMessage::User(
            async_openai::types::chat::ChatCompletionRequestUserMessageArgs::default()
                .content("Respond with exactly: OK")
                .build()
                .unwrap(),
        )])
        .max_tokens(10u32)
        .build()
        .unwrap();

    let response = _env_watcher
        .use_client()
        .chat()
        .create(request)
        .await
        .expect("Real LLM API call failed");

    let content = response.choices.first()
        .and_then(|c| c.message.content.as_deref())
        .unwrap_or("");

    assert!(!content.is_empty(), "LLM response should not be empty");
    eprintln!("Real LLM test response: {}", content);
}

#[tokio::test]
async fn real_llm_with_tool_calling() {
    let config = LlmConfig::from_env().expect("Failed to load LLM config");
    let client = config.build_client();

    let mut tool_registry = ToolRegistry::new();
    tool_registry.add_tool(Box::new(WeatherTool));
    let tool_registry = Arc::new(RwLock::new(tool_registry));

    let (tool_bus, exec_rx) = ToolBus::new();
    tokio::spawn(ToolExecutor::new(tool_registry, exec_rx).run());

    let (_state_bus, turn_highway_handle) = EnvStateBus::new();
    let (env, env_watcher) = FuneraEnv::new(ToolRegistry::new(), client, &config.model);
    let (env_state_tx, _env_state_rx) = tokio::sync::broadcast::channel(20);

    // Ask the LLM about weather to trigger tool calling
    let session = FuneraSession::<Idle>::new();
    let mut running = session.run();

    let init_msg = FuneraMessage::new(
        Role::User,
        MsgVariant::Text(TextMessage {
            text: "What is the weather in Tokyo?".into(),
            reasoning_content: None,
        }),
    );

    let config = ReActLoopConfig::new(
        10,
        3,
        env_watcher,
        tool_bus,
        env_state_tx,
        turn_highway_handle,
    );

    let result = running
        .react_loop(init_msg, config, _state_bus.env_state_tx.clone())
        .await;

    assert!(result.is_ok(), "Real LLM tool-call session should complete: {:?}", result.err());
    eprintln!("Real LLM tool-call session completed successfully");
}
