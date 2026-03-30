//! Simplified AgentLoop using modular ReAct engine

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use dashmap::DashMap;
use parking_lot::Mutex;
use tokio::task::AbortHandle;
use tracing::{Instrument, debug, debug_span, error, info, trace};

use crate::traits::{Agent, ContextProvider};
use crate::react::{
    ExecutionContext, LoopOutcome, ModelConfig, ProgressEmitter, ReActExecutor,
};
use crate::error::AgentResult;
use nanobot_bus::{
    InboundCommand, InboundMessage, MessageBus, MessageId, MessageMetadata, OutboundMessage,
};
use nanobot_provider::LLMProvider;
use nanobot_session::{ConsolidationOutcome, Session, SessionEntry, SessionManager};
use nanobot_tools::mcp::MCPManager;
use nanobot_tools::{ToolContext, ToolRegistry};
use nanobot_types::SessionKey;
use nanobot_types::provider::{ChatMessage, MessageContent, MessageRole, UsageStats};
use nanobot_types::task::TaskId;
use crate::utils::preview_text;

const TARGET_AGENT: &str = "nanobot.agent";

pub struct AgentLoop {
    pub(crate) bus: MessageBus,
    pub(crate) provider: Arc<dyn LLMProvider>,
    pub(crate) model: String,
    pub(crate) max_iterations: usize,
    pub(crate) temperature: f32,
    pub(crate) max_tokens: i32,
    pub(crate) memory_window: usize,
    pub(crate) reasoning_effort: Option<String>,
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

impl AgentLoop {
    const CLEANUP_INTERVAL: Duration = Duration::from_secs(300);

    async fn ensure_mcp_connected(&self) {
        if let Some(mcp) = &self.mcp {
            if let Err(err) = mcp.connect_if_needed(&self.tools).await {
                error!(
                    target: TARGET_AGENT,
                    "failed to connect MCP servers (will retry on next message): {}",
                    err
                );
            }
        }
    }

    pub async fn close_mcp(&self) {
        if let Some(mcp) = &self.mcp {
            debug!(target: TARGET_AGENT, "closing MCP manager");
            mcp.close(&self.tools).await;
            debug!(target: TARGET_AGENT, "MCP manager closed");
        }
    }

    pub async fn close_provider(&self) {
        debug!(target: TARGET_AGENT, "closing provider");
        self.provider.close().await;
        debug!(target: TARGET_AGENT, "provider closed");
    }

    pub async fn run(&self) {
        self.running.store(true, Ordering::Release);
        self.ensure_mcp_connected().await;
        info!(target: TARGET_AGENT, "agent loop started");
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
                target: TARGET_AGENT,
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
                target: TARGET_AGENT,
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

        info!(target: TARGET_AGENT, "agent loop stopped");
    }

    pub async fn stop(&self) {
        self.running.store(false, Ordering::Release);
        info!(target: TARGET_AGENT, "stopping agent loop");
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
        debug!(target: TARGET_AGENT, aborted, "cleared active task registry during shutdown");
    }

