//! Simplified AgentLoop using modular ReAct engine

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use dashmap::DashMap;
use parking_lot::Mutex;
use tokio::task::AbortHandle;
use tracing::{Instrument, debug, debug_span, error, info, trace};

use crate::error::{AgentError, AgentResult};
use crate::react::LoopExitReason;
use crate::react::{ExecutionContext, LoopOutcome, ModelConfig, ProgressEmitter, ReActExecutor};
use crate::traits::{Agent, ContextProvider};
use crate::utils::preview_text;
use nanobot_bus::{
    InboundCommand, InboundMessage, MessageBus, MessageId, MessageMetadata, OutboundMessage,
};
use nanobot_config::schema::{AgentRuntimeOverrides, ChannelsConfig};
use nanobot_provider::LLMProvider;
use nanobot_session::{
    ConsolidationConfig, ConsolidationOutcome, Session, SessionEntry, SessionManager,
};
use nanobot_tools::mcp::MCPManager;
use nanobot_tools::{ToolContext, ToolRegistry};
use nanobot_types::SessionKey;
use nanobot_types::provider::{ChatMessage, MessageContent, MessageRole, UsageStats};
use nanobot_types::task::TaskId;

const TARGET: &str = "nanobot::agent";
const INTERNAL_ERROR_PREFIX: &str = "⚠️ ";
const SYSTEM_INFO_PREFIX: &str = "ℹ️ ";
const SYSTEM_SUCCESS_PREFIX: &str = "✅ ";
const SAVE_WITH_CONSOLIDATION_TIMEOUT: Duration = Duration::from_secs(90);

pub struct AgentLoop {
    pub(crate) bus: MessageBus,
    pub(crate) provider: Arc<dyn LLMProvider>,
    pub(crate) model: String,
    pub(crate) max_iterations: usize,
    pub(crate) temperature: f32,
    pub(crate) max_tokens: i32,
    pub(crate) memory_window: usize,
    pub(crate) reasoning_effort: Option<String>,
    pub(crate) consolidation_config: ConsolidationConfig,
    pub(crate) consolidation_enabled: bool,
    pub(crate) channel_configs: ChannelsConfig,
    pub(crate) send_usage_summary: bool,
    pub(crate) tools: Arc<ToolRegistry>,
    pub(crate) mcp: Option<Arc<MCPManager>>,
    pub(crate) context: Arc<dyn ContextProvider>,
    pub sessions: Arc<SessionManager>,
    pub(crate) running: Arc<AtomicBool>,
    pub(crate) session_locks: Arc<DashMap<SessionKey, Arc<tokio::sync::Mutex<()>>>>,
    pub(crate) active_tasks: Arc<DashMap<SessionKey, DashMap<TaskId, AbortHandle>>>,
    pub(crate) last_cleanup: Arc<Mutex<Instant>>,
}

struct OutboundEnvelope {
    message: OutboundMessage,
    usage: Option<UsageStats>,
}

#[derive(Debug, Clone)]
struct AgentRuntimeSettings {
    memory_window: usize,
    consolidation_enabled: bool,
    consolidation_config: ConsolidationConfig,
}

impl AgentLoop {
    const CLEANUP_INTERVAL: Duration = Duration::from_secs(300);

    pub(crate) fn format_internal_error(message: impl AsRef<str>) -> String {
        let message = message.as_ref().trim();
        if message.starts_with(INTERNAL_ERROR_PREFIX) {
            message.to_string()
        } else {
            format!("{INTERNAL_ERROR_PREFIX}{message}")
        }
    }

    pub(crate) fn format_system_info(message: impl AsRef<str>) -> String {
        let message = message.as_ref().trim();
        if message.starts_with(SYSTEM_INFO_PREFIX) {
            message.to_string()
        } else {
            format!("{SYSTEM_INFO_PREFIX}{message}")
        }
    }

    pub(crate) fn format_system_success(message: impl AsRef<str>) -> String {
        let message = message.as_ref().trim();
        if message.starts_with(SYSTEM_SUCCESS_PREFIX) {
            message.to_string()
        } else {
            format!("{SYSTEM_SUCCESS_PREFIX}{message}")
        }
    }

