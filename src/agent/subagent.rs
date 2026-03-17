use std::sync::Arc;

use async_trait::async_trait;
use dashmap::DashMap;
use regex::Regex;
use tokio::task::JoinHandle;
use tracing::{error, info};

use crate::agent::skills::SkillsLoader;
use crate::agent::traits::SpawnService;
use crate::bus::{InboundMessage, MessageBus, MessageMetadata};
use crate::error::NanobotResult;
use crate::observability::TARGET_SUBAGENT;
use crate::provider::{
    AssistantFunctionCall, AssistantToolCall, ChatMessage, ChatRequest, LLMProvider,
};
use crate::tools::{ToolContext, ToolRegistry};
use crate::types::SessionKey;
use crate::types::task::TaskId;

const SUBAGENT_PROMPT_TEMPLATE: &str = "# Subagent\n\nCurrent Time: {runtime}\n\nYou are a subagent spawned by the main agent to complete a specific task. Stay focused and provide a concise final result.\n\n## Workspace\n{workspace}";
const SUBAGENT_SKILLS_PREAMBLE: &str =
    "## Skills\n\nRead SKILL.md with read_file to use a skill.\n\n";

struct SubagentManagerInner {
    provider: Arc<dyn LLMProvider>,
    workspace: std::path::PathBuf,
    bus: MessageBus,
    tools: Arc<ToolRegistry>,
    model: String,
    temperature: f32,
    max_tokens: i32,
    reasoning_effort: Option<String>,
    /// task id => task handle
    /// Using DashMap for lock-free concurrent access
    running_tasks: DashMap<TaskId, JoinHandle<()>>,
    /// session => running tasks
    /// Using nested DashMap for better concurrent access
    session_tasks: DashMap<SessionKey, DashMap<TaskId, ()>>,
}

impl std::fmt::Debug for SubagentManagerInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubagentManagerInner")
            .field("workspace", &self.workspace)
            .field("model", &self.model)
            .field("temperature", &self.temperature)
            .field("max_tokens", &self.max_tokens)
            .field("reasoning_effort", &self.reasoning_effort)
            .field("running_tasks", &"<DashMap>")
            .field("session_tasks", &"<DashMap>")
            .finish()
    }
}

#[derive(Clone, Debug)]
pub struct SubagentManager {
    inner: Arc<SubagentManagerInner>,
}

impl SubagentManager {
    pub(crate) fn new(
        provider: Arc<dyn LLMProvider>,
        workspace: std::path::PathBuf,
        bus: MessageBus,
        tools: Arc<ToolRegistry>,
        model: String,
        temperature: f32,
        max_tokens: i32,
        reasoning_effort: Option<String>,
    ) -> Self {
        Self {
            inner: Arc::new(SubagentManagerInner {
                provider,
                workspace,
                bus,
                tools,
                model,
                temperature,
                max_tokens,
                reasoning_effort,
                running_tasks: DashMap::new(),
                session_tasks: DashMap::new(),
            }),
        }
    }

    pub async fn cancel_by_session(&self, session_key: &SessionKey) -> usize {
        self.inner.cancel_by_session(session_key).await
    }
}

impl SubagentManagerInner {
    async fn spawn_impl(
        self: &Arc<Self>,
        task: String,
        label: Option<String>,
        origin_channel: String,
        origin_chat_id: String,
        session_key: Option<SessionKey>,
    ) -> String {
        let task_id = TaskId::new();
        let display_label = label.unwrap_or_else(|| truncate(&task, 30));

        let this = self.clone();
        let handle = tokio::spawn({
            let session_key = session_key.clone();
            let display_label = display_label.clone();

            async move {
                this.run_subagent(
                    &task_id,
                    &task,
                    &display_label,
                    &origin_channel,
                    &origin_chat_id,
                )
                .await;
                this.cleanup_task(&task_id, session_key.as_ref()).await;
            }
        });

        self.running_tasks.insert(task_id, handle);
        if let Some(session) = session_key {
            self.session_tasks
                .entry(session)
                .or_insert_with(DashMap::new)
                .insert(task_id, ());
        }

        info!(
            target: TARGET_SUBAGENT,
            "spawned subagent [{}]: {}",
            task_id,
            display_label
        );
        format!(
            "Subagent [{}] started (id: {}). I'll notify you when it completes.",
            display_label, task_id
        )
    }

