//! Subagent spawning tool for background task execution.
//!
//! Provides the `spawn` tool that allows the LLM to delegate complex or
//! time-consuming tasks to independent subagents running in the background.
//! The subagent completes the task and reports back when done.
//!
//! ## Architecture
//!
//! ```text
//! SpawnTool (Tool impl) → SpawnService (trait) → SubagentManager (impl)
//! ```
//!
//! The [`SpawnService`] trait decouples the tool from the actual subagent
//! management implementation, allowing the spawn service to be injected
//! via [`ToolRegistry::set_spawn_service`](crate::registry::ToolRegistry::set_spawn_service).
//! This breaks circular dependencies: the subagent manager needs the tool
//! registry, and the tool registry needs the spawn service.

use std::sync::Arc;
use std::sync::OnceLock;

use async_trait::async_trait;
use serde_json::json;

use nanobot_types::SessionKey;
use nanobot_types::tools::SpawnArgs;

use crate::base::{Tool, ToolContext, ToolDefinition, parse_args, tool_definition_from_json};
use crate::error::{ToolError, ToolResult};

// Tool descriptions
const SPAWN_DESC: &str = "Spawn a subagent to handle a task in the background. Use this for complex or time-consuming tasks that can run independently. The subagent will complete the task and report back when done.";
const SPAWN_TASK_DESC: &str = "The task for the subagent to complete";
const SPAWN_LABEL_DESC: &str = "Optional short label for the task (for display)";

/// Trait for spawning background subagent tasks.
///
/// This trait abstracts the subagent spawning mechanism behind an
/// interface, allowing the tool to be decoupled from the concrete
/// subagent manager implementation.
///
/// Implementations must be `Send + Sync` for use across async boundaries.
///
/// # Flow
///
/// ```text
/// AgentLoop → SpawnTool → SpawnService::spawn() → Background subagent
/// ```
#[async_trait]
pub trait SpawnService: Send + Sync {
    /// Spawns a background subagent task.
    ///
    /// # Arguments
    ///
    /// * `task` - The natural-language task description for the subagent.
    /// * `label` - Optional short label for display/tracking purposes.
    /// * `origin_channel` - The channel from which the spawn was requested.
    /// * `origin_chat_id` - The chat/conversation from which the spawn was requested.
    /// * `session_key` - Optional session key for scoping the subagent's context.
    ///
    /// # Returns
    ///
    /// A human-readable string indicating the spawned task (e.g., task ID).
    async fn spawn(
        &self,
        task: String,
        label: Option<String>,
        origin_channel: String,
        origin_chat_id: String,
        session_key: Option<SessionKey>,
    ) -> String;

    /// Cancels all subagent tasks associated with a session.
    ///
    /// Called when a session ends or is interrupted to ensure background
    /// tasks do not continue running orphaned.
    ///
    /// # Returns
    ///
    /// The number of tasks that were cancelled.
    async fn cancel_by_session(&self, session_key: &SessionKey) -> anyhow::Result<usize>;
}

/// Tool for spawning background subagent tasks.
///
/// Delegates to a [`SpawnService`] implementation for the actual
/// subagent lifecycle management.
pub struct SpawnTool {
    service: Arc<dyn SpawnService>,
}

impl SpawnTool {
    /// Creates a new `SpawnTool` backed by the given spawn service.
    pub fn new(service: Arc<dyn SpawnService>) -> Self {
        Self { service }
    }

    /// Returns the static tool definition (name: "spawn").
    ///
    /// Uses a `OnceLock` to cache the definition after first construction.
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

    /// Executes the spawn tool with strongly-typed arguments.
    ///
    /// Falls back to defaults if the session context is empty
    /// (e.g., "cli" channel, "direct" chat).
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

    /// Cancels all spawns for the given session.
    async fn cancel_by_session(&self, session_key: &str) -> ToolResult<usize> {
        self.service
            .cancel_by_session(&SessionKey::from(session_key))
            .await
            .map_err(|err| ToolError::execution(self.name(), err))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use async_trait::async_trait;

    struct MockSpawnService;

    #[async_trait]
    impl SpawnService for MockSpawnService {
        async fn spawn(
            &self,
            task: String,
            _label: Option<String>,
            _origin_channel: String,
            _origin_chat_id: String,
            _session_key: Option<SessionKey>,
        ) -> String {
            format!("Spawned: {}", task)
        }

        async fn cancel_by_session(&self, _session_key: &SessionKey) -> anyhow::Result<usize> {
            Ok(1)
        }
    }

    #[test]
    fn definition_has_required_task_param() {
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