    fn usage_summary_text(usage: &UsageStats) -> String {
        let prompt_tokens = usage.prompt_tokens.unwrap_or(0);
        let completion_tokens = usage.completion_tokens.unwrap_or(0);
        let total_tokens = usage
            .total_tokens
            .or_else(|| {
                usage
                    .prompt_tokens
                    .zip(usage.completion_tokens)
                    .map(|(prompt, completion)| prompt + completion)
            })
            .unwrap_or(0);
        format!(
            "Tokens: {} in / {} out / {} total",
            prompt_tokens, completion_tokens, total_tokens
        )
    }

    fn runtime_settings_for_channel(&self, channel: &str) -> AgentRuntimeSettings {
        let overrides = match channel {
            "telegram" => self.channel_configs.telegram.agent_overrides.as_ref(),
            "discord" => self.channel_configs.discord.agent_overrides.as_ref(),
            "feishu" | "lark" => self.channel_configs.feishu.agent_overrides.as_ref(),
            _ => None,
        };
        Self::merge_runtime_settings(
            self.memory_window,
            self.consolidation_enabled,
            &self.consolidation_config,
            overrides,
        )
    }

    fn merge_runtime_settings(
        default_memory_window: usize,
        default_consolidation_enabled: bool,
        default_consolidation_config: &ConsolidationConfig,
        overrides: Option<&AgentRuntimeOverrides>,
    ) -> AgentRuntimeSettings {
        let mut settings = AgentRuntimeSettings {
            memory_window: default_memory_window,
            consolidation_enabled: default_consolidation_enabled,
            consolidation_config: default_consolidation_config.clone(),
        };

        if let Some(overrides) = overrides {
            if let Some(memory_window) = overrides.memory_window {
                settings.memory_window = memory_window;
            }
            if let Some(enabled) = overrides.consolidation_enabled {
                settings.consolidation_enabled = enabled;
            }
            if let Some(keep_recent) = overrides.consolidation_keep_recent {
                settings.consolidation_config.keep_recent = keep_recent;
            }
            if let Some(min_messages) = overrides.consolidation_min_messages {
                settings.consolidation_config.min_messages = min_messages;
            }
            if let Some(max_tokens) = overrides.consolidation_summary_max_tokens {
                settings.consolidation_config.max_tokens = max_tokens;
            }
        }

        settings
    }

    async fn ensure_mcp_connected(&self) {
        if let Some(mcp) = &self.mcp
            && let Err(err) = mcp.connect_if_needed(&self.tools).await
        {
            error!(
                target: TARGET,
                "failed to connect MCP servers (will retry on next message): {}",
                err
            );
        }
    }

    pub async fn close_mcp(&self) {
        if let Some(mcp) = &self.mcp {
            debug!(target: TARGET, "closing MCP manager");
            mcp.close(&self.tools).await;
            debug!(target: TARGET, "MCP manager closed");
        }
    }

    pub async fn close_provider(&self) {
        debug!(target: TARGET, "closing provider");
        self.provider.close().await;
        debug!(target: TARGET, "provider closed");
    }

    pub async fn run(&self) {
        self.running.store(true, Ordering::Release);
        self.ensure_mcp_connected().await;
        info!(target: TARGET, "agent loop started");
        let mut inbound_rx = self.bus.subscribe_inbound();

        loop {
            if !self.running.load(Ordering::Acquire) {
                break;
            }
            let Ok(msg) = inbound_rx.recv().await else {
                continue;
            };
            if !self.running.load(Ordering::Acquire) {
                break;
            }

            let command = msg.command();
            debug!(
                target: TARGET,
                session_key = %msg.session_key(),
                channel = %msg.channel,
                chat_id = %msg.chat_id,
                command = ?command.map(|cmd| cmd.as_str()),
                content_len = msg.content_text().len(),
                media_count = msg.media.len(),
                "inbound message received"
            );

            if command == Some(InboundCommand::Stop) {
                self.handle_stop(msg).await;
                continue;
            }

            let task_id = TaskId::new();
            let session_key = msg.session_key();
            let this = self.clone();
            let span = debug_span!(
                target: TARGET,
                "dispatch_task",
                task_id = %task_id,
                session_key = %session_key,
                channel = %msg.channel,
                chat_id = %msg.chat_id
            );

            let handle = tokio::spawn({
                let session_key = session_key.clone();
                async move {
                    this.dispatch(msg).await;
                    this.unregister_task(&session_key, &task_id).await;
                }
                .instrument(span)
            });

            self.register_task(&session_key, task_id, handle.abort_handle())
                .await;
        }

        info!(target: TARGET, "agent loop stopped");
    }

