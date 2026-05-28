use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use std::collections::HashMap;

use super::adapter::StreamAdapter;
use super::events::{StreamError, StreamEvent, StreamResponse};
use crate::anthropic_types::{
    AnthropicStreamContentBlock, AnthropicStreamContentDelta, AnthropicStreamEvent,
};

/// SSE format adapter (for Anthropic).
///
/// Anthropic API uses standard Server-Sent Events (SSE) format:
/// Spec source: https://docs.anthropic.com/en/docs/build-with-claude/streaming
/// ```text
/// event: message_start
/// data: {"type":"message_start","message":{"id":"msg_123",...}}
///
/// event: content_block_delta
/// data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}
///
/// event: message_stop
/// data: {"type":"message_stop"}
/// ```
pub struct SseAdapter;

#[async_trait]
impl StreamAdapter for SseAdapter {
    async fn adapt_stream(
        &self,
        response: reqwest::Response,
    ) -> Result<StreamResponse, StreamError> {
        let bytes_stream = response.bytes_stream();

        let event_stream = bytes_stream
            .map(|chunk_result| chunk_result.map_err(|e| StreamError::Network(e.to_string())))
            .scan(SseParser::new(), |parser, chunk_result| {
                let result = match chunk_result {
                    Ok(chunk) => parser.parse_chunk(chunk),
                    Err(e) => vec![Err(e)],
                };
                futures::future::ready(Some(result))
            })
            .flat_map(futures::stream::iter);

        Ok(Box::pin(event_stream))
    }
}

/// SSE parser (state machine).
struct SseParser {
    buffer: String,
    current_event: Option<SseEvent>,
    /// Tracks currently processing content blocks for associating tool call IDs.
    content_blocks: HashMap<usize, ContentBlockState>,
}

#[derive(Debug)]
struct SseEvent {
    event_type: Option<String>,
    data: String,
}

#[derive(Debug, Clone)]
struct ContentBlockState {
    #[allow(dead_code)]
    block_type: String,
    tool_call_id: Option<String>,
    #[allow(dead_code)]
    tool_call_name: Option<String>,
}

impl SseParser {
    fn new() -> Self {
        Self {
            buffer: String::new(),
            current_event: None,
            content_blocks: HashMap::new(),
        }
    }

    fn parse_chunk(&mut self, chunk: Bytes) -> Vec<Result<StreamEvent, StreamError>> {
        self.buffer.push_str(&String::from_utf8_lossy(&chunk));

        let mut events = Vec::new();

        // Split by lines
        while let Some(line_end) = self.buffer.find('\n') {
            let line = self.buffer[..line_end].trim_end_matches('\r').to_string();
            self.buffer.drain(..=line_end);

            if line.is_empty() {
                // Empty line indicates event end
                if let Some(sse_event) = self.current_event.take() {
                    events.extend(self.parse_sse_event(sse_event));
                }
            } else if let Some(data) = line.strip_prefix("data: ") {
                // Data line
                let event = self.current_event.get_or_insert_with(|| SseEvent {
                    event_type: None,
                    data: String::new(),
                });
                if !event.data.is_empty() {
                    event.data.push('\n');
                }
                event.data.push_str(data);
            } else if let Some(event_type) = line.strip_prefix("event: ") {
                // Event type line
                let event = self.current_event.get_or_insert_with(|| SseEvent {
                    event_type: None,
                    data: String::new(),
                });
                event.event_type = Some(event_type.to_string());
            }
            // Ignore other lines (id:, retry:, etc.)
        }

        events
    }

    fn parse_sse_event(&mut self, sse_event: SseEvent) -> Vec<Result<StreamEvent, StreamError>> {
        let event = match serde_json::from_str::<AnthropicStreamEvent>(&sse_event.data) {
            Ok(value) => value,
            Err(err) => return vec![Err(StreamError::Parse(err.to_string()))],
        };

        event.to_stream_events(self)
    }
}

trait AnthropicEventExt {
    fn to_stream_events(&self, parser: &mut SseParser) -> Vec<Result<StreamEvent, StreamError>>;
}

