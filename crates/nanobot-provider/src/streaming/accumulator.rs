//! Streaming event accumulator that builds a complete [`LLMResponse`] from events.
//!
//! [`StreamAccumulator`] provides a simple state machine that processes streaming events
//! in order and produces a fully populated [`LLMResponse`] at the end. It handles:
//!
//! - Text content from multiple content blocks (joined with double newlines)
//! - Thinking blocks from reasoning content deltas
//! - Tool calls with incremental argument JSON deltas
//! - Usage statistics updates (cumulative)
//! - Finish reason propagation
//!
//! # Usage
//!
//! ```ignore
//! let mut acc = StreamAccumulator::new();
//! for event in stream {
//!     acc.process_event(&event);
//! }
//! let response = acc.build_response();
//! ```

use std::collections::HashMap;

use crate::{LLMResponse, ThinkingBlock, ToolCallRequest, ToolName, UsageStats};

use super::events::StreamEvent;

/// Accumulates streaming events to build a complete response.
///
/// This is a stateful accumulator that must receive events in order. It is not
/// thread-safe and should be used from a single task.
pub struct StreamAccumulator {
    /// Text content blocks, indexed by their block position.
    content_blocks: Vec<String>,
    /// Thinking/reasoning blocks accumulated from `ThinkingDelta` events.
    thinking_blocks: Vec<ThinkingBlock>,
    /// In-progress tool calls, keyed by tool call id.
    tool_calls: HashMap<String, ToolCallBuilder>,
    /// Maps output index to tool call id for delta routing.
    tool_calls_by_index: HashMap<usize, String>,
    /// Accumulated usage statistics.
    usage: UsageStats,
    /// Finish reason, if received.
    finish_reason: Option<String>,
}

/// Internal builder for incrementally constructing a tool call from streaming deltas.
struct ToolCallBuilder {
    id: String,
    name: String,
    /// JSON arguments string being built incrementally from `ToolCallArgumentsDelta` events.
    arguments_json: String,
}

impl StreamAccumulator {
    /// Creates a new empty accumulator.
    pub fn new() -> Self {
        Self {
            content_blocks: Vec::new(),
            thinking_blocks: Vec::new(),
            tool_calls: HashMap::new(),
            tool_calls_by_index: HashMap::new(),
            usage: UsageStats::default(),
            finish_reason: None,
        }
    }

    /// Processes a streaming event and updates internal state.
    pub fn process_event(&mut self, event: &StreamEvent) {
        match event {
            StreamEvent::TextDelta { content, index } => {
                self.ensure_content_block(*index);
                self.content_blocks[*index].push_str(content);
            }
            StreamEvent::ThinkingDelta { content } => {
                if self.thinking_blocks.is_empty() {
                    self.thinking_blocks.push(ThinkingBlock::new(String::new()));
                }
                self.thinking_blocks
                    .last_mut()
                    .unwrap()
                    .thinking
                    .push_str(content);
            }
            StreamEvent::SignatureDelta { signature } => {
                if self.thinking_blocks.is_empty() {
                    self.thinking_blocks.push(ThinkingBlock::new(String::new()));
                }
                self.thinking_blocks.last_mut().unwrap().signature = Some(signature.clone());
            }
            StreamEvent::ToolCallStart { id, name, index } => {
                self.tool_calls.insert(
                    id.clone(),
                    ToolCallBuilder {
                        id: id.clone(),
                        name: name.clone(),
                        arguments_json: String::new(),
                    },
                );
                self.tool_calls_by_index.insert(*index, id.clone());
            }
            StreamEvent::ToolCallArgumentsDelta {
                id, arguments_json, ..
            } => {
                if let Some(builder) = self.tool_calls.get_mut(id) {
                    builder.arguments_json.push_str(arguments_json);
                }
            }
            StreamEvent::ToolCallEnd { .. } => {
                // Tool call ended, no special handling needed
            }
            StreamEvent::UsageUpdate {
                input_tokens,
                output_tokens,
                total_tokens,
            } => {
                if let Some(tokens) = input_tokens {
                    self.usage.prompt_tokens = Some(*tokens as u64);
                }
                if let Some(tokens) = output_tokens {
                    self.usage.completion_tokens = Some(*tokens as u64);
                }
                if let Some(tokens) = total_tokens {
                    self.usage.total_tokens = Some(*tokens as u64);
                }
            }
            StreamEvent::FinishReasonUpdate { reason } => {
                self.finish_reason = Some(reason.clone());
            }
            StreamEvent::Done { .. } | StreamEvent::Error { .. } => {
                // These events don't need accumulation
            }
        }
    }

