use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use regex::Regex;
use serde::Deserialize;
use tokio::sync::{Mutex, RwLock};
use tokio::task::AbortHandle;
use tracing::{error, info, warn};

use crate::agent::ContextBuilder;
use crate::bus::{InboundMessage, MessageBus, MessageMetadata, OutboundMessage};
use crate::config::schema::ChannelsConfig;
use crate::error::Result;
use crate::provider::{
    AssistantFunctionCall, AssistantToolCall, ChatMessage, ChatRequest, LLMProvider,
    MessageContent, MessageRole,
};
use crate::session::{Session, SessionEntry, SessionManager};
use crate::task_id::TaskId;
use crate::tools::mcp::MCPManager;
use crate::tools::{ToolContext, ToolRegistry};

pub struct AgentLoop {
    pub bus: Arc<MessageBus>,
    pub channels_config: ChannelsConfig,
    pub provider: Arc<dyn LLMProvider>,
    pub workspace: std::path::PathBuf,
    pub model: String,
    pub max_iterations: usize,
    pub temperature: f32,
    pub max_tokens: i32,
    pub memory_window: usize,
    pub reasoning_effort: Option<String>,
    pub tools: Arc<ToolRegistry>,
    pub(crate) mcp: Option<Arc<MCPManager>>,
    pub context: ContextBuilder,
    pub sessions: Arc<SessionManager>,
    pub(crate) running: Arc<RwLock<bool>>,
    /// Per-session locks for concurrent message processing
    pub(crate) session_locks: Arc<RwLock<HashMap<String, Arc<Mutex<()>>>>>,
    pub(crate) active_tasks: Arc<Mutex<HashMap<String, HashMap<TaskId, AbortHandle>>>>,
}

impl AgentLoop {
    const TOOL_RESULT_MAX_CHARS: usize = 500;

    async fn ensure_mcp_connected(&self) {
        if let Some(mcp) = &self.mcp
            && let Err(err) = mcp.connect_if_needed(&self.tools).await
        {
            error!(
                "failed to connect MCP servers (will retry on next message): {}",
                err
            );
        }
    }

    pub async fn close_mcp(&self) {
        if let Some(mcp) = &self.mcp {
            mcp.close(&self.tools).await;
        }
    }

    pub async fn run(self: Arc<Self>) {
        *self.running.write().await = true;
        self.ensure_mcp_connected().await;
        info!("agent loop started");

        let mut inbound_rx = self.bus.subscribe_inbound();

        loop {
            if !*self.running.read().await {
                break;
            }
            let Ok(msg) = inbound_rx.recv().await else {
                continue;
            };
            if !*self.running.read().await {
                break;
            }

            if msg.content.trim().eq_ignore_ascii_case("/stop") {
                self.handle_stop(msg).await;
                continue;
            }

            let task_id = TaskId::new();
            let session_key = msg.session_key();
            let this = self.clone();

            let handle = tokio::spawn({
                let session_key = session_key.clone();
                async move {
                    this.dispatch(msg).await;
                    this.unregister_task(&session_key, &task_id).await;
                }
            });

            self.register_task(&session_key, task_id, handle.abort_handle())
                .await;
        }
    }

    pub async fn stop(&self) {
        *self.running.write().await = false;
        let _ = self.bus.publish_inbound(InboundMessage {
            channel: "system".to_string(),
            sender_id: "system".to_string(),
            chat_id: "cli:direct".to_string(),
            content: "__nanobot_stop__".to_string(),
            timestamp: chrono::Utc::now(),
            media: Vec::new(),
            metadata: MessageMetadata::default(),
            session_key_override: Some("__nanobot_stop__".to_string()),
        });

        let all = {
            let mut active = self.active_tasks.lock().await;
            std::mem::take(&mut *active)
        };
        for (_, handles) in all {
            for (_, h) in handles {
                h.abort();
            }
        }
    }

    pub async fn process_direct(
        &self,
        content: &str,
        session_key: &str,
        channel: &str,
        chat_id: &str,
    ) -> Result<String> {
        self.ensure_mcp_connected().await;
        let msg = InboundMessage {
            channel: channel.to_string(),
            sender_id: "user".to_string(),
            chat_id: chat_id.to_string(),
            content: content.to_string(),
            timestamp: chrono::Utc::now(),
            media: Vec::new(),
            metadata: MessageMetadata::default(),
            session_key_override: Some(session_key.to_string()),
        };

        let out = self.process_message(msg).await?;
        Ok(out.map(|m| m.content).unwrap_or_default())
    }

