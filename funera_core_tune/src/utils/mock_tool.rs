use async_trait::async_trait;
use funera_core::re_act::tool::{Tool, ToolCallError, ToolType};
use serde_json::Value as JsonValue;

pub struct MockTool {
    name: String,
    description: String,
    schema: JsonValue,
    result: Result<String, ToolCallError>,
}

impl MockTool {
    pub fn new(name: impl Into<String>, schema: JsonValue) -> Self {
        let name = name.into();
        let description = format!("Mock tool: {}", &name);
        Self {
            name,
            description,
            schema,
            result: Ok("mock_result".to_string()),
        }
    }

    pub fn with_result(mut self, result: Result<String, ToolCallError>) -> Self {
        self.result = result;
        self
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }
}

#[async_trait]
impl Tool for MockTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn schema(&self) -> JsonValue {
        self.schema.clone()
    }

    fn get_type(&self) -> ToolType {
        ToolType::Function
    }

    async fn execute(&self, _args: JsonValue) -> Result<String, ToolCallError> {
        match &self.result {
            Ok(s) => Ok(s.clone()),
            Err(e) => Err(match e {
                ToolCallError::ParameterMismatch(v) => ToolCallError::ParameterMismatch(v.clone()),
                ToolCallError::ToolExecutionError(e) => {
                    ToolCallError::ToolExecutionError(anyhow::anyhow!("{}", e))
                }
                ToolCallError::ToolUnavailable(s) => ToolCallError::ToolUnavailable(s.clone()),
                ToolCallError::ToolNotFound(s) => ToolCallError::ToolNotFound(s.clone()),
            }),
        }
    }
}