    pub async fn stop(&self) {
        self.running.store(false, Ordering::Release);
        info!(target: TARGET, "stopping agent loop");
        let _ = self.bus.publish_inbound(InboundMessage {
            channel: "system".to_string(),
            sender_id: "system".to_string(),
            chat_id: "cli:direct".to_string(),
            content: "__nanobot_stop__".into(),
            timestamp: chrono::Utc::now(),
            media: Vec::new(),
            metadata: MessageMetadata::default(),
            session_key_override: Some(SessionKey::from("__nanobot_stop__")),
        });

        let mut aborted = 0usize;
        for entry in self.active_tasks.iter() {
            let handles = entry.value();
            for handle_entry in handles.iter() {
                handle_entry.value().abort();
                aborted += 1;
            }
        }
        self.active_tasks.clear();
        debug!(target: TARGET, aborted, "cleared active task registry during shutdown");
    }

    pub async fn process_direct(
        &self,
        content: &str,
        session_key: &SessionKey,
        channel: &str,
        chat_id: &str,
    ) -> AgentResult<String> {
        debug!(
            target: TARGET,
            session_key = %session_key,
            channel,
            chat_id,
            content_preview = %preview_text(content, 120),
            "processing direct request"
        );
        self.ensure_mcp_connected().await;
        let msg = InboundMessage {
            channel: channel.to_string(),
            sender_id: "user".to_string(),
            chat_id: chat_id.to_string(),
            content: content.into(),
            timestamp: chrono::Utc::now(),
            media: Vec::new(),
            metadata: MessageMetadata::default(),
            session_key_override: Some(session_key.clone()),
        };

        let out = self.process_message(msg).await?;
        let mut content = out
            .as_ref()
            .map(|m| m.message.content.clone())
            .unwrap_or_default();
        if self.send_usage_summary
            && let Some(usage_text) = out
                .as_ref()
                .and_then(|o| o.usage.as_ref())
                .map(|u| format!("\n\n---\n_{}_", Self::usage_summary_text(u)))
        {
            content.push_str(&usage_text);
        }
        debug!(
            target: TARGET,
            session_key = %session_key,
            channel,
            chat_id,
            content_len = content.len(),
            content_preview = %preview_text(&content, 120),
            "direct request completed"
        );
        Ok(content)
    }

    async fn register_task(&self, session_key: &SessionKey, task_id: TaskId, handle: AbortHandle) {
        self.active_tasks
            .entry(session_key.clone())
            .or_default()
            .insert(task_id, handle);
    }

    async fn unregister_task(&self, session_key: &SessionKey, task_id: &TaskId) {
        if let Some(tasks) = self.active_tasks.get(session_key) {
            tasks.remove(task_id);
            if tasks.is_empty() {
                drop(tasks);
                self.active_tasks.remove(session_key);
            }
        }
    }