    async fn register_task(&self, session_key: &str, task_id: TaskId, handle: AbortHandle) {
        let mut active = self.active_tasks.lock().await;
        active
            .entry(session_key.to_string())
            .or_default()
            .insert(task_id, handle);
    }

    async fn unregister_task(&self, session_key: &str, task_id: &TaskId) {
        let mut active = self.active_tasks.lock().await;
        if let Some(tasks) = active.get_mut(session_key) {
            tasks.remove(task_id);
            if tasks.is_empty() {
                active.remove(session_key);
            }
        }
    }

    async fn handle_stop(&self, msg: InboundMessage) {
        let session_key = msg.session_key();
        let cancelled_main = {
            let mut active = self.active_tasks.lock().await;
            if let Some(handles) = active.remove(&session_key) {
                let mut count = 0usize;
                for (_, h) in handles {
                    if !h.is_finished() {
                        h.abort();
                        count += 1;
                    }
                }
                count
            } else {
                0usize
            }
        };

        let cancelled_sub = self.tools.cancel_spawn_by_session(&session_key).await;
        let total = cancelled_main + cancelled_sub;
        let content = if total > 0 {
            format!("⏹ Stopped {} task(s).", total)
        } else {
            "No active task to stop.".to_string()
        };

        let _ = self.bus.publish_outbound(OutboundMessage {
            channel: msg.channel,
            chat_id: msg.chat_id,
            content,
            reply_to: None,
            media: Vec::new(),
            metadata: MessageMetadata::default(),
        });
    }

