use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use regex::Regex;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::{error, info};

use crate::agent::skills::SkillsLoader;
use crate::bus::{InboundMessage, MessageBus, MessageMetadata};
use crate::provider::{
    AssistantFunctionCall, AssistantToolCall, ChatMessage, ChatRequest, LLMProvider,
};
use crate::task_id::TaskId;
use crate::tools::{ToolContext, ToolRegistry};

pub struct SubagentManager {
    provider: Arc<dyn LLMProvider>,
    workspace: std::path::PathBuf,
    bus: Arc<MessageBus>,
    tools: Arc<ToolRegistry>,
    model: String,
    temperature: f32,
    max_tokens: i32,
    reasoning_effort: Option<String>,
    /// task id => task handle
    running_tasks: Mutex<HashMap<TaskId, JoinHandle<()>>>,
    /// session => running tasks
    session_tasks: Mutex<HashMap<String, HashSet<TaskId>>>,
}

impl SubagentManager {
    pub(crate) fn new(
        provider: Arc<dyn LLMProvider>,
        workspace: std::path::PathBuf,
        bus: Arc<MessageBus>,
        tools: Arc<ToolRegistry>,
        model: String,
        temperature: f32,
        max_tokens: i32,
        reasoning_effort: Option<String>,
    ) -> Self {
        Self {
            provider,
            workspace,
            bus,
            tools,
            model,
            temperature,
            max_tokens,
            reasoning_effort,
            running_tasks: Mutex::new(HashMap::new()),
            session_tasks: Mutex::new(HashMap::new()),
        }
    }