    async fn cancel_by_session(&self, session_key: &SessionKey) -> usize {
        // Remove all task IDs for this session
        let ids = if let Some((_, tasks)) = self.session_tasks.remove(session_key) {
            tasks.into_iter().map(|(id, _)| id).collect::<Vec<_>>()
        } else {
            return 0;
        };

        let mut cancelled = 0usize;
        for id in ids {
            if let Some((_, handle)) = self.running_tasks.remove(&id) {
                if !handle.is_finished() {
                    handle.abort();
                    cancelled += 1;
                }
            }
        }

        cancelled
    }

    async fn cleanup_task(&self, task_id: &TaskId, session_key: Option<&SessionKey>) {
        self.running_tasks.remove(task_id);
        if let Some(session_key) = session_key {
            if let Some(tasks) = self.session_tasks.get(session_key) {
                tasks.remove(task_id);
                if tasks.is_empty() {
                    drop(tasks);
                    self.session_tasks.remove(session_key);
                }
            }
        }
    }

    async fn run_subagent(
        &self,
        task_id: &TaskId,
        task: &str,
        label: &str,
        origin_channel: &str,
        origin_chat_id: &str,
    ) {
        info!(
            target: TARGET_SUBAGENT,
            "subagent [{}] starting: {}",
            task_id,
            label
        );

        let tool_context = ToolContext {
            channel: origin_channel.to_string(),
            chat_id: origin_chat_id.to_string(),
            session_key: SessionKey::new(origin_channel, origin_chat_id),
            message_id: None,
        };

        let outcome = run_subagent_loop_impl(
            task,
            &tool_context,
            self.provider.as_ref(),
            &self.workspace,
            self.tools.as_ref(),
            &self.model,
            self.temperature,
            self.max_tokens,
            self.reasoning_effort.as_deref(),
        )
        .await;

        match outcome {
            Ok(result) => {
                info!(target: TARGET_SUBAGENT, "subagent [{}] completed", task_id);
                announce_result_impl(
                    &task_id.to_string(),
                    label,
                    task,
                    &result,
                    origin_channel,
                    origin_chat_id,
                    "ok",
                    &self.bus,
                );
            }
            Err(err) => {
                error!(
                    target: TARGET_SUBAGENT,
                    "subagent [{}] failed: {}",
                    task_id,
                    err
                );
                announce_result_impl(
                    &task_id.to_string(),
                    label,
                    task,
                    &format!("Error: {}", err),
                    origin_channel,
                    origin_chat_id,
                    "error",
                    &self.bus,
                );
            }
        }
    }
}

/// Implement SpawnService trait for SubagentManager.
///
/// This allows SubagentManager to be used as a SpawnService without
/// creating a circular dependency with ToolRegistry.
///
/// The trait implementation delegates to the internal Arc-based implementation,
/// ensuring consistent behavior and proper resource cleanup.
#[async_trait]
impl SpawnService for SubagentManager {
    async fn spawn(
        &self,
        task: String,
        label: Option<String>,
        origin_channel: String,
        origin_chat_id: String,
        session_key: Option<SessionKey>,
    ) -> String {
        self.inner
            .spawn_impl(task, label, origin_channel, origin_chat_id, session_key)
            .await
    }

    async fn cancel_by_session(&self, session_key: &SessionKey) -> NanobotResult<usize> {
        Ok(self.inner.cancel_by_session(session_key).await)
    }
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out = String::new();
    for c in text.chars().take(max) {
        out.push(c);
    }
    out.push_str("...");
    out
}

