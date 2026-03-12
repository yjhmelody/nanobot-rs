//! Model query and response parsing for ReAct loop

use futures::StreamExt;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, trace};

use crate::bus::{MessageBus, MessageId, MessageMetadata, OutboundMessage};
use crate::error::Result;
use crate::error::{NanobotError, ProviderError};
use crate::observability::TARGET_REACT;
use crate::provider::streaming::{StreamAccumulator, StreamError, StreamEvent};
use crate::provider::{ChatRequest, LLMProvider};
use crate::tools::base::ToolDefinition;
use crate::types::provider::{ChatMessage, ToolCallRequest, UsageStats};
use crate::utils::throttle::{Throttle, truncate_text};

const PROGRESS_MIN_CHARS: usize = 24;
const PROGRESS_MIN_INTERVAL: Duration = Duration::from_millis(500);
const TOOL_HINT_MIN_CHARS: usize = 24;
const TOOL_HINT_MIN_INTERVAL: Duration = Duration::from_millis(500);
const TOOL_HINT_MAX_CHARS: usize = 480;

/// Queries the model and parses responses
pub struct Planner {
    provider: Arc<dyn LLMProvider>,
}

impl Planner {
    pub fn new(provider: Arc<dyn LLMProvider>) -> Self {
        Self { provider }
    }