    pub async fn spawn(
        self: &Arc<Self>,
        task: String,
        label: Option<String>,
        origin_channel: String,
        origin_chat_id: String,
        session_key: Option<String>,
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
                this.cleanup_task(&task_id, session_key.as_deref()).await;
            }
        });

        {
            let mut running = self.running_tasks.lock().await;
            running.insert(task_id, handle);
        }
        if let Some(session) = session_key {
            let mut sessions = self.session_tasks.lock().await;
            sessions.entry(session).or_default().insert(task_id);
        }

        info!("spawned subagent [{}]: {}", task_id, display_label);
        format!(
            "Subagent [{}] started (id: {}). I'll notify you when it completes.",
            display_label, task_id
        )
    }

    pub async fn cancel_by_session(&self, session_key: &str) -> usize {
        let ids = {
            let mut sessions = self.session_tasks.lock().await;
            sessions
                .remove(session_key)
                .map(|set| set.into_iter().collect::<Vec<_>>())
                .unwrap_or_default()
        };

        if ids.is_empty() {
            return 0;
        }

        let mut cancelled = 0usize;
        let mut running = self.running_tasks.lock().await;
        for id in ids {
            if let Some(handle) = running.remove(&id) {
                if !handle.is_finished() {
                    handle.abort();
                    cancelled += 1;
                }
            }
        }

        cancelled
    }

    async fn cleanup_task(&self, task_id: &TaskId, session_key: Option<&str>) {
        self.running_tasks.lock().await.remove(task_id);
        if let Some(session_key) = session_key {
            let mut sessions = self.session_tasks.lock().await;
            if let Some(ids) = sessions.get_mut(session_key) {
                ids.remove(task_id);
                if ids.is_empty() {
                    sessions.remove(session_key);
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
        info!("subagent [{}] starting: {}", task_id, label);

        let tool_context = ToolContext {
            channel: origin_channel.to_string(),
            chat_id: origin_chat_id.to_string(),
            session_key: format!("{}:{}", origin_channel, origin_chat_id),
            message_id: None,
        };

        let outcome = self.run_subagent_loop(task, &tool_context).await;
        match outcome {
            Ok(result) => {
                info!("subagent [{}] completed", task_id);
                self.announce_result(
                    &task_id.to_string(),
                    label,
                    task,
                    &result,
                    origin_channel,
                    origin_chat_id,
                    "ok",
                );
            }
            Err(err) => {
                error!("subagent [{}] failed: {}", task_id, err);
                self.announce_result(
                    &task_id.to_string(),
                    label,
                    task,
                    &format!("Error: {}", err),
                    origin_channel,
                    origin_chat_id,
                    "error",
                );
            }
        }
    }

    async fn run_subagent_loop(&self, task: &str, tool_context: &ToolContext) -> anyhow::Result<String> {
        let tools = self.tools.definitions();

        let mut messages = vec![
            ChatMessage::system_text(self.build_subagent_prompt()),
            ChatMessage::user_text(task),
        ];

        let mut final_result = None;
        for _ in 0..15 {
            let response = self
                .provider
                .chat(ChatRequest {
                    messages: messages.clone(),
                    tools: Some(tools.clone()),
                    model: Some(self.model.clone()),
                    max_tokens: self.max_tokens,
                    temperature: self.temperature,
                    reasoning_effort: self.reasoning_effort.clone(),
                })
                .await;

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
                    let result = self
                        .tools
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

    fn build_subagent_prompt(&self) -> String {
        let runtime = chrono::Local::now()
            .format("%Y-%m-%d %H:%M (%A)")
            .to_string();
        let mut parts = vec![format!(
            "# Subagent\n\nCurrent Time: {}\n\nYou are a subagent spawned by the main agent to complete a specific task. Stay focused and provide a concise final result.\n\n## Workspace\n{}",
            runtime,
            self.workspace.display(),
        )];

        let skills = SkillsLoader::new(&self.workspace).build_skills_summary();
        if !skills.trim().is_empty() {
            parts.push(format!(
                "## Skills\n\nRead SKILL.md with read_file to use a skill.\n\n{}",
                skills
            ));
        }

        parts.join("\n\n")
    }

    fn announce_result(
        &self,
        _task_id: &str,
        label: &str,
        task: &str,
        result: &str,
        origin_channel: &str,
        origin_chat_id: &str,
        status: &str,
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
            content,
            timestamp: chrono::Utc::now(),
            media: Vec::new(),
            metadata: MessageMetadata::default(),
            session_key_override: None,
        };

        if let Err(err) = self.bus.publish_inbound(msg) {
            error!("failed to publish subagent result: {}", err);
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{ChatRequest, LLMResponse, UsageStats};
    use async_trait::async_trait;

    struct MockProvider {
        response: String,
    }

    #[async_trait]
    impl LLMProvider for MockProvider {
        async fn chat(&self, _req: ChatRequest) -> LLMResponse {
            LLMResponse {
                content: Some(self.response.clone()),
                tool_calls: Vec::new(),
                finish_reason: "stop".to_string(),
                usage: UsageStats::default(),
                reasoning_content: None,
                thinking_blocks: None,
            }
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

        let bus = Arc::new(MessageBus::new());
        let tools = Arc::new(ToolRegistry::new(
            workspace.clone(),
            false,
            crate::config::schema::ExecToolConfig::default(),
            crate::config::schema::WebToolsConfig::default(),
            Some(bus.clone()),
            None,
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
                Some("cli:direct".to_string()),
            )
            .await;

        assert!(result.contains("Subagent"));
        assert!(result.contains("test"));
    }

    #[tokio::test]
    async fn subagent_manager_cancels_by_session() {
        use tokio::time::Duration;

        // Create a provider that delays to ensure task is still running
        struct SlowProvider;

        #[async_trait]
        impl LLMProvider for SlowProvider {
            async fn chat(&self, _req: ChatRequest) -> LLMResponse {
                tokio::time::sleep(Duration::from_secs(1)).await;
                LLMResponse {
                    content: Some("Task completed".to_string()),
                    tool_calls: Vec::new(),
                    finish_reason: "stop".to_string(),
                    usage: UsageStats::default(),
                    reasoning_content: None,
                    thinking_blocks: None,
                }
            }

            fn default_model(&self) -> &str {
                "slow/model"
            }
        }

        let provider = Arc::new(SlowProvider);
        let workspace = std::env::temp_dir().join(format!("nanobot-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&workspace).unwrap();

        let bus = Arc::new(MessageBus::new());
        let tools = Arc::new(ToolRegistry::new(
            workspace.clone(),
            false,
            crate::config::schema::ExecToolConfig::default(),
            crate::config::schema::WebToolsConfig::default(),
            Some(bus.clone()),
            None,
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
                Some("cli:direct".to_string()),
            )
            .await;

        // Give it a moment to start
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Cancel by session
        let cancelled = manager.cancel_by_session("cli:direct").await;
        assert_eq!(cancelled, 1);

        // Verify no tasks remain
        let cancelled_again = manager.cancel_by_session("cli:direct").await;
        assert_eq!(cancelled_again, 0);
    }
}