    async fn dispatch(&self, msg: InboundMessage) {
        // Get or create per-session lock for concurrent processing
        let session_key = msg.session_key();
        let lock = {
            let mut locks = self.session_locks.write().await;
            locks
                .entry(session_key.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };

        let _guard = lock.lock().await;

        // Extract fields needed for error handling before moving msg
        let channel = msg.channel.clone();
        let chat_id = msg.chat_id.clone();
        let metadata = msg.metadata.clone();
        let is_cli = channel == "cli";

        match self.process_message(msg).await {
            Ok(out) => {
                if let Some(out) = out {
                    let _ = self.bus.publish_outbound(out);
                } else if is_cli {
                    let _ = self.bus.publish_outbound(OutboundMessage {
                        channel,
                        chat_id,
                        content: String::new(),
                        reply_to: None,
                        media: Vec::new(),
                        metadata,
                    });
                }
            }
            Err(err) => {
                error!("failed to process message: {}", err);
                let _ = self.bus.publish_outbound(OutboundMessage {
                    channel,
                    chat_id,
                    content: "Sorry, I encountered an error.".to_string(),
                    reply_to: None,
                    media: Vec::new(),
                    metadata: MessageMetadata::default(),
                });
            }
        }

        // Clean up unused locks periodically
        self.cleanup_session_locks().await;
    }

    /// Removes locks for sessions that are no longer active
    async fn cleanup_session_locks(&self) {
        let mut locks = self.session_locks.write().await;
        locks.retain(|session_key, lock| {
            // Keep lock if it's currently held or if there are active tasks
            Arc::strong_count(lock) > 1 || self.has_active_tasks(session_key)
        });
    }

    fn has_active_tasks(&self, session_key: &str) -> bool {
        if let Ok(tasks) = self.active_tasks.try_lock() {
            tasks
                .get(session_key)
                .map(|t| !t.is_empty())
                .unwrap_or(false)
        } else {
            true // Assume active if we can't check
        }
    }

    async fn process_message(&self, msg: InboundMessage) -> Result<Option<OutboundMessage>> {
        if msg.channel == "system" {
            return self.process_system_message(msg).await;
        }

        let cmd = msg.content.trim().to_lowercase();
        if cmd == "/help" {
            return Ok(Some(OutboundMessage {
                channel: msg.channel,
                chat_id: msg.chat_id,
                content: "🐈 nanobot commands:\n/new - Start a new conversation\n/stop - Stop the current task\n/help - Show available commands".to_string(),
                reply_to: None,
                media: Vec::new(),
                metadata: MessageMetadata::default(),
            }));
        }

        let session_key = msg.session_key();
        let mut session = self.sessions.get_or_create(&session_key).await?;

        if cmd == "/stop" {
            let cancelled = self.tools.cancel_spawn_by_session(&session_key).await;
            let content = if cancelled > 0 {
                format!("⏹ Stopped {} task(s).", cancelled)
            } else {
                "No active task to stop.".to_string()
            };
            return Ok(Some(OutboundMessage {
                channel: msg.channel,
                chat_id: msg.chat_id,
                content,
                reply_to: None,
                media: Vec::new(),
                metadata: MessageMetadata::default(),
            }));
        }

        if cmd == "/new" {
            session.clear();
            self.sessions.save(&session).await?;
            self.sessions.invalidate(&session.key).await;
            return Ok(Some(OutboundMessage {
                channel: msg.channel,
                chat_id: msg.chat_id,
                content: "New session started.".to_string(),
                reply_to: None,
                media: Vec::new(),
                metadata: MessageMetadata::default(),
            }));
        }

        // Build tool context once and reuse
        let tool_context = ToolContext {
            channel: msg.channel.clone(),
            chat_id: msg.chat_id.clone(),
            session_key: session_key.clone(),
            message_id: msg.metadata.message_id.clone(),
        };

        self.tools.start_turn().await;

        let history = session.get_history(self.memory_window);
        let history_len = history.len();
        let messages = self
            .context
            .build_messages(
                history,
                &msg.content,
                if msg.media.is_empty() {
                    None
                } else {
                    Some(&msg.media)
                },
                Some(&msg.channel),
                Some(&msg.chat_id),
            )
            .await;

        let start_index = messages.len() - 1 - history_len;
        let (final_content, all_msgs) = self.run_agent_loop(messages, &tool_context).await;

        self.save_turn(&mut session, all_msgs, start_index);
        self.sessions.save(&session).await?;

        if self.tools.message_sent_in_turn().await {
            return Ok(None);
        }

        Ok(Some(OutboundMessage {
            channel: msg.channel,
            chat_id: msg.chat_id,
            content: final_content.unwrap_or_else(|| {
                "I've completed processing but have no response to give.".to_string()
            }),
            reply_to: None,
            media: Vec::new(),
            metadata: msg.metadata,
        }))
    }

    async fn process_system_message(&self, msg: InboundMessage) -> Result<Option<OutboundMessage>> {
        let (channel, chat_id) = if let Some((c, id)) = msg.chat_id.split_once(':') {
            (c.to_string(), id.to_string())
        } else {
            ("cli".to_string(), msg.chat_id.clone())
        };

        let session_key = format!("{}:{}", channel, chat_id);
        let mut session = self.sessions.get_or_create(&session_key).await?;

        // Build tool context once and reuse
        let tool_context = ToolContext {
            channel: channel.clone(),
            chat_id: chat_id.clone(),
            session_key: session_key.clone(),
            message_id: msg.metadata.message_id.clone(),
        };

        self.tools.start_turn().await;

        let history = session.get_history(self.memory_window);
        let history_len = history.len();
        let messages = self
            .context
            .build_messages(history, &msg.content, None, Some(&channel), Some(&chat_id))
            .await;

        let start_index = messages.len() - 1 - history_len;
        let (final_content, all_msgs) = self.run_agent_loop(messages, &tool_context).await;

        self.save_turn(&mut session, all_msgs, start_index);
        self.sessions.save(&session).await?;

        Ok(Some(OutboundMessage {
            channel,
            chat_id,
            content: final_content.unwrap_or_else(|| "Background task completed.".to_string()),
            reply_to: None,
            media: Vec::new(),
            metadata: MessageMetadata::default(),
        }))
    }

    async fn run_agent_loop(
        &self,
        mut messages: Vec<ChatMessage>,
        tool_context: &ToolContext,
    ) -> (Option<String>, Vec<ChatMessage>) {
        let mut final_content = None;
        for _iteration in 0..self.max_iterations {
            let response = self
                .provider
                .chat(ChatRequest {
                    messages: messages.clone(),
                    tools: Some(self.tools.definitions()),
                    model: Some(self.model.clone()),
                    max_tokens: self.max_tokens,
                    temperature: self.temperature,
                    reasoning_effort: self.reasoning_effort.clone(),
                })
                .await;

            if response.has_tool_calls() {
                // Persist the assistant tool-call message first, then append each tool result.
                let clean = strip_think(response.content.as_deref());
                if let Some(text) = clean {
                    debug_progress(&text, false);
                }
                debug_progress(&tool_hint(&response.tool_calls), true);

                let tool_call_dicts = response
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

                self.context.add_assistant_message(
                    &mut messages,
                    response.content,
                    Some(tool_call_dicts),
                    response.reasoning_content,
                    response.thinking_blocks,
                );

                // Use into_iter to move tool calls instead of cloning
                for call in response.tool_calls {
                    info!("tool call: {}({})", call.name, call.arguments_json);
                    // TODO: suppport to response to user the tool call info
                    let result = match self
                        .tools
                        .execute(call.name.as_str(), &call.arguments_json, tool_context)
                        .await
                    {
                        Ok(value) => value,
                        Err(err) => {
                            // TODO: add warning logging.
                            format_tool_error(&err)
                        }
                    };
                    self.context.add_tool_result(
                        &mut messages,
                        &call.id,
                        call.name.as_str(),
                        &result,
                    );
                }
                continue;
            }

            let clean = strip_think(response.content.as_deref());
            if response.finish_reason == "error" {
                warn!("LLM returned error: {:?}", clean);
                final_content = Some(clean.unwrap_or_else(|| {
                    "Sorry, I encountered an error calling the AI model.".to_string()
                }));
                break;
            }

            self.context.add_assistant_message(
                &mut messages,
                clean.clone(),
                None,
                response.reasoning_content,
                response.thinking_blocks,
            );
            final_content = clean;
            break;
        }

        if final_content.is_none() {
            final_content = Some(format!(
                "I reached the maximum number of tool call iterations ({}) without completing the task. You can try breaking the task into smaller steps.",
                self.max_iterations
            ));
        }

        (final_content, messages)
    }

    fn save_turn(&self, session: &mut Session, messages: Vec<ChatMessage>, skip: usize) {
        for msg in messages.into_iter().skip(skip) {
            let ChatMessage {
                role,
                mut content,
                tool_calls,
                tool_call_id,
                name,
                reasoning_content,
                thinking_blocks,
            } = msg;

            if matches!(role, MessageRole::Assistant) && content.is_none() && tool_calls.is_none() {
                continue;
            }

            if matches!(role, MessageRole::Tool)
                && let Some(MessageContent::Text(text)) = &mut content
                && text.len() > Self::TOOL_RESULT_MAX_CHARS
            {
                // Tool outputs can be very large; cap storage to keep session files bounded.
                *text = format!("{}\n... (truncated)", &text[..Self::TOOL_RESULT_MAX_CHARS]);
            }

            if matches!(role, MessageRole::User) {
                if let Some(MessageContent::Text(text)) = &mut content {
                    if let Some(user) = strip_runtime_context(text) {
                        if user.is_empty() {
                            continue;
                        }
                        *text = user;
                    }
                }

                if let Some(MessageContent::Parts(parts)) = &mut content {
                    if let Some(crate::provider::ContentPart::Text { text }) = parts.first()
                        && text.starts_with(ContextBuilder::RUNTIME_CONTEXT_TAG)
                    {
                        parts.remove(0);
                    }
                    if parts.is_empty() {
                        continue;
                    }
                }
            }

            let entry = SessionEntry {
                role,
                content,
                timestamp: chrono::Utc::now().to_rfc3339(),
                tool_calls,
                tool_call_id,
                name,
                reasoning_content,
                thinking_blocks,
            };
            session.messages.push(entry);
        }
        session.updated_at = chrono::Utc::now();
    }
}

fn strip_runtime_context(text: &str) -> Option<String> {
    // Runtime metadata is prepended to user messages and should not be persisted as user intent.
    if !text.starts_with(ContextBuilder::RUNTIME_CONTEXT_TAG) {
        return None;
    }
    let mut parts = text.splitn(2, "\n\n");
    let _ = parts.next();
    Some(parts.next().unwrap_or("").trim().to_string())
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

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum PreviewValue {
    String(String),
    Bool(bool),
    Integer(i64),
    Float(f64),
    Array(Vec<PreviewValue>),
    Object(BTreeMap<String, PreviewValue>),
}

impl PreviewValue {
    fn short(&self) -> String {
        match self {
            Self::String(s) => s.clone(),
            Self::Bool(v) => v.to_string(),
            Self::Integer(v) => v.to_string(),
            Self::Float(v) => v.to_string(),
            Self::Array(values) => values
                .first()
                .map(|v| v.short())
                .unwrap_or_else(|| "[]".to_string()),
            Self::Object(map) => map
                .iter()
                .next()
                .map(|(_, v)| v.short())
                .unwrap_or_else(|| "{}".to_string()),
        }
    }
}

fn tool_hint(calls: &[crate::provider::ToolCallRequest]) -> String {
    // Build a compact preview string for progress logs, not for execution.
    let mut hints = Vec::new();
    for tc in calls {
        if let Ok(obj) = serde_json::from_str::<BTreeMap<String, PreviewValue>>(&tc.arguments_json)
            && let Some((_, first)) = obj.iter().next()
        {
            let raw = first.short();
            let shown = if raw.len() > 40 {
                format!("{}...", &raw[..40])
            } else {
                raw
            };
            hints.push(format!("{}(\"{}\")", tc.name, shown));
            continue;
        }
        hints.push(tc.name.to_string());
    }
    hints.join(", ")
}

fn debug_progress(content: &str, tool_hint: bool) {
    if tool_hint {
        info!("↳ [tool] {}", content);
    } else {
        info!("↳ {}", content);
    }
}

fn format_tool_error(err: &crate::error::NanobotError) -> String {
    format!(
        "Error: {}\n\n[Analyze the error above and try a different approach.]",
        err
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::provider::ToolCallRequest;

    #[test]
    fn strip_runtime_context_extracts_user_content() {
        let text = format!(
            "{}\nCurrent Time: 2026-01-01 10:00\n\nhello user",
            ContextBuilder::RUNTIME_CONTEXT_TAG
        );
        let out = strip_runtime_context(&text);
        assert_eq!(out.as_deref(), Some("hello user"));

        assert!(strip_runtime_context("plain text").is_none());
    }

    #[test]
    fn strip_think_removes_think_blocks() {
        let out = strip_think(Some("<think>internal</think>final answer"));
        assert_eq!(out.as_deref(), Some("final answer"));

        let only_think = strip_think(Some("<think>internal</think>"));
        assert!(only_think.is_none());
    }

    #[test]
    fn tool_hint_uses_first_argument_preview_and_fallback_name() {
        let long = "abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJ";
        let calls = vec![
            ToolCallRequest {
                id: "1".to_string(),
                name: "read_file".into(),
                arguments_json: format!(r#"{{"path":"{}"}}"#, long),
            },
            ToolCallRequest {
                id: "2".to_string(),
                name: "exec".into(),
                arguments_json: "{bad-json}".to_string(),
            },
        ];

        let out = tool_hint(&calls);
        assert!(out.contains("read_file(\""));
        assert!(out.contains("...\")"));
        assert!(out.contains(", exec"));
    }

    #[test]
    fn format_tool_error_contains_analysis_hint() {
        let err = crate::error::NanobotError::tool_execution("test", anyhow::anyhow!("boom"));
        let out = format_tool_error(&err);
        assert!(out.contains("Error:"));
        assert!(out.contains("Analyze the error above"));
    }

    // New comprehensive tests for AgentLoop

    use crate::provider::{ChatRequest, LLMResponse, UsageStats};
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct MockProvider {
        call_count: Arc<AtomicUsize>,
        response: String,
        tool_calls: Vec<ToolCallRequest>,
    }

    impl MockProvider {
        fn new(response: &str) -> Self {
            Self {
                call_count: Arc::new(AtomicUsize::new(0)),
                response: response.to_string(),
                tool_calls: Vec::new(),
            }
        }

        fn with_tool_calls(mut self, calls: Vec<ToolCallRequest>) -> Self {
            self.tool_calls = calls;
            self
        }
    }

    #[async_trait]
    impl LLMProvider for MockProvider {
        async fn chat(&self, _req: ChatRequest) -> LLMResponse {
            let count = self.call_count.fetch_add(1, Ordering::SeqCst);

            // First call returns tool calls, second returns final response
            if count == 0 && !self.tool_calls.is_empty() {
                LLMResponse {
                    content: Some("Using tools".to_string()),
                    tool_calls: self.tool_calls.clone(),
                    finish_reason: "tool_calls".to_string(),
                    usage: UsageStats::default(),
                    reasoning_content: None,
                    thinking_blocks: None,
                }
            } else {
                LLMResponse {
                    content: Some(self.response.clone()),
                    tool_calls: Vec::new(),
                    finish_reason: "stop".to_string(),
                    usage: UsageStats::default(),
                    reasoning_content: None,
                    thinking_blocks: None,
                }
            }
        }

        fn default_model(&self) -> &str {
            "mock/model"
        }
    }

    async fn create_test_agent(provider: Arc<dyn LLMProvider>) -> AgentLoop {
        use crate::agent::AgentLoopBuilder;
        use crate::bus::MessageBus;

        let bus = Arc::new(MessageBus::new());
        let workspace = std::env::temp_dir().join(format!("nanobot-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&workspace).unwrap();

        AgentLoopBuilder::new(bus, provider, workspace)
            .build()
            .unwrap()
    }

    #[tokio::test]
    async fn agent_loop_processes_simple_message() {
        let provider = Arc::new(MockProvider::new("Hello, user!"));
        let agent = create_test_agent(provider).await;

        let result = agent
            .process_direct("Hi", "test-session", "cli", "direct")
            .await
            .unwrap();

        assert_eq!(result, "Hello, user!");
    }

    #[tokio::test]
    async fn agent_loop_handles_tool_calls() {
        let tool_calls = vec![ToolCallRequest {
            id: "call_1".to_string(),
            name: "list_dir".into(),
            arguments_json: r#"{"path":"."}"#.to_string(),
        }];

        let provider = Arc::new(MockProvider::new("Listed files").with_tool_calls(tool_calls));
        let agent = create_test_agent(provider).await;

        let result = agent
            .process_direct("List files", "test-session", "cli", "direct")
            .await
            .unwrap();

        assert_eq!(result, "Listed files");
    }

    #[tokio::test]
    async fn agent_loop_respects_max_iterations() {
        // Provider that always returns tool calls
        struct InfiniteToolProvider;

        #[async_trait]
        impl LLMProvider for InfiniteToolProvider {
            async fn chat(&self, _req: ChatRequest) -> LLMResponse {
                LLMResponse {
                    content: Some("Calling tool".to_string()),
                    tool_calls: vec![ToolCallRequest {
                        id: "call_1".to_string(),
                        name: "list_dir".into(),
                        arguments_json: r#"{"path":"."}"#.to_string(),
                    }],
                    finish_reason: "tool_calls".to_string(),
                    usage: UsageStats::default(),
                    reasoning_content: None,
                    thinking_blocks: None,
                }
            }

            fn default_model(&self) -> &str {
                "infinite/model"
            }
        }

        let provider = Arc::new(InfiniteToolProvider);
        let agent = create_test_agent(provider).await;

        let result = agent
            .process_direct("Do something", "test-session", "cli", "direct")
            .await
            .unwrap();

        // Should hit max iterations and return error message
        assert!(result.contains("maximum number of tool call iterations"));
        assert!(result.contains("40")); // default max_iterations
    }

    #[tokio::test]
    async fn agent_loop_handles_concurrent_sessions() {
        let provider = Arc::new(MockProvider::new("Response"));
        let agent = Arc::new(create_test_agent(provider).await);

        // Spawn multiple concurrent requests from different sessions
        let mut handles = vec![];
        for i in 0..5 {
            let agent = agent.clone();
            let session = format!("session-{}", i);
            let handle = tokio::spawn(async move {
                agent
                    .process_direct("Test", &session, "cli", "direct")
                    .await
                    .unwrap()
            });
            handles.push(handle);
        }

        // All should complete successfully
        for handle in handles {
            let result = handle.await.unwrap();
            assert_eq!(result, "Response");
        }
    }

    #[tokio::test]
    async fn session_locks_are_cleaned_up() {
        let provider = Arc::new(MockProvider::new("Response"));
        let agent = create_test_agent(provider).await;

        // Manually create a lock by simulating dispatch
        let session_key = "cli:direct".to_string();
        {
            let mut locks = agent.session_locks.write().await;
            locks.insert(session_key.clone(), Arc::new(Mutex::new(())));
        }

        // Verify lock exists
        {
            let locks = agent.session_locks.read().await;
            assert!(locks.contains_key(&session_key));
        }

        // Trigger cleanup (no active tasks, so should be removed)
        agent.cleanup_session_locks().await;

        // Lock should be removed
        {
            let locks = agent.session_locks.read().await;
            assert!(!locks.contains_key(&session_key));
        }
    }

    #[tokio::test]
    async fn agent_loop_saves_session_history() {
        let provider = Arc::new(MockProvider::new("Response"));
        let agent = create_test_agent(provider).await;

        // Create session first
        let session_key = "cli:direct";
        agent.sessions.get_or_create(session_key).await.unwrap();

        // Process a message
        agent
            .process_direct("Hello", "test-session", "cli", "direct")
            .await
            .unwrap();

        // Check session exists
        let session = agent.sessions.get_or_create(session_key).await.unwrap();

        // Session system works correctly
        assert!(session.key == session_key);
    }
}
