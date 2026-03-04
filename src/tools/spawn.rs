use std::sync::Arc;
use std::sync::OnceLock;

use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;

use crate::agent::SubagentManager;
use crate::tools::base::{JsonSchema, Tool, ToolContext, ToolDefinition, parse_args, schema_props};

#[derive(Debug, Deserialize)]
pub(crate) struct SpawnArgs {
    task: String,
    label: Option<String>,
}

pub struct SpawnTool {
    manager: Arc<SubagentManager>,
}

impl SpawnTool {
    pub fn new(manager: Arc<SubagentManager>) -> Self {
        Self { manager }
    }

    pub fn definition() -> ToolDefinition {
        static DEF: OnceLock<ToolDefinition> = OnceLock::new();
        DEF.get_or_init(|| {
            ToolDefinition::function(
                "spawn",
                "Spawn a subagent to handle a task in the background. Use this for complex or time-consuming tasks that can run independently. The subagent will complete the task and report back when done.",
                JsonSchema::object(
                    schema_props([
                        (
                            "task",
                            JsonSchema::string(Some("The task for the subagent to complete")),
                        ),
                        (
                            "label",
                            JsonSchema::string(Some(
                                "Optional short label for the task (for display)",
                            )),
                        ),
                    ]),
                    vec!["task"],
                ),
            )
        })
        .clone()
    }

    pub(crate) async fn execute_typed(&self, args: SpawnArgs, ctx: &ToolContext) -> Result<String> {
        Ok(self
            .manager
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

    pub async fn cancel_by_session(&self, session_key: &str) -> usize {
        self.manager.cancel_by_session(session_key).await
    }
}

#[async_trait]
impl Tool for SpawnTool {
    fn name(&self) -> &str {
        "spawn"
    }

    fn definition(&self) -> ToolDefinition {
        Self::definition()
    }

    async fn execute(&self, args_json: &str, ctx: &ToolContext) -> Result<String> {
        let parsed = parse_args::<SpawnArgs>(args_json)?;
        self.execute_typed(parsed, ctx).await
    }

    async fn cancel_by_session(&self, session_key: &str) -> Result<usize> {
        Ok(self.cancel_by_session(session_key).await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use async_trait::async_trait;

    use crate::bus::MessageBus;
    use crate::config::schema::{ExecToolConfig, WebToolsConfig};
    use crate::provider::{ChatRequest, LLMProvider, LLMResponse, UsageStats};

    struct DummyProvider;

    #[async_trait]
    impl LLMProvider for DummyProvider {
        async fn chat(&self, _req: ChatRequest) -> LLMResponse {
            LLMResponse {
                content: Some("done".to_string()),
                tool_calls: Vec::new(),
                finish_reason: "stop".to_string(),
                usage: UsageStats::default(),
                reasoning_content: None,
                thinking_blocks: None,
            }
        }

        fn default_model(&self) -> &str {
            "openai/gpt-4o-mini"
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
    #[ignore] // TODO: Update test after SubagentManager refactoring
    async fn execute_returns_started_message() {
        // This test needs to be updated to work with the new SubagentManager
        // that requires ToolRegistry, which creates a circular dependency in tests.
        // Consider creating a mock ToolRegistry or restructuring the test.
    }
}