    /// Consumes the accumulator and produces the final [`LLMResponse`].
    ///
    /// Text content blocks are joined with double newlines. Tool calls are collected
    /// into a vec (order is not guaranteed). If no finish reason was received, defaults to `"stop"`.
    /// Thinking blocks with empty content are discarded.
    pub fn build_response(self) -> LLMResponse {
        let content = if self.content_blocks.is_empty() {
            None
        } else {
            Some(self.content_blocks.join("\n\n"))
        };

        let thinking_blocks = if self.thinking_blocks.iter().all(|b| b.thinking.is_empty()) {
            None
        } else {
            Some(self.thinking_blocks)
        };

        let tool_calls = self
            .tool_calls
            .into_values()
            .map(|builder| ToolCallRequest {
                id: builder.id,
                name: ToolName::from(builder.name),
                arguments_json: builder.arguments_json,
            })
            .collect();

        LLMResponse {
            content,
            tool_calls,
            finish_reason: self.finish_reason.unwrap_or_else(|| "stop".to_string()),
            usage: self.usage,
            reasoning_content: None,
            thinking_blocks,
        }
    }

    /// Ensures that the content block at `index` exists, extending the vec with
    /// empty strings as needed. This handles out-of-order or missing content blocks.
    fn ensure_content_block(&mut self, index: usize) {
        while self.content_blocks.len() <= index {
            self.content_blocks.push(String::new());
        }
    }
}

impl Default for StreamAccumulator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accumulator_builds_text_response() {
        let mut acc = StreamAccumulator::new();

        acc.process_event(&StreamEvent::TextDelta {
            content: "Hello".to_string(),
            index: 0,
        });
        acc.process_event(&StreamEvent::TextDelta {
            content: " world".to_string(),
            index: 0,
        });

        let response = acc.build_response();
        assert_eq!(response.content.as_deref(), Some("Hello world"));
        assert_eq!(response.finish_reason, "stop");
    }

    #[test]
    fn accumulator_handles_multiple_content_blocks() {
        let mut acc = StreamAccumulator::new();

        acc.process_event(&StreamEvent::TextDelta {
            content: "Block 1".to_string(),
            index: 0,
        });
        acc.process_event(&StreamEvent::TextDelta {
            content: "Block 2".to_string(),
            index: 1,
        });

        let response = acc.build_response();
        assert_eq!(response.content.as_deref(), Some("Block 1\n\nBlock 2"));
    }

    #[test]
    fn accumulator_builds_thinking_blocks() {
        let mut acc = StreamAccumulator::new();

        acc.process_event(&StreamEvent::ThinkingDelta {
            content: "Let me think...".to_string(),
        });
        acc.process_event(&StreamEvent::ThinkingDelta {
            content: " about this.".to_string(),
        });

        let response = acc.build_response();
        assert_eq!(
            response.thinking_blocks.as_deref(),
            Some(&[ThinkingBlock::new("Let me think... about this.")][..])
        );
    }

    #[test]
    fn accumulator_builds_tool_calls() {
        let mut acc = StreamAccumulator::new();

        acc.process_event(&StreamEvent::ToolCallStart {
            id: "call_1".to_string(),
            name: "read_file".to_string(),
            index: 0,
        });
        acc.process_event(&StreamEvent::ToolCallArgumentsDelta {
            id: "call_1".to_string(),
            arguments_json: r#"{"path":"#.to_string(),
            index: 0,
        });
        acc.process_event(&StreamEvent::ToolCallArgumentsDelta {
            id: "call_1".to_string(),
            arguments_json: r#""test.txt"}"#.to_string(),
            index: 0,
        });

        let response = acc.build_response();
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].id, "call_1");
        assert_eq!(response.tool_calls[0].name.as_str(), "read_file");
        assert_eq!(
            response.tool_calls[0].arguments_json,
            r#"{"path":"test.txt"}"#
        );
    }

    #[test]
    fn accumulator_updates_usage_stats() {
        let mut acc = StreamAccumulator::new();

        acc.process_event(&StreamEvent::UsageUpdate {
            input_tokens: Some(10),
            output_tokens: None,
            total_tokens: None,
        });
        acc.process_event(&StreamEvent::UsageUpdate {
            input_tokens: None,
            output_tokens: Some(20),
            total_tokens: Some(30),
        });

        let response = acc.build_response();
        assert_eq!(response.usage.prompt_tokens, Some(10));
        assert_eq!(response.usage.completion_tokens, Some(20));
        assert_eq!(response.usage.total_tokens, Some(30));
    }

    #[test]
    fn accumulator_updates_finish_reason() {
        let mut acc = StreamAccumulator::new();

        acc.process_event(&StreamEvent::FinishReasonUpdate {
            reason: "tool_calls".to_string(),
        });

        let response = acc.build_response();
        assert_eq!(response.finish_reason, "tool_calls");
    }

    #[test]
    fn accumulator_default_finish_reason_is_stop() {
        let acc = StreamAccumulator::new();
        let response = acc.build_response();
        assert_eq!(response.finish_reason, "stop");
    }
}
