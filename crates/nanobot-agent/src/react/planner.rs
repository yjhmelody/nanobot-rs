//! Model query and response parsing for the ReAct loop.
//!
//! [`Planner`] sends the current conversation to the LLM provider,
//! optionally streaming the response, accumulates the result, and returns
//! a structured [`PlannerResponse`]. It also handles progress emission
//! for live-streaming channels.
//!
//! # Design Notes
//!
//! - **Streaming fallback**: If the provider does not emit any stream
//!   events within `STREAM_SETUP_TIMEOUT`, the planner falls back to
//!   a non-streaming `chat()` call.
//! - **Progress throttling**: Progress updates are rate-limited via
//!   [`Throttle`] to avoid flooding the bus with micro-updates.
//! - **Tool-call hints**: When a `ToolCallStart` event is received, a
//!   tool hint is emitted to let the user know what tool is being invoked.

use futures::StreamExt;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, trace};

use super::TARGET;
use crate::error::{AgentError, AgentResult};
use crate::utils::Throttle;
use nanobot_bus::{MessageBus, MessageId, MessageMetadata, OutboundMessage};
use nanobot_provider::streaming::{StreamAccumulator, StreamError, StreamEvent};
use nanobot_provider::{ChatRequest, LLMProvider};
use nanobot_tools::base::ToolDefinition;
use nanobot_types::provider::{
    ChatMessage, ReasoningConfig, ThinkingBlock, ToolCallRequest, UsageStats,
};

/// Minimum character delta between progress updates.
const PROGRESS_MIN_CHARS: usize = 24;
/// Minimum interval between progress updates.
const PROGRESS_MIN_INTERVAL: Duration = Duration::from_millis(500);
/// Timeout for the initial stream setup.
const STREAM_SETUP_TIMEOUT: Duration = Duration::from_secs(60);
/// Idle timeout between stream events (no data for this long triggers an
/// error).
const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(90);

/// Queries the LLM model and parses responses into [`PlannerResponse`].
///
/// Wraps an [`LLMProvider`] and adds progress-emission logic on top.
pub struct Planner {
    provider: Arc<dyn LLMProvider>,
}

impl Planner {
    /// Creates a new `Planner` backed by the given provider.
    pub fn new(provider: Arc<dyn LLMProvider>) -> Self {
        Self { provider }
    }