    async fn handle_stop(&self, msg: InboundMessage) {
        let session_key = msg.session_key();

        let cancelled_main = if let Some((_, handles)) = self.active_tasks.remove(&session_key) {
            let mut count = 0usize;
            for entry in handles.iter() {
                let handle = entry.value();
                if !handle.is_finished() {
                    handle.abort();
                    count += 1;
                }
            }
            count
        } else {
            0usize
        };

        let cancelled_sub = self.tools.cancel_spawn_by_session(&session_key).await;
        let total = cancelled_main + cancelled_sub;
        let content = if total > 0 {
            Self::format_system_success(format!("Stopped {} task(s).", total))
        } else {
            Self::format_system_info("No active task to stop.")
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
        let session_key = msg.session_key();
        // 同一 session 串行执行，避免并发回合同时改写会话状态导致上下文错乱。
        let lock = self
            .session_locks
            .entry(session_key.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();

        let lock_wait_start = Instant::now();
        let _guard = lock.lock().await;
        let lock_wait = lock_wait_start.elapsed();
        if lock_wait > Duration::from_millis(100) {
            debug!(
                target: TARGET,
                session_key = %session_key,
                lock_wait_ms = lock_wait.as_millis(),
                "acquired session lock after wait"
            );
        }

        match self.process_message(msg.clone()).await {
            Ok(Some(out)) => {
                if let Err(err) = self.bus.publish_outbound(out.message.clone()) {
                    error!(target: TARGET, error = %err, "failed to publish outbound message");
                }
                if self.send_usage_summary
                    && let Some(usage) = out.usage
                {
                    let usage_msg = OutboundMessage {
                        channel: out.message.channel.clone(),
                        chat_id: out.message.chat_id.clone(),
                        content: Self::format_system_info(format!(
                            "**{}**",
                            Self::usage_summary_text(&usage)
                        )),
                        reply_to: None,
                        media: Vec::new(),
                        metadata: MessageMetadata::default(),
                    };
                    if let Err(err) = self.bus.publish_outbound(usage_msg) {
                        error!(target: TARGET, session_key = %session_key, error = %err, "failed to publish usage summary");
                    }
                }
            }
            Ok(None) => {
                trace!(target: TARGET, session_key = %session_key, "no outbound message to publish");
            }
            Err(err) => {
                error!(target: TARGET, session_key = %session_key, error = %err, "error processing message");
                let _ = self.bus.publish_outbound(OutboundMessage {
                    channel: msg.channel,
                    chat_id: msg.chat_id,
                    content: Self::format_internal_error(format!("Error: {}", err)),
                    reply_to: None,
                    media: Vec::new(),
                    metadata: msg.metadata,
                });
            }
        }

        self.maybe_cleanup().await;
    }

    async fn maybe_cleanup(&self) {
        let now = Instant::now();
        let mut last = self.last_cleanup.lock();
        if now.duration_since(*last) < Self::CLEANUP_INTERVAL {
            return;
        }
        *last = now;
        drop(last);

        let before = self.session_locks.len();
        self.session_locks
            .retain(|_, lock| Arc::strong_count(lock) > 1);
        let after = self.session_locks.len();
        if before != after {
            debug!(
                target: TARGET,
                removed = before - after,
                remaining = after,
                "cleaned up unused session locks"
            );
        }
    }

    pub fn has_active_tasks(&self, session_key: &SessionKey) -> bool {
        self.active_tasks
            .get(session_key)
            .map(|tasks| !tasks.is_empty())
            .unwrap_or(false)
    }

    async fn process_message(&self, msg: InboundMessage) -> AgentResult<Option<OutboundEnvelope>> {
        trace!(
            target: TARGET,
            session_key = %msg.session_key(),
            content_preview = %preview_text(msg.content_text(), 120),
            media_count = msg.media.len(),
            message_id = ?msg.metadata.message_id,
            "process_message start"
        );
        if msg.channel == "system" {
            return self.process_system_message(msg).await.map(|msg| {
                msg.map(|message| OutboundEnvelope {
                    message,
                    usage: None,
                })
            });
        }

        if let Some(command) = msg.command() {
            return self.process_builtin_command(msg, command).await.map(|msg| {
                msg.map(|message| OutboundEnvelope {
                    message,
                    usage: None,
                })
            });
        }

        let session_key = msg.session_key();
        let mut session = self.sessions.get_or_create(session_key.as_str()).await?;
        let runtime_settings = self.runtime_settings_for_channel(&msg.channel);

        let tool_context = ToolContext {
            channel: msg.channel.clone(),
            chat_id: msg.chat_id.clone(),
            session_key: session_key.clone(),
            message_id: msg.metadata.message_id.clone(),
        };

        self.tools.start_turn().await?;

        let history = self
            .sessions
            .get_history(&session, runtime_settings.memory_window)
            .await?;
        let history_len = history.len();
        let messages = self
            .context
            .build_messages(
                &self.sessions,
                session_key.as_str(),
                history,
                msg.content_text(),
                if msg.media.is_empty() {
                    None
                } else {
                    Some(&msg.media)
                },
                Some(&msg.channel),
                Some(&msg.chat_id),
            )
            .await;

        // 新一轮输入插入在“历史尾部”，保存时从这里开始截取新增消息，避免重复落盘旧历史。
        let start_index = messages.len() - 1 - history_len;

        let reply_to = msg
            .metadata
            .message_id
            .as_ref()
            .and_then(MessageId::external_id)
            .map(str::to_string);
        let stream_id = msg
            .metadata
            .message_id
            .as_ref()
            .and_then(MessageId::external_id)
            .map(str::to_string)
            .unwrap_or_else(|| format!("stream:{}", TaskId::new().as_str()));
        let progress = ProgressEmitter::new(
            self.bus.clone(),
            msg.channel.clone(),
            msg.chat_id.clone(),
            reply_to.clone(),
            stream_id.clone(),
        );
        let outcome = self
            .run_agent_loop(messages, &tool_context, &session_key, Some(progress))
            .await?;

        if outcome.exit_reason == LoopExitReason::ProviderError {
            let detail = outcome
                .error_detail
                .as_deref()
                .unwrap_or("provider error detail unavailable");
            return Err(AgentError::loop_error(format!(
                "provider request failed; check provider config/network and retry (session_key={}, channel={}, chat_id={}, detail={})",
                session_key, msg.channel, msg.chat_id, detail
            )));
        }

        self.save_turn(&mut session, outcome.messages, start_index);
        tokio::time::timeout(
            SAVE_WITH_CONSOLIDATION_TIMEOUT,
            self.sessions.save_with_consolidation(
                &mut session,
                &self.provider,
                &self.model,
                Some(&runtime_settings.consolidation_config),
                runtime_settings.consolidation_enabled,
            ),
        )
        .await
        .map_err(|_| {
            AgentError::loop_error(format!(
                "session save/consolidation timeout (session_key={}, channel={}, chat_id={}, timeout={}s)",
                session_key,
                msg.channel,
                msg.chat_id,
                SAVE_WITH_CONSOLIDATION_TIMEOUT.as_secs()
            ))
        })??;

        // 如果本轮已经显式调用 message 工具发消息，则跳过默认最终回复，避免重复发送。
        if self.tools.message_sent_in_turn().await {
            return Ok(None);
        }

        Ok(Some(OutboundEnvelope {
            usage: outcome.loop_usage.clone().or(outcome.usage.clone()),
            message: OutboundMessage {
                channel: msg.channel,
                chat_id: msg.chat_id,
                content: outcome.final_content.unwrap_or_else(|| {
                    "I've completed processing but have no response to give.".to_string()
                }),
                reply_to,
                media: Vec::new(),
                metadata: MessageMetadata {
                    message_id: msg.metadata.message_id,
                    stream_id: Some(stream_id),
                },
            },
        }))
    }

    async fn process_builtin_command(
        &self,
        msg: InboundMessage,
        command: InboundCommand,
    ) -> AgentResult<Option<OutboundMessage>> {
        match command {
            InboundCommand::Help => Ok(Some(OutboundMessage {
                channel: msg.channel,
                chat_id: msg.chat_id,
                content: Self::format_system_info(
                    "nanobot commands:\n/new - Start a new conversation\n/stop - Stop the current task\n/compact - Consolidate session history\n/help - Show available commands",
                ),
                reply_to: None,
                media: Vec::new(),
                metadata: msg.metadata,
            })),
            InboundCommand::New => {
                let session_key = msg.session_key();
                self.sessions.delete(session_key.as_str()).await?;
                Ok(Some(OutboundMessage {
                    channel: msg.channel,
                    chat_id: msg.chat_id,
                    content: Self::format_system_success("Starting a new conversation."),
                    reply_to: None,
                    media: Vec::new(),
                    metadata: msg.metadata,
                }))
            }
            InboundCommand::Compact => {
                let session_key = msg.session_key();
                let mut session = self.sessions.get_or_create(session_key.as_str()).await?;
                let runtime_settings = self.runtime_settings_for_channel(&msg.channel);
                let content = if !runtime_settings.consolidation_enabled {
                    Self::format_system_info("Consolidation is disabled for this channel.")
                } else {
                    match self
                        .sessions
                        .consolidate_now_with_config(
                            &mut session,
                            &self.provider,
                            &self.model,
                            &runtime_settings.consolidation_config,
                        )
                        .await
                    {
                        Err(e) => {
                            Self::format_internal_error(format!("Failed to consolidate: {}", e))
                        }
                        Ok(ConsolidationOutcome::Disabled) => {
                            Self::format_system_info("Consolidation is not configured.")
                        }
                        Ok(ConsolidationOutcome::Skipped) => {
                            Self::format_system_info("Not enough messages to consolidate yet.")
                        }
                        Ok(ConsolidationOutcome::Consolidated { removed }) => {
                            Self::format_system_success(format!(
                                "Consolidated {} messages.",
                                removed
                            ))
                        }
                    }
                };
                Ok(Some(OutboundMessage {
                    channel: msg.channel,
                    chat_id: msg.chat_id,
                    content,
                    reply_to: None,
                    media: Vec::new(),
                    metadata: msg.metadata,
                }))
            }
            InboundCommand::Stop => {
                unreachable!("stop command should be handled before dispatch")
            }
        }
    }

    async fn process_system_message(
        &self,
        _msg: InboundMessage,
    ) -> AgentResult<Option<OutboundMessage>> {
        Ok(None)
    }

    async fn run_agent_loop(
        &self,
        messages: Vec<ChatMessage>,
        tool_context: &ToolContext,
        session_key: &SessionKey,
        progress: Option<ProgressEmitter>,
    ) -> AgentResult<LoopOutcome> {
        let executor = ReActExecutor::new(
            self.provider.clone(),
            self.tools.clone(),
            self.max_iterations,
        );

        let config = ModelConfig {
            model: self.model.clone(),
            temperature: self.temperature,
            max_tokens: self.max_tokens,
            reasoning_effort: self.reasoning_effort.clone(),
            iteration: 0,
        };

        let cancelled = Arc::new(AtomicBool::new(false));
        let exec_context = ExecutionContext {
            session_key: session_key.clone(),
            channel: tool_context.channel.clone(),
            chat_id: tool_context.chat_id.clone(),
            cancelled,
        };

        executor
            .run(
                messages,
                self.tools.definitions(),
                config,
                exec_context,
                progress,
            )
            .await
    }

    fn save_turn(&self, session: &mut Session, all_msgs: Vec<ChatMessage>, start_index: usize) {
        let before = session.messages.len();
        let mut skipped_empty_assistant = 0usize;
        let mut skipped_empty_user = 0usize;
        let mut truncated_tool_results = 0usize;

        for msg in all_msgs.into_iter().skip(start_index) {
            if msg.role == MessageRole::Assistant
                && msg.content.is_none()
                && msg.tool_calls.is_none()
            {
                skipped_empty_assistant += 1;
                continue;
            }
            if msg.role == MessageRole::User {
                match &msg.content {
                    Some(MessageContent::Text(t)) if t.trim().is_empty() => {
                        skipped_empty_user += 1;
                        continue;
                    }
                    None => {
                        skipped_empty_user += 1;
                        continue;
                    }
                    _ => {}
                }
            }

            const MAX_TOOL_RESULT_CHARS: usize = 8000;
            // 工具返回可能很长，写入会话前截断，防止会话文件膨胀导致后续上下文加载变慢。
            let content = msg.content.map(|c| match c {
                MessageContent::Text(t) => {
                    if msg.role == MessageRole::Tool && t.len() > MAX_TOOL_RESULT_CHARS {
                        truncated_tool_results += 1;
                        MessageContent::Text(format!(
                            "{}\u{2026}[truncated]",
                            truncate_utf8_prefix(&t, MAX_TOOL_RESULT_CHARS)
                        ))
                    } else {
                        MessageContent::Text(t)
                    }
                }
                other => other,
            });

            let tool_calls = msg.tool_calls.clone();

            let reasoning_content = msg.reasoning_content.clone();
            let thinking_blocks = msg.thinking_blocks.clone();

            let entry = SessionEntry {
                role: msg.role,
                content,
                timestamp: chrono::Utc::now().to_rfc3339(),
                tool_calls,
                tool_call_id: msg.tool_call_id.clone(),
                name: msg.name.clone(),
                reasoning_content,
                thinking_blocks,
            };
            session.messages.push(entry);
        }
        session.updated_at = chrono::Utc::now();
        debug!(
            target: TARGET,
            session_key = %session.key,
            saved = session.messages.len().saturating_sub(before),
            skipped_empty_assistant,
            skipped_empty_user,
            truncated_tool_results,
            total_messages = session.messages.len(),
            "persisted turn into session history"
        );
    }
}

fn truncate_utf8_prefix(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }
    let mut boundary = max_bytes;
    while boundary > 0 && !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    &value[..boundary]
}

impl Clone for AgentLoop {
    fn clone(&self) -> Self {
        Self {
            bus: self.bus.clone(),
            provider: self.provider.clone(),
            model: self.model.clone(),
            max_iterations: self.max_iterations,
            temperature: self.temperature,
            max_tokens: self.max_tokens,
            memory_window: self.memory_window,
            reasoning_effort: self.reasoning_effort.clone(),
            consolidation_config: self.consolidation_config.clone(),
            consolidation_enabled: self.consolidation_enabled,
            channel_configs: self.channel_configs.clone(),
            send_usage_summary: self.send_usage_summary,
            tools: self.tools.clone(),
            mcp: self.mcp.clone(),
            context: self.context.clone(),
            sessions: self.sessions.clone(),
            running: self.running.clone(),
            session_locks: self.session_locks.clone(),
            active_tasks: self.active_tasks.clone(),
            last_cleanup: self.last_cleanup.clone(),
        }
    }
}

#[async_trait]
impl Agent for AgentLoop {
    async fn run(self: std::sync::Arc<Self>) {
        AgentLoop::run(&*self).await;
    }