    /// Query model with current messages and available tools
    pub async fn query(
        &self,
        messages: &[ChatMessage],
        tools: &[Arc<ToolDefinition>],
        config: &ModelConfig,
        progress: Option<&ProgressEmitter>,
    ) -> Result<PlannerResponse> {
        debug!(
            target: TARGET_REACT,
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

        let mut stream = self
            .provider
            .chat_stream(request.clone())
            .await
            .map_err(map_stream_error)?;

        let mut accumulator = StreamAccumulator::new();
        let mut progress_state = ProgressState::new();
        let mut tool_hint_state = ToolHintState::new();
        let mut saw_event = false;
        let mut progress_throttle = Throttle::new(PROGRESS_MIN_CHARS, PROGRESS_MIN_INTERVAL);
        let mut done_response = None;

        while let Some(event) = stream.next().await {
            let event = event.map_err(map_stream_error)?;
            saw_event = true;

            match &event {
                StreamEvent::Done { response } => {
                    done_response = Some(response.clone());
                    break;
                }
                StreamEvent::Error { message } => {
                    return Err(NanobotError::Provider(ProviderError::Other(
                        message.clone(),
                    )));
                }
                StreamEvent::ToolCallStart { id, name, index } => {
                    if let Some(progress) = progress {
                        progress.send_tool_hint(&format!(
                            "Tool call started: {} (id={}, index={})",
                            name, id, index
                        ));
                    }
                }
                StreamEvent::ToolCallArgumentsDelta {
                    id,
                    arguments_json,
                    index,
                } => {
                    if let Some(progress) = progress {
                        if let Some(hint) = tool_hint_state.update_args(id, arguments_json, *index)
                        {
                            progress.send_tool_hint(&hint);
                        }
                    }
                }
                StreamEvent::ToolCallEnd { id, index } => {
                    if let Some(progress) = progress {
                        if let Some(hint) = tool_hint_state.finish_call(id, *index) {
                            progress.send_tool_hint(&hint);
                        }
                    }
                }
                _ => {}
            }

            accumulator.process_event(&event);

            if let Some(progress) = progress {
                if let Some(content) = progress_state.apply_event(&event) {
                    if progress_throttle.should_send(content.len()) {
                        progress.send_progress(&content);
                        progress_throttle.mark_sent(content.len());
                    }
                }
            }
        }

        if let Some(progress) = progress {
            if let Some(content) = progress_state.content() {
                if progress_throttle.should_send(content.len()) {
                    progress.send_progress(&content);
                }
            }
        }

        let response = if !saw_event {
            self.provider.chat(request).await
        } else {
            done_response.unwrap_or_else(|| accumulator.build_response())
        };

        trace!(
            target: TARGET_REACT,
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

#[derive(Clone)]
pub struct ProgressEmitter {
    bus: MessageBus,
    channel: String,
    chat_id: String,
    reply_to: Option<String>,
    stream_id: String,
}

impl ProgressEmitter {
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

struct ProgressState {
    content_blocks: Vec<String>,
}

impl ProgressState {
    fn new() -> Self {
        Self {
            content_blocks: Vec::new(),
        }
    }

    fn apply_event(&mut self, event: &StreamEvent) -> Option<String> {
        match event {
            StreamEvent::TextDelta { content, index } => {
                self.ensure_block(*index);
                self.content_blocks[*index].push_str(content);
                Some(self.content_blocks.join("\n\n"))
            }
            _ => None,
        }
    }

    fn content(&self) -> Option<String> {
        if self.content_blocks.is_empty() {
            None
        } else {
            Some(self.content_blocks.join("\n\n"))
        }
    }

    fn ensure_block(&mut self, index: usize) {
        while self.content_blocks.len() <= index {
            self.content_blocks.push(String::new());
        }
    }
}

fn map_stream_error(err: StreamError) -> NanobotError {
    NanobotError::Provider(ProviderError::Other(err.to_string()))
}

struct ToolHintState {
    calls: HashMap<String, ToolHintCall>,
}

struct ToolHintCall {
    args: String,
    throttle: Throttle,
    index: usize,
}

impl ToolHintState {
    fn new() -> Self {
        Self {
            calls: HashMap::new(),
        }
    }

    fn update_args(&mut self, id: &str, delta: &str, index: usize) -> Option<String> {
        let entry = self
            .calls
            .entry(id.to_string())
            .or_insert_with(|| ToolHintCall {
                args: String::new(),
                throttle: Throttle::new(TOOL_HINT_MIN_CHARS, TOOL_HINT_MIN_INTERVAL),
                index,
            });

        entry.args.push_str(delta);

        if !entry.throttle.should_send(entry.args.len()) {
            return None;
        }

        entry.throttle.mark_sent(entry.args.len());

        Some(format!(
            "Tool call args (id={}, index={}): {}",
            id,
            entry.index,
            truncate_text(&entry.args, TOOL_HINT_MAX_CHARS)
        ))
    }

    fn finish_call(&mut self, id: &str, index: usize) -> Option<String> {
        let args = self
            .calls
            .remove(id)
            .map(|call| call.args)
            .unwrap_or_default();

        if args.is_empty() {
            Some(format!("Tool call ready: id={}, index={}", id, index))
        } else {
            Some(format!(
                "Tool call ready: id={}, index={}, args={}",
                id,
                index,
                truncate_text(&args, TOOL_HINT_MAX_CHARS)
            ))
        }
    }
}

// Remove the old truncate_for_hint function as we now use truncate_text from utils

/// Configuration for model query
#[derive(Debug, Clone)]
pub struct ModelConfig {
    pub model: String,
    pub temperature: f32,
    pub max_tokens: i32,
    pub reasoning_effort: Option<String>,
    pub iteration: usize,
}

/// Response from model query
#[derive(Debug)]
pub struct PlannerResponse {
    pub content: Option<String>,
    pub tool_calls: Vec<ToolCallRequest>,
    pub finish_reason: String,
    pub reasoning_content: Option<String>,
    pub thinking_blocks: Option<Vec<String>>,
    pub usage: UsageStats,
}

impl PlannerResponse {
    /// Check if this is a final answer (no tool calls and not truncated)
    pub fn is_final(&self) -> bool {
        self.tool_calls.is_empty() && self.finish_reason != "length"
    }

    /// Check if model wants to use tools
    pub fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }

    /// Check if response was truncated due to max_tokens limit
    pub fn is_truncated(&self) -> bool {
        self.finish_reason == "length"
    }
}