    /// Queries the model with the current messages and tool definitions.
    ///
    /// Attempts a streaming call first. If the stream produces no events
    /// (timeout), falls back to a non-streaming call.
    ///
    /// Streamed progress updates are emitted via [`ProgressEmitter`] when
    /// provided.
    ///
    /// # Arguments
    ///
    /// * `messages` — The conversation history + current user message.
    /// * `tools` — Available tool definitions to give the model.
    /// * `config` — Model configuration (model name, temperature, etc.).
    /// * `progress` — Optional emitter for streaming progress.
    ///
    /// # Errors
    ///
    /// Returns [`AgentError::Loop`] on provider errors, stream timeouts,
    /// or idle timeouts.
    pub async fn query(
        &self,
        messages: &[ChatMessage],
        tools: &[Arc<ToolDefinition>],
        config: &ModelConfig,
        progress: Option<&ProgressEmitter>,
    ) -> AgentResult<PlannerResponse> {
        debug!(
            target: TARGET,
            iteration = config.iteration,
            message_count = messages.len(),
            "Querying model"
        );

        let request = ChatRequest {
            session_key: None,
            model: Some(config.model.clone()),
            messages: messages.to_vec(),
            tools: if tools.is_empty() {
                None
            } else {
                Some(tools.to_vec())
            },
            temperature: config.temperature,
            max_tokens: config.max_tokens,
            reasoning_effort: config.reasoning_effort.clone(),
        };

        // Emit a stream-start progress marker before provider request so channels that
        // support begin_stream (e.g. Feishu placeholder) can show "thinking" even when
        // the provider doesn't emit incremental deltas (chat_completions path).
        if let Some(progress) = progress {
            progress.send_progress_start();
        }

        let mut stream = tokio::time::timeout(
            STREAM_SETUP_TIMEOUT,
            self.provider.chat_stream(request.clone()),
        )
        .await
        .map_err(|_| {
            AgentError::loop_error(format!(
                "provider stream setup timeout (model='{}', iteration={}, timeout={}s)",
                config.model,
                config.iteration,
                STREAM_SETUP_TIMEOUT.as_secs()
            ))
        })?
        .map_err(|err| map_stream_error(err, &config.model, config.iteration))?;

        let mut accumulator = StreamAccumulator::new();
        let mut progress_state = ProgressState::new();
        let mut saw_event = false;
        let mut progress_throttle = Throttle::new(PROGRESS_MIN_CHARS, PROGRESS_MIN_INTERVAL);
        let mut done_response = None;

        while let Some(event) = tokio::time::timeout(STREAM_IDLE_TIMEOUT, stream.next())
            .await
            .map_err(|_| {
                AgentError::loop_error(format!(
                    "provider stream idle timeout (model='{}', iteration={}, timeout={}s)",
                    config.model,
                    config.iteration,
                    STREAM_IDLE_TIMEOUT.as_secs()
                ))
            })?
        {
            let event =
                event.map_err(|err| map_stream_error(err, &config.model, config.iteration))?;
            saw_event = true;

            match &event {
                StreamEvent::Done { response } => {
                    done_response = Some(response.clone());
                    break;
                }
                StreamEvent::Error { message } => {
                    return Err(AgentError::loop_error(format!(
                        "provider stream error (model='{}', iteration={}): {}",
                        config.model, config.iteration, message
                    )));
                }
                StreamEvent::ToolCallStart { name, .. } => {
                    if let Some(progress) = progress {
                        progress.send_tool_hint(&format!("Using: {}", name));
                    }
                }
                _ => {}
            }

            accumulator.process_event(&event);

            // Throttled progress emission for text deltas
            if let Some(progress) = progress
                && let Some(content) = progress_state.apply_event(&event)
                && progress_throttle.should_send(content.len())
            {
                progress.send_progress(&content);
                progress_throttle.mark_sent(content.len());
            }
        }

        // Send the final accumulated content
        if let Some(progress) = progress {
            let content = progress_state.content();
            if !content.is_empty() && content.len() != progress_throttle.last_sent_len() {
                progress.send_progress(&content);
            }
        }

        // If no stream events were received, fall back to a non-streaming call
        let response = if !saw_event {
            self.provider.chat(request).await.map_err(|e| {
                AgentError::loop_error(format!(
                    "llm provider error (model='{}', iteration={}): {}",
                    config.model, config.iteration, e
                ))
            })?
        } else {
            done_response.unwrap_or_else(|| accumulator.build_response())
        };

        trace!(
            target: TARGET,
            content_len = response.content.as_ref().map(|s| s.len()).unwrap_or(0),
            tool_calls = response.tool_calls.len(),
            "Model response received"
        );

        Ok(PlannerResponse {
            content: response.content,
            tool_calls: response.tool_calls,
            finish_reason: response.finish_reason,
            reasoning_content: response.reasoning_content,
            thinking_blocks: response.thinking_blocks,
            usage: response.usage,
        })
    }
}

/// Emits streaming progress updates and tool-hint messages to the
/// message bus during LLM response generation.
///
/// Progress messages are sent with `MessageId::Progress` and tool hints
/// with `MessageId::ToolHint`, allowing channel adapters to handle them
/// appropriately (e.g. edit a placeholder message vs. send a status line).
#[derive(Clone)]
pub struct ProgressEmitter {
    bus: MessageBus,
    channel: String,
    chat_id: String,
    reply_to: Option<String>,
    stream_id: String,
}

impl ProgressEmitter {
    /// Creates a new `ProgressEmitter`.
    ///
    /// * `bus` — Message bus to publish progress on.
    /// * `channel` — Target channel.
    /// * `chat_id` — Target chat.
    /// * `reply_to` — Optional message ID to reply to.
    /// * `stream_id` — A unique stream identifier for correlating
    ///   progress updates with the final message.
    pub fn new(
        bus: MessageBus,
        channel: impl Into<String>,
        chat_id: impl Into<String>,
        reply_to: Option<String>,
        stream_id: impl Into<String>,
    ) -> Self {
        Self {
            bus,
            channel: channel.into(),
            chat_id: chat_id.into(),
            reply_to,
            stream_id: stream_id.into(),
        }
    }

    /// Sends a streaming progress update.
    pub fn send_progress(&self, content: &str) {
        if content.trim().is_empty() {
            return;
        }
        let _ = self.bus.publish_outbound(OutboundMessage {
            channel: self.channel.clone(),
            chat_id: self.chat_id.clone(),
            content: content.to_string(),
            reply_to: self.reply_to.clone(),
            media: Vec::new(),
            metadata: MessageMetadata {
                message_id: Some(MessageId::Progress),
                stream_id: Some(self.stream_id.clone()),
            },
        });
    }