    async fn stop(&self) {
        AgentLoop::stop(self).await;
    }

    async fn process_direct(
        &self,
        content: &str,
        session_key: &SessionKey,
        channel: &str,
        chat_id: &str,
    ) -> AgentResult<String> {
        self.process_direct(content, session_key, channel, chat_id)
            .await
    }

    fn has_active_tasks(&self, session_key: &SessionKey) -> bool {
        AgentLoop::has_active_tasks(self, session_key)
    }

    async fn close_mcp(&self) {
        AgentLoop::close_mcp(self).await;
    }

    async fn close_provider(&self) {
        AgentLoop::close_provider(self).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_runtime_settings_uses_defaults_without_overrides() {
        let defaults = ConsolidationConfig {
            min_messages: 20,
            keep_recent: 12,
            max_tokens: 900,
        };

        let settings = AgentLoop::merge_runtime_settings(80, true, &defaults, None);

        assert_eq!(settings.memory_window, 80);
        assert!(settings.consolidation_enabled);
        assert_eq!(settings.consolidation_config.min_messages, 20);
        assert_eq!(settings.consolidation_config.keep_recent, 12);
        assert_eq!(settings.consolidation_config.max_tokens, 900);
    }

    #[test]
    fn merge_runtime_settings_applies_channel_overrides() {
        let defaults = ConsolidationConfig {
            min_messages: 20,
            keep_recent: 12,
            max_tokens: 900,
        };
        let overrides = AgentRuntimeOverrides {
            memory_window: Some(160),
            consolidation_enabled: Some(false),
            consolidation_keep_recent: Some(36),
            consolidation_min_messages: Some(48),
            consolidation_summary_max_tokens: Some(1800),
        };

        let settings = AgentLoop::merge_runtime_settings(80, true, &defaults, Some(&overrides));

        assert_eq!(settings.memory_window, 160);
        assert!(!settings.consolidation_enabled);
        assert_eq!(settings.consolidation_config.min_messages, 48);
        assert_eq!(settings.consolidation_config.keep_recent, 36);
        assert_eq!(settings.consolidation_config.max_tokens, 1800);
    }
}