impl AnthropicEventExt for AnthropicStreamEvent {
    fn to_stream_events(&self, parser: &mut SseParser) -> Vec<Result<StreamEvent, StreamError>> {
        match self {
            AnthropicStreamEvent::MessageStart { message } => {
                let mut events = Vec::new();
                if let Some(usage) = message.as_ref().and_then(|m| m.usage.as_ref())
                    && let Some(tokens) = usage.input_tokens
                {
                    events.push(Ok(StreamEvent::usage_update(Some(tokens), None, None)));
                }
                events
            }
            AnthropicStreamEvent::MessageStop
            | AnthropicStreamEvent::Ping
            | AnthropicStreamEvent::Unknown => Vec::new(),
            AnthropicStreamEvent::ContentBlockStart {
                index,
                content_block,
            } => match content_block {
                AnthropicStreamContentBlock::ToolUse { id, name } => {
                    parser.content_blocks.insert(
                        *index,
                        ContentBlockState {
                            block_type: "tool_use".to_string(),
                            tool_call_id: Some(id.clone()),
                            tool_call_name: Some(name.clone()),
                        },
                    );
                    vec![Ok(StreamEvent::tool_call_start(
                        id.clone(),
                        name.clone(),
                        *index,
                    ))]
                }
                AnthropicStreamContentBlock::Text { .. } => {
                    parser.content_blocks.insert(
                        *index,
                        ContentBlockState {
                            block_type: "text".to_string(),
                            tool_call_id: None,
                            tool_call_name: None,
                        },
                    );
                    Vec::new()
                }
                AnthropicStreamContentBlock::Thinking {
                    thinking,
                    signature,
                } => {
                    parser.content_blocks.insert(
                        *index,
                        ContentBlockState {
                            block_type: "thinking".to_string(),
                            tool_call_id: None,
                            tool_call_name: None,
                        },
                    );
                    let mut events = Vec::new();
                    if let Some(text) = thinking
                        && !text.trim().is_empty()
                    {
                        events.push(Ok(StreamEvent::thinking_delta(text.clone())));
                    }
                    if let Some(sig) = signature
                        && !sig.trim().is_empty()
                    {
                        events.push(Ok(StreamEvent::signature_delta(sig.clone())));
                    }
                    events
                }
                AnthropicStreamContentBlock::Unknown => Vec::new(),
            },
            AnthropicStreamEvent::ContentBlockDelta { index, delta } => match delta {
                AnthropicStreamContentDelta::TextDelta { text } => {
                    vec![Ok(StreamEvent::text_delta(text.clone(), *index))]
                }
                AnthropicStreamContentDelta::InputJsonDelta { partial_json } => {
                    if let Some(state) = parser.content_blocks.get(index)
                        && let Some(id) = &state.tool_call_id
                    {
                        return vec![Ok(StreamEvent::tool_call_arguments_delta(
                            id.clone(),
                            partial_json.clone(),
                            *index,
                        ))];
                    }
                    Vec::new()
                }
                AnthropicStreamContentDelta::ThinkingDelta { thinking } => {
                    vec![Ok(StreamEvent::thinking_delta(thinking.clone()))]
                }
                AnthropicStreamContentDelta::SignatureDelta { signature } => {
                    vec![Ok(StreamEvent::signature_delta(signature.clone()))]
                }
                AnthropicStreamContentDelta::Unknown => Vec::new(),
            },
            AnthropicStreamEvent::ContentBlockStop { index } => {
                if let Some(state) = parser.content_blocks.get(index)
                    && let Some(id) = &state.tool_call_id
                {
                    return vec![Ok(StreamEvent::tool_call_end(id.clone(), *index))];
                }
                Vec::new()
            }
            AnthropicStreamEvent::MessageDelta { delta, usage } => {
                let mut events = Vec::new();
                if let Some(delta) = delta
                    && let Some(reason) = delta.stop_reason.as_deref()
                {
                    events.push(Ok(StreamEvent::finish_reason_update(reason)));
                }
                if let Some(usage) = usage
                    && let Some(tokens) = usage.output_tokens
                {
                    events.push(Ok(StreamEvent::usage_update(None, Some(tokens), None)));
                }
                events
            }
            AnthropicStreamEvent::Error { error } => {
                let message = error
                    .as_ref()
                    .and_then(|detail| detail.message.clone())
                    .unwrap_or_else(|| "Unknown error".to_string());
                vec![Err(StreamError::Provider(message))]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_sse_chunk(lines: Vec<&str>) -> Bytes {
        Bytes::from(lines.join("\n") + "\n")
    }

    #[test]
    fn sse_parser_parses_text_delta() {
        let mut parser = SseParser::new();

        let chunk = create_sse_chunk(vec![
            "event: content_block_delta",
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
            "",
        ]);

        let events = parser.parse_chunk(chunk);
        assert_eq!(events.len(), 1);

        match &events[0] {
            Ok(StreamEvent::TextDelta { content, index }) => {
                assert_eq!(content, "Hello");
                assert_eq!(*index, 0);
            }
            _ => panic!("Expected TextDelta event"),
        }
    }

    #[test]
    fn sse_parser_parses_tool_call_start() {
        let mut parser = SseParser::new();

        let chunk = create_sse_chunk(vec![
            "event: content_block_start",
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_123","name":"read_file"}}"#,
            "",
        ]);

        let events = parser.parse_chunk(chunk);
        assert_eq!(events.len(), 1);

        match &events[0] {
            Ok(StreamEvent::ToolCallStart { id, name, index }) => {
                assert_eq!(id, "toolu_123");
                assert_eq!(name, "read_file");
                assert_eq!(*index, 0);
            }
            _ => panic!("Expected ToolCallStart event"),
        }
    }

    #[test]
    fn sse_parser_parses_tool_call_arguments() {
        let mut parser = SseParser::new();

        // 先发送 tool call start
        let chunk1 = create_sse_chunk(vec![
            "event: content_block_start",
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_123","name":"read_file"}}"#,
            "",
        ]);
        parser.parse_chunk(chunk1);

        // 再发送参数增量
        let chunk2 = create_sse_chunk(vec![
            "event: content_block_delta",
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"test.txt\"}"}}"#,
            "",
        ]);

        let events = parser.parse_chunk(chunk2);
        assert_eq!(events.len(), 1);

        match &events[0] {
            Ok(StreamEvent::ToolCallArgumentsDelta {
                id,
                arguments_json,
                index,
            }) => {
                assert_eq!(id, "toolu_123");
                assert_eq!(arguments_json, r#"{"path":"test.txt"}"#);
                assert_eq!(*index, 0);
            }
            _ => panic!("Expected ToolCallArgumentsDelta event"),
        }
    }

    #[test]
    fn sse_parser_parses_usage_update() {
        let mut parser = SseParser::new();

        let chunk = create_sse_chunk(vec![
            "event: message_delta",
            r#"data: {"type":"message_delta","delta":{},"usage":{"output_tokens":42}}"#,
            "",
        ]);

        let events = parser.parse_chunk(chunk);
        assert_eq!(events.len(), 1);

        match &events[0] {
            Ok(StreamEvent::UsageUpdate {
                input_tokens,
                output_tokens,
                total_tokens,
            }) => {
                assert_eq!(*input_tokens, None);
                assert_eq!(*output_tokens, Some(42));
                assert_eq!(*total_tokens, None);
            }
            _ => panic!("Expected UsageUpdate event"),
        }
    }

    #[test]
    fn sse_parser_parses_finish_reason() {
        let mut parser = SseParser::new();

        let chunk = create_sse_chunk(vec![
            "event: message_delta",
            r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{}}"#,
            "",
        ]);

        let events = parser.parse_chunk(chunk);
        assert_eq!(events.len(), 1);

        match &events[0] {
            Ok(StreamEvent::FinishReasonUpdate { reason }) => {
                assert_eq!(reason, "end_turn");
            }
            _ => panic!("Expected FinishReasonUpdate event"),
        }
    }

    #[test]
    fn sse_parser_handles_error_event() {
        let mut parser = SseParser::new();

        let chunk = create_sse_chunk(vec![
            "event: error",
            r#"data: {"type":"error","error":{"type":"rate_limit_error","message":"Rate limit exceeded"}}"#,
            "",
        ]);

        let events = parser.parse_chunk(chunk);
        assert_eq!(events.len(), 1);

        match &events[0] {
            Err(StreamError::Provider(msg)) => {
                assert_eq!(msg, "Rate limit exceeded");
            }
            _ => panic!("Expected Provider error"),
        }
    }

    #[test]
    fn sse_parser_handles_multiline_data() {
        let mut parser = SseParser::new();

        let chunk = create_sse_chunk(vec![
            "event: content_block_delta",
            r#"data: {"type":"content_block_delta","#,
            r#"data: "index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
            "",
        ]);

        let events = parser.parse_chunk(chunk);
        assert_eq!(events.len(), 1);

        match &events[0] {
            Ok(StreamEvent::TextDelta { content, .. }) => {
                assert_eq!(content, "Hello");
            }
            _ => panic!("Expected TextDelta event"),
        }
    }

    #[test]
    fn sse_parser_ignores_ping_and_message_stop() {
        let mut parser = SseParser::new();

        let chunk = create_sse_chunk(vec![
            "event: ping",
            r#"data: {"type":"ping"}"#,
            "",
            "event: message_stop",
            r#"data: {"type":"message_stop"}"#,
            "",
        ]);

        let events = parser.parse_chunk(chunk);
        assert_eq!(events.len(), 0);
    }
}