    /// Sends a stream-start marker (empty content) so channels that
    /// support begin_stream (e.g. Feishu placeholder) can show a
    /// "thinking" indicator.
    pub fn send_progress_start(&self) {
        let _ = self.bus.publish_outbound(OutboundMessage {
            channel: self.channel.clone(),
            chat_id: self.chat_id.clone(),
            content: String::new(),
            reply_to: self.reply_to.clone(),
            media: Vec::new(),
            metadata: MessageMetadata {
                message_id: Some(MessageId::Progress),
                stream_id: Some(self.stream_id.clone()),
            },
        });
    }

    /// Sends a tool-hint message (e.g. "Using: read_file") to inform the
    /// user about tool execution.
    pub fn send_tool_hint(&self, content: &str) {
        if content.trim().is_empty() {
            return;
        }
        let _ = self.bus.publish_outbound(OutboundMessage {
            channel: self.channel.clone(),
            chat_id: self.chat_id.clone(),
            content: content.to_string(),
            reply_to: self.reply_to.clone(),
            media: Vec::new(),
            metadata: MessageMetadata {
                message_id: Some(MessageId::ToolHint),
                stream_id: Some(self.stream_id.clone()),
            },
        });
    }
}

/// Tracks streaming text content across multiple content blocks,
/// used by [`Planner`] to build incremental progress updates.
struct ProgressState {
    content_blocks: Vec<String>,
}

impl ProgressState {
    fn new() -> Self {
        Self {
            content_blocks: Vec::new(),
        }
    }

    /// Applies a stream event and returns the current accumulated content
    /// if the event produced new text.
    fn apply_event(&mut self, event: &StreamEvent) -> Option<String> {
        match event {
            StreamEvent::TextDelta { content, index } => {
                while self.content_blocks.len() <= *index {
                    self.content_blocks.push(String::new());
                }
                self.content_blocks[*index].push_str(content);
                Some(self.content())
            }
            _ => None,
        }
    }

    /// Returns the full accumulated text content.
    fn content(&self) -> String {
        self.content_blocks.join("")
    }
}

/// Maps a [`StreamError`] to an [`AgentError`].
fn map_stream_error(err: StreamError, model: &str, iteration: usize) -> AgentError {
    AgentError::loop_error(format!(
        "provider stream error (model='{}', iteration={}): {}",
        model, iteration, err
    ))
}

/// Configuration for a single model query within the ReAct loop.
///
/// Includes the model identifier, sampling parameters, and the current
/// iteration number for diagnostic logging.
#[derive(Debug, Clone)]
pub struct ModelConfig {
    /// Model identifier (e.g. `"anthropic/claude-opus-4-6"`).
    pub model: String,
    /// LLM sampling temperature.
    pub temperature: f32,
    /// Maximum output tokens for this call.
    pub max_tokens: i32,
    /// Optional extended-thinking configuration.
    pub reasoning_effort: Option<ReasoningConfig>,
    /// Current ReAct iteration (0-based, for logging).
    pub iteration: usize,
}

/// Parsed response from a single model query.
#[derive(Debug)]
pub struct PlannerResponse {
    /// Text content of the response (None if only tool calls).
    pub content: Option<String>,
    /// Tool calls requested by the model.
    pub tool_calls: Vec<ToolCallRequest>,
    /// Finish reason from the provider (e.g. `"end_turn"`, `"length"`).
    pub finish_reason: String,
    /// Reasoning/chain-of-thought content (provider-specific).
    pub reasoning_content: Option<String>,
    /// Extended thinking blocks (provider-specific).
    pub thinking_blocks: Option<Vec<ThinkingBlock>>,
    /// Token usage statistics for this call.
    pub usage: UsageStats,
}

impl PlannerResponse {
    /// Returns `true` if this is a final answer (no tool calls and not
    /// truncated by `max_tokens`).
    pub fn is_final(&self) -> bool {
        self.tool_calls.is_empty() && self.finish_reason != "length"
    }

    /// Returns `true` if the model wants to call tools.
    pub fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }

    /// Returns `true` if the response was truncated because it hit
    /// `max_tokens`.
    pub fn is_truncated(&self) -> bool {
        self.finish_reason == "length"
    }
}
