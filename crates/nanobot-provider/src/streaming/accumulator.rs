use std::collections::HashMap;

use crate::{LLMResponse, ToolCallRequest, ToolName, UsageStats};

use super::events::StreamEvent;

/// Accumulates streaming events to build a complete response.
pub struct StreamAccumulator {
    content_blocks: Vec<String>,
    thinking_blocks: Vec<String>,
    tool_calls: HashMap<String, ToolCallBuilder>,
    tool_calls_by_index: HashMap<usize, String>,
    usage: UsageStats,
    finish_reason: Option<String>,
}

struct ToolCallBuilder {
    id: String,
    name: String,
    arguments_json: String,
}

impl StreamAccumulator {
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
                    self.thinking_blocks.push(String::new());
                }
                self.thinking_blocks.last_mut().unwrap().push_str(content);
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
                dbg!(total_tokens);
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

    /// Builds the final LLMResponse from accumulated events.
    pub fn build_response(self) -> LLMResponse {
        let content = if self.content_blocks.is_empty() {
            None
        } else {
            Some(self.content_blocks.join("\n\n"))
        };

        let thinking_blocks = if self.thinking_blocks.is_empty() {
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
            Some(&["Let me think... about this.".to_string()][..])
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