    pub async fn process_direct(
        &self,
        content: &str,
        session_key: &SessionKey,
        channel: &str,
        chat_id: &str,
    ) -> AgentResult<String> {
        debug!(
            target: TARGET_AGENT,
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
        if self.send_usage_summary {
            if let Some(usage_text) = out.as_ref().and_then(|o| o.usage.as_ref()).map(|u| {
                format!("\n\n---\n_Tokens: {} in / {} out_", u.prompt_tokens.unwrap_or(0), u.completion_tokens.unwrap_or(0))
            }) {
                content.push_str(&usage_text);
            }
        }
        debug!(
            target: TARGET_AGENT,
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
            .or_insert_with(DashMap::new)
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
            format!("\u{23f9} Stopped {} task(s).", total)
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
        let session_key = msg.session_key();
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
                target: TARGET_AGENT,
                session_key = %session_key,
                lock_wait_ms = lock_wait.as_millis(),
                "acquired session lock after wait"
            );
        }

        match self.process_message(msg.clone()).await {
            Ok(Some(out)) => {
                if let Err(err) = self.bus.publish_outbound(out.message.clone()) {
                    error!(target: TARGET_AGENT, error = %err, "failed to publish outbound message");
                }
                if self.send_usage_summary {
                    if let Some(usage) = out.usage {
                        let usage_msg = OutboundMessage {
                            channel: out.message.channel.clone(),
                            chat_id: out.message.chat_id.clone(),
                            content: format!("_Tokens: {} in / {} out_", usage.prompt_tokens.unwrap_or(0), usage.completion_tokens.unwrap_or(0)),
                            reply_to: None,
                            media: Vec::new(),
                            metadata: MessageMetadata::default(),
                        };
                        if let Err(err) = self.bus.publish_outbound(usage_msg) {
                            error!(target: TARGET_AGENT, session_key = %session_key, error = %err, "failed to publish usage summary");
                        }
                    }
                }
            }
            Ok(None) => {
                trace!(target: TARGET_AGENT, session_key = %session_key, "no outbound message to publish");
            }
            Err(err) => {
                error!(target: TARGET_AGENT, session_key = %session_key, error = %err, "error processing message");
                let _ = self.bus.publish_outbound(OutboundMessage {
                    channel: msg.channel,
                    chat_id: msg.chat_id,
                    content: format!("Error: {}", err),
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
                target: TARGET_AGENT,
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

    async fn process_message(
        &self,
        msg: InboundMessage,
    ) -> AgentResult<Option<OutboundEnvelope>> {
        trace!(
            target: TARGET_AGENT,
            session_key = %msg.session_key(),
            content_preview = %preview_text(msg.content_text(), 120),
            media_count = msg.media.len(),
            message_id = ?msg.metadata.message_id,
            "process_message start"
        );
        if msg.channel == "system" {
            return self.process_system_message(msg).await.map(|msg| {
                msg.map(|message| OutboundEnvelope { message, usage: None })
            });
        }

        if let Some(command) = msg.command() {
            return self.process_builtin_command(msg, command).await.map(|msg| {
                msg.map(|message| OutboundEnvelope { message, usage: None })
            });
        }

        let session_key = msg.session_key();
        let mut session = self.sessions.get_or_create(session_key.as_str()).await?;

        let tool_context = ToolContext {
            channel: msg.channel.clone(),
            chat_id: msg.chat_id.clone(),
            session_key: session_key.clone(),
            message_id: msg.metadata.message_id.clone(),
        };

        let _ = self.tools.start_turn().await?;

        let history = self
            .sessions
            .get_history(&session, self.memory_window)
            .await?;
        let history_len = history.len();
        let messages = self
            .context
            .build_messages(
                &self.sessions,
                session_key.as_str(),
                history,
                msg.content_text(),
                if msg.media.is_empty() { None } else { Some(&msg.media) },
                Some(&msg.channel),
                Some(&msg.chat_id),
            )
            .await;

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

        self.save_turn(&mut session, outcome.messages, start_index);
        self.sessions.save(&mut session).await?;

        if self.tools.message_sent_in_turn().await {
            return Ok(None);
        }

        Ok(Some(OutboundEnvelope {
            usage: outcome.usage.clone(),
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
                content: "\u{1f408} nanobot commands:\n/new - Start a new conversation\n/stop - Stop the current task\n/compact - Consolidate session history\n/help - Show available commands".to_string(),
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
                    content: "\u{1f195} Starting a new conversation.".to_string(),
                    reply_to: None,
                    media: Vec::new(),
                    metadata: msg.metadata,
                }))
            }
            InboundCommand::Compact => {
                let session_key = msg.session_key();
                let mut session = self.sessions.get_or_create(session_key.as_str()).await?;
                let content = match self.sessions.consolidate_now(&mut session).await {
                    Err(e) => format!("Failed to consolidate: {}", e),
                    Ok(ConsolidationOutcome::Disabled) => {
                        "Consolidation is not configured.".to_string()
                    }
                    Ok(ConsolidationOutcome::Skipped) => {
                        "Not enough messages to consolidate yet.".to_string()
                    }
                    Ok(ConsolidationOutcome::Consolidated { removed }) => {
                        format!("\u{2705} Consolidated {} messages.", removed)
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
            .run(messages, self.tools.definitions(), config, exec_context, progress)
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
            let content = msg.content.map(|c| match c {
                MessageContent::Text(t) => {
                    if msg.role == MessageRole::Tool && t.len() > MAX_TOOL_RESULT_CHARS {
                        truncated_tool_results += 1;
                        MessageContent::Text(format!(
                            "{}\u{2026}[truncated]",
                            &t[..MAX_TOOL_RESULT_CHARS]
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
            target: TARGET_AGENT,
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
        self.process_direct(content, session_key, channel, chat_id).await
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






