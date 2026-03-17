use std::sync::Arc;
use std::sync::OnceLock;

use async_trait::async_trait;
use serde_json::json;

use crate::agent::SpawnService;
use crate::tools::{ToolError, ToolResult};
use crate::tools::base::{
    Tool, ToolContext, ToolDefinition, parse_args, tool_definition_from_json,
};
use crate::types::SessionKey;
use crate::types::tools::SpawnArgs;

// Tool descriptions
const SPAWN_DESC: &str = "Spawn a subagent to handle a task in the background. Use this for complex or time-consuming tasks that can run independently. The subagent will complete the task and report back when done.";
const SPAWN_TASK_DESC: &str = "The task for the subagent to complete";
const SPAWN_LABEL_DESC: &str = "Optional short label for the task (for display)";

pub struct SpawnTool {
    service: Arc<dyn SpawnService>,
}

impl SpawnTool {
    pub fn new(service: Arc<dyn SpawnService>) -> Self {
        Self { service }
    }

    pub fn definition() -> Arc<ToolDefinition> {
        static DEF: OnceLock<Arc<ToolDefinition>> = OnceLock::new();
        DEF.get_or_init(|| {
            Arc::new(tool_definition_from_json(json!({
                "type": "function",
                "function": {
                    "name": "spawn",
                    "description": SPAWN_DESC,
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "task": {
                                "type": "string",
                                "description": SPAWN_TASK_DESC
                            },
                            "label": {
                                "type": "string",
                                "description": SPAWN_LABEL_DESC
                            }
                        },
                        "required": ["task"]
                    }
                }
            })))
        })
        .clone()
    }

    pub(crate) async fn execute_typed(
        &self,
        args: SpawnArgs,
        ctx: &ToolContext,
    ) -> ToolResult<String> {
        Ok(self
            .service
            .spawn(
                args.task,
                args.label,
                if ctx.channel.is_empty() {
                    "cli".to_string()
                } else {
                    ctx.channel.clone()
                },
                if ctx.chat_id.is_empty() {
                    "direct".to_string()
                } else {
                    ctx.chat_id.clone()
                },
                if ctx.session_key.is_empty() {
                    None
                } else {
                    Some(ctx.session_key.clone())
                },
            )
            .await)
    }
}

#[async_trait]
impl Tool for SpawnTool {
    fn name(&self) -> &str {
        "spawn"
    }

    fn definition(&self) -> Arc<ToolDefinition> {
        Self::definition()
    }

    async fn execute(&self, args_json: &str, ctx: &ToolContext) -> ToolResult<String> {
        let parsed = parse_args::<SpawnArgs>(args_json)?;
        self.execute_typed(parsed, ctx).await
    }

    async fn cancel_by_session(&self, session_key: &str) -> ToolResult<usize> {
        self.service
            .cancel_by_session(&SessionKey::from(session_key))
            .await
            .map_err(|err| ToolError::execution(self.name(), err.into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use async_trait::async_trait;

    use crate::agent::SpawnService;
    use crate::provider::ProviderError;
    use crate::provider::{ChatRequest, LLMProvider, LLMResponse, UsageStats};

    #[allow(unused)]
    struct DummyProvider;

    #[async_trait]
    impl LLMProvider for DummyProvider {
        async fn chat(&self, _req: ChatRequest) -> std::result::Result<LLMResponse, ProviderError> {
            Ok(LLMResponse {
                content: Some("done".to_string()),
                tool_calls: Vec::new(),
                finish_reason: "stop".to_string(),
                usage: UsageStats::default(),
                reasoning_content: None,
                thinking_blocks: None,
            })
        }

        fn default_model(&self) -> &str {
            "openai/gpt-4o-mini"
        }
    }

    struct MockSpawnService;

    #[async_trait]
    impl SpawnService for MockSpawnService {
        async fn spawn(
            &self,
            task: String,
            _label: Option<String>,
            _origin_channel: String,
            _origin_chat_id: String,
            _session_key: Option<crate::types::SessionKey>,
        ) -> String {
            format!("Spawned: {}", task)
        }

        async fn cancel_by_session(
            &self,
            _session_key: &crate::types::SessionKey,
        ) -> crate::error::NanobotResult<usize> {
            Ok(1)
        }
    }

    #[test]
    fn definition_requires_task_parameter() {
        let def = SpawnTool::definition();
        assert_eq!(def.function.name, "spawn");
        assert!(
            def.function
                .parameters
                .required
                .contains(&"task".to_string())
        );
    }

    #[tokio::test]
    async fn execute_returns_spawned_message() {
        let service = Arc::new(MockSpawnService);
        let tool = SpawnTool::new(service);

        let ctx = ToolContext {
            channel: "cli".to_string(),
            chat_id: "direct".to_string(),
            session_key: "cli:direct".into(),
            message_id: None,
        };

        let result = tool
            .execute(r#"{"task":"test task"}"#, &ctx)
            .await
            .expect("execute spawn tool");

        assert!(result.contains("Spawned"));
        assert!(result.contains("test task"));
    }

    #[tokio::test]
    async fn cancel_by_session_returns_count() {
        let service = Arc::new(MockSpawnService);
        let tool = SpawnTool::new(service);

        let cancelled = tool.cancel_by_session("test").await.expect("cancel");
        assert_eq!(cancelled, 1);
    }
}