fn strip_think(text: Option<&str>) -> Option<String> {
    let Some(t) = text else {
        return None;
    };
    let re = Regex::new(r"<think>[\s\S]*?</think>").ok()?;
    let cleaned = re.replace_all(t, "").trim().to_string();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

/// Helper function to run the subagent loop logic.
/// Extracted to be reusable from both the Arc-based spawn method and the trait implementation.
async fn run_subagent_loop_impl(
    task: &str,
    tool_context: &ToolContext,
    provider: &dyn LLMProvider,
    workspace: &std::path::Path,
    tools: &ToolRegistry,
    model: &str,
    temperature: f32,
    max_tokens: i32,
    reasoning_effort: Option<&str>,
) -> NanobotResult<String> {
    let tool_defs = tools.definitions();

    let runtime = chrono::Local::now()
        .format("%Y-%m-%d %H:%M (%A)")
        .to_string();
    let mut parts = vec![
        SUBAGENT_PROMPT_TEMPLATE
            .replace("{runtime}", &runtime)
            .replace("{workspace}", &workspace.display().to_string()),
    ];

    let skills = SkillsLoader::new(workspace).build_skills_summary().await;
    if !skills.trim().is_empty() {
        parts.push(format!("{SUBAGENT_SKILLS_PREAMBLE}{skills}"));
    }

    let system_prompt = parts.join("\n\n");

    let mut messages = vec![
        ChatMessage::system_text(system_prompt),
        ChatMessage::user_text(task),
    ];

    let mut final_result = None;
    const MAX_ITERATOR: usize = 15;
    for _ in 0..MAX_ITERATOR {
        let response = provider
            .chat(ChatRequest {
                session_key: Some(tool_context.session_key.clone()),
                messages: messages.clone(),
                tools: Some(tool_defs.clone()),
                model: Some(model.to_string()),
                max_tokens,
                temperature,
                reasoning_effort: reasoning_effort.map(|s| s.to_string()),
            })
            .await
            .map_err(|e| anyhow::anyhow!("Subagent LLM provider error: {}", e))?;

        // when tool call the sub agent will end task
        if response.has_tool_calls() {
            let tool_calls = response
                .tool_calls
                .iter()
                .map(|tc| AssistantToolCall {
                    id: tc.id.clone(),
                    kind: "function".to_string(),
                    function: AssistantFunctionCall {
                        name: tc.name.to_string(),
                        arguments: tc.arguments_json.clone(),
                    },
                })
                .collect::<Vec<_>>();

            messages.push(ChatMessage::assistant(
                response.content,
                Some(tool_calls),
                response.reasoning_content,
                response.thinking_blocks,
            ));

            for call in response.tool_calls {
                let result = tools
                    .execute(call.name.as_str(), &call.arguments_json, tool_context)
                    .await;

                let rendered = match result {
                    Ok(v) => v,
                    Err(err) => format!("Error: {}", err),
                };

                messages.push(ChatMessage::tool_result(
                    call.id,
                    call.name.to_string(),
                    rendered,
                ));
            }
        } else {
            final_result = strip_think(response.content.as_deref());
            break;
        }
    }

    Ok(final_result
        .unwrap_or_else(|| "Task completed but no final response was generated.".to_string()))
}

/// Helper function to announce subagent results.
fn announce_result_impl(
    _task_id: &str,
    label: &str,
    task: &str,
    result: &str,
    origin_channel: &str,
    origin_chat_id: &str,
    status: &str,
    bus: &MessageBus,
) {
    let status_text = if status == "ok" {
        "completed successfully"
    } else {
        "failed"
    };

    let content = format!(
        "[Subagent '{}' {}]\n\nTask: {}\n\nResult:\n{}\n\nSummarize this naturally for the user. Keep it brief (1-2 sentences). Do not mention technical details like subagent or task IDs.",
        label, status_text, task, result
    );

    let msg = InboundMessage {
        channel: "system".to_string(),
        sender_id: "subagent".to_string(),
        chat_id: format!("{}:{}", origin_channel, origin_chat_id),
        content: content.into(),
        timestamp: chrono::Utc::now(),
        media: Vec::new(),
        metadata: MessageMetadata::default(),
        session_key_override: None,
    };

    // Inject as system message to trigger main agent
    if let Err(err) = bus.publish_inbound(msg) {
        error!(
            target: TARGET_SUBAGENT,
            "failed to publish subagent result: {}",
            err
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ProviderError;
    use crate::provider::{ChatRequest, LLMResponse, UsageStats};
    use async_trait::async_trait;

    #[derive(Debug)]
    struct MockProvider {
        response: String,
    }

    #[async_trait]
    impl LLMProvider for MockProvider {
        async fn chat(&self, _req: ChatRequest) -> std::result::Result<LLMResponse, ProviderError> {
            Ok(LLMResponse {
                content: Some(self.response.clone()),
                tool_calls: Vec::new(),
                finish_reason: "stop".to_string(),
                usage: UsageStats::default(),
                reasoning_content: None,
                thinking_blocks: None,
            })
        }

        fn default_model(&self) -> &str {
            "mock/model"
        }
    }

    #[test]
    fn truncate_respects_max_length() {
        let text = "Hello, World!";
        assert_eq!(truncate(text, 5), "Hello...");
        assert_eq!(truncate(text, 20), text);
        assert_eq!(truncate(text, 13), text);
    }

    #[test]
    fn truncate_handles_unicode() {
        let text = "你好世界";
        assert_eq!(truncate(text, 2), "你好...");
    }

    #[test]
    fn strip_think_removes_think_tags() {
        let text = "<think>internal thoughts</think>final answer";
        assert_eq!(strip_think(Some(text)), Some("final answer".to_string()));

        let only_think = "<think>only thoughts</think>";
        assert_eq!(strip_think(Some(only_think)), None);

        let no_think = "just text";
        assert_eq!(strip_think(Some(no_think)), Some("just text".to_string()));

        assert_eq!(strip_think(None), None);
    }

    #[tokio::test]
    async fn subagent_manager_spawns_task() {
        let provider = Arc::new(MockProvider {
            response: "Task completed".to_string(),
        });
        let workspace = std::env::temp_dir().join(format!("nanobot-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&workspace).unwrap();

        let bus = MessageBus::new();
        let tools = Arc::new(ToolRegistry::new(
            workspace.clone(),
            false,
            crate::config::schema::ExecToolConfig::default(),
            crate::config::schema::WebToolsConfig::default(),
            Some(bus.clone()),
            None,
        ));

        let manager = Arc::new(SubagentManager::new(
            provider,
            workspace,
            bus,
            tools,
            "test/model".to_string(),
            0.1,
            1000,
            None,
        ));

        let result = manager
            .spawn(
                "test task".to_string(),
                Some("test".to_string()),
                "cli".to_string(),
                "direct".to_string(),
                Some(SessionKey::from("cli:direct")),
            )
            .await;

        assert!(result.contains("Subagent"));
        assert!(result.contains("test"));
    }

    #[tokio::test]
    async fn subagent_manager_cancels_by_session() {
        use tokio::time::Duration;

        // Create a provider that delays to ensure task is still running
        #[derive(Debug)]
        struct SlowProvider;

        #[async_trait]
        impl LLMProvider for SlowProvider {
            async fn chat(
                &self,
                _req: ChatRequest,
            ) -> std::result::Result<LLMResponse, ProviderError> {
                tokio::time::sleep(Duration::from_secs(1)).await;
                Ok(LLMResponse {
                    content: Some("Task completed".to_string()),
                    tool_calls: Vec::new(),
                    finish_reason: "stop".to_string(),
                    usage: UsageStats::default(),
                    reasoning_content: None,
                    thinking_blocks: None,
                })
            }

            fn default_model(&self) -> &str {
                "slow/model"
            }
        }

        let provider = Arc::new(SlowProvider);
        let workspace = std::env::temp_dir().join(format!("nanobot-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&workspace).unwrap();

        let bus = MessageBus::new();
        let tools = Arc::new(ToolRegistry::new(
            workspace.clone(),
            false,
            crate::config::schema::ExecToolConfig::default(),
            crate::config::schema::WebToolsConfig::default(),
            Some(bus.clone()),
            None,
        ));

        let manager = Arc::new(SubagentManager::new(
            provider,
            workspace,
            bus,
            tools,
            "test/model".to_string(),
            0.1,
            1000,
            None,
        ));

        // Spawn a task
        manager
            .spawn(
                "long running task".to_string(),
                None,
                "cli".to_string(),
                "direct".to_string(),
                Some(SessionKey::from("cli:direct")),
            )
            .await;

        // Give it a moment to start
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Cancel by session
        let cancelled = manager
            .cancel_by_session(&SessionKey::from("cli:direct"))
            .await;
        assert_eq!(cancelled, 1);

        // Verify no tasks remain
        let cancelled_again = manager
            .cancel_by_session(&SessionKey::from("cli:direct"))
            .await;
        assert_eq!(cancelled_again, 0);
    }
}
