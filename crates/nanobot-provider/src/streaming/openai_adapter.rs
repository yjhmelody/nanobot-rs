use std::collections::HashMap;

use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;

use super::adapter::StreamAdapter;
use super::events::{StreamError, StreamEvent, StreamResponse};
use crate::openai_types::{
    OpenAIResponsesResponse, ResponsesOutputItem, ResponsesStreamEvent,
};

/// OpenAI Responses streaming adapter.
///
/// Spec sources:
/// - https://platform.openai.com/docs/guides/streaming-responses
/// - https://platform.openai.com/docs/api-reference/responses-streaming/response/output_text/delta
pub struct OpenAiAdapter;

#[async_trait]
impl StreamAdapter for OpenAiAdapter {
    async fn adapt_stream(
        &self,
        response: reqwest::Response,
    ) -> Result<StreamResponse, StreamError> {
        let bytes_stream = response.bytes_stream();

        let event_stream = bytes_stream
            .map(|chunk_result| chunk_result.map_err(|e| StreamError::Network(e.to_string())))
            .scan(OpenAiParser::new(), |parser, chunk_result| {
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

/// OpenAI SSE parser.
struct OpenAiParser {
    buffer: String,
    state: ResponsesStreamState,
}

impl OpenAiParser {
    fn new() -> Self {
        Self {
            buffer: String::new(),
            state: ResponsesStreamState::new(),
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
                continue;
            }

            // Check for [DONE] marker (legacy; Responses streaming typically omits this)
            if line == "data: [DONE]" {
                continue;
            }

            // Parse data line
            if let Some(data) = line.strip_prefix("data: ") {
                events.extend(self.parse_data_line(data));
            }
        }

        events
    }

    fn parse_data_line(&mut self, data: &str) -> Vec<Result<StreamEvent, StreamError>> {
        let event = match serde_json::from_str::<ResponsesStreamEvent>(data) {
            Ok(value) => value,
            Err(err) => return vec![Err(StreamError::Parse(err.to_string()))],
        };

        event.to_stream_events(&mut self.state)
    }
}

struct ResponsesStreamState {
    text_block_indices: HashMap<(usize, usize), usize>,
    next_text_block_index: usize,
    tool_calls_by_item_id: HashMap<String, ToolCallState>,
}

struct ToolCallState {
    id: String,
    name: Option<String>,
    output_index: usize,
    started: bool,
}

impl ResponsesStreamState {
    fn new() -> Self {
        Self {
            text_block_indices: HashMap::new(),
            next_text_block_index: 0,
            tool_calls_by_item_id: HashMap::new(),
        }
    }

    fn text_block_index(&mut self, output_index: usize, content_index: usize) -> usize {
        let key = (output_index, content_index);
        if let Some(index) = self.text_block_indices.get(&key) {
            return *index;
        }

        let index = self.next_text_block_index;
        self.next_text_block_index += 1;
        self.text_block_indices.insert(key, index);
        index
    }

    fn remember_tool_call(
        &mut self,
        item_id: Option<&str>,
        call_id: Option<&str>,
        name: Option<&str>,
        output_index: usize,
    ) -> String {
        let fallback_id = item_id.unwrap_or_else(|| "unknown_call");
        let id = call_id.unwrap_or(fallback_id).to_string();

        if let Some(item_id) = item_id {
            use std::collections::hash_map::Entry;
            match self.tool_calls_by_item_id.entry(item_id.to_string()) {
                Entry::Occupied(mut entry) => {
                    let state = entry.get_mut();
                    if state.name.is_none() {
                        state.name = name.map(|value| value.to_string());
                    }
                    state.output_index = output_index;
                }
                Entry::Vacant(entry) => {
                    entry.insert(ToolCallState {
                        id: id.clone(),
                        name: name.map(|value| value.to_string()),
                        output_index,
                        started: false,
                    });
                }
            }
        }

        id
    }

    fn tool_call_id_for(&mut self, item_id: &str, output_index: usize) -> String {
        if let Some(state) = self.tool_calls_by_item_id.get(item_id) {
            return state.id.clone();
        }

        let id = item_id.to_string();
        self.tool_calls_by_item_id.insert(
            item_id.to_string(),
            ToolCallState {
                id: id.clone(),
                name: None,
                output_index,
                started: false,
            },
        );
        id
    }

    fn mark_tool_call_started(&mut self, item_id: Option<&str>) -> bool {
        let Some(item_id) = item_id else {
            return true;
        };
        if let Some(state) = self.tool_calls_by_item_id.get_mut(item_id) {
            if state.started {
                return false;
            }
            state.started = true;
            return true;
        }
        false
    }
}

trait ResponsesEventExt {
    fn to_stream_events(
        &self,
        state: &mut ResponsesStreamState,
    ) -> Vec<Result<StreamEvent, StreamError>>;
}

impl ResponsesEventExt for ResponsesStreamEvent {
    fn to_stream_events(
        &self,
        state: &mut ResponsesStreamState,
    ) -> Vec<Result<StreamEvent, StreamError>> {
        match self {
            ResponsesStreamEvent::OutputTextDelta(event) => {
                let index = state.text_block_index(event.output_index, event.content_index);
                vec![Ok(StreamEvent::text_delta(event.delta.clone(), index))]
            }
            ResponsesStreamEvent::OutputTextDone(_event) => Vec::new(),
            ResponsesStreamEvent::ReasoningTextDelta(event) => {
                vec![Ok(StreamEvent::thinking_delta(event.delta.clone()))]
            }
            ResponsesStreamEvent::ReasoningTextDone(_event) => Vec::new(),
            ResponsesStreamEvent::FunctionCallArgumentsDelta(event) => {
                let id = state.tool_call_id_for(&event.item_id, event.output_index);
                vec![Ok(StreamEvent::tool_call_arguments_delta(
                    id,
                    event.delta.clone(),
                    event.output_index,
                ))]
            }
            ResponsesStreamEvent::FunctionCallArgumentsDone(event) => {
                let mut events = Vec::new();
                if let Some(item) = &event.item {
                    events.extend(tool_call_events_from_item(
                        state,
                        item,
                        event.output_index,
                        true,
                    ));
                } else if let Some(arguments) = &event.arguments {
                    if let Some(item_id) = event.item_id.as_deref() {
                        let id = state.tool_call_id_for(item_id, event.output_index);
                        events.push(Ok(StreamEvent::tool_call_arguments_delta(
                            id.clone(),
                            arguments.clone(),
                            event.output_index,
                        )));
                        events.push(Ok(StreamEvent::tool_call_end(id, event.output_index)));
                    }
                }
                events
            }
            ResponsesStreamEvent::OutputItemAdded(event) => {
                tool_call_events_from_item(state, &event.item, event.output_index, false)
            }
            ResponsesStreamEvent::OutputItemDone(event) => {
                tool_call_events_from_item(state, &event.item, event.output_index, true)
            }
            ResponsesStreamEvent::ResponseCompleted(event) => {
                if let Some(response) = &event.response {
                    if let Some(update) = usage_update_from_response(response) {
                        return vec![Ok(update)];
                    }
                }
                Vec::new()
            }
            ResponsesStreamEvent::Error(event) => {
                let message = event
                    .error
                    .as_ref()
                    .and_then(|err| err.message.clone())
                    .or_else(|| event.message.clone())
                    .unwrap_or_else(|| "Unknown error".to_string());
                vec![Err(StreamError::Provider(message))]
            }
            ResponsesStreamEvent::Unknown => Vec::new(),
        }
    }
}

fn usage_update_from_response(response: &OpenAIResponsesResponse) -> Option<StreamEvent> {
    let usage = response.usage.as_ref()?;
    let input_tokens = usage.input_tokens.map(|v| v as i32);
    let output_tokens = usage.output_tokens.map(|v| v as i32);
    let total_tokens = usage.total_tokens.map(|v| v as i32);
    Some(StreamEvent::usage_update(
        input_tokens,
        output_tokens,
        total_tokens,
    ))
}

fn tool_call_events_from_item(
    state: &mut ResponsesStreamState,
    item: &ResponsesOutputItem,
    output_index: usize,
    emit_end: bool,
) -> Vec<Result<StreamEvent, StreamError>> {
    match item {
        ResponsesOutputItem::FunctionCall {
            id,
            call_id,
            name,
            arguments,
            ..
        } => {
            let mut events = Vec::new();
            let call_id_str = call_id.as_deref().or_else(|| id.as_deref());
            let selected_id =
                state.remember_tool_call(id.as_deref(), call_id_str, name.as_deref(), output_index);

            if let Some(name) = name {
                if state.mark_tool_call_started(id.as_deref()) {
                    events.push(Ok(StreamEvent::tool_call_start(
                        selected_id.clone(),
                        name.clone(),
                        output_index,
                    )));
                }
            }

            if let Some(arguments) = arguments {
                if !arguments.is_empty() {
                    events.push(Ok(StreamEvent::tool_call_arguments_delta(
                        selected_id.clone(),
                        arguments.clone(),
                        output_index,
                    )));
                }
            }

            if emit_end {
                events.push(Ok(StreamEvent::tool_call_end(selected_id, output_index)));
            }

            events
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_chunk(data: &str) -> Bytes {
        Bytes::from(format!("data: {}\n\n", data))
    }

    #[test]
    fn openai_parser_parses_text_delta() {
        let mut parser = OpenAiParser::new();

        let chunk = create_chunk(
            r#"{"type":"response.output_text.delta","item_id":"item_1","output_index":0,"content_index":0,"delta":"Hello"}"#,
        );

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
    fn openai_parser_parses_tool_call_start() {
        let mut parser = OpenAiParser::new();

        let chunk = create_chunk(
            r#"{"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"item_123","call_id":"call_123","name":"read_file"}}"#,
        );

        let events = parser.parse_chunk(chunk);
        assert_eq!(events.len(), 1);

        match &events[0] {
            Ok(StreamEvent::ToolCallStart { id, name, index }) => {
                assert_eq!(id, "call_123");
                assert_eq!(name, "read_file");
                assert_eq!(*index, 0);
            }
            _ => panic!("Expected ToolCallStart event"),
        }
    }

    #[test]
    fn openai_parser_parses_tool_call_arguments() {
        let mut parser = OpenAiParser::new();

        let chunk = create_chunk(
            r#"{"type":"response.function_call_arguments.delta","item_id":"item_123","output_index":0,"delta":"{\"path\":"}"#,
        );

        let events = parser.parse_chunk(chunk);
        assert_eq!(events.len(), 1);

        match &events[0] {
            Ok(StreamEvent::ToolCallArgumentsDelta {
                id,
                arguments_json,
                index,
            }) => {
                assert_eq!(id, "item_123");
                assert_eq!(arguments_json, r#"{"path":"#);
                assert_eq!(*index, 0);
            }
            _ => panic!("Expected ToolCallArgumentsDelta event"),
        }
    }

    #[test]
    fn openai_parser_parses_usage() {
        let mut parser = OpenAiParser::new();

        let chunk = create_chunk(
            r#"{"type":"response.completed","response":{"output":[],"usage":{"input_tokens":10,"output_tokens":20,"total_tokens":30}}}"#,
        );

        let events = parser.parse_chunk(chunk);
        assert_eq!(events.len(), 1);

        match &events[0] {
            Ok(StreamEvent::UsageUpdate {
                input_tokens,
                output_tokens,
                total_tokens,
            }) => {
                assert_eq!(*input_tokens, Some(10));
                assert_eq!(*output_tokens, Some(20));
                assert_eq!(*total_tokens, Some(30));
            }
            _ => panic!("Expected UsageUpdate event"),
        }
    }

    #[test]
    fn openai_parser_handles_done_marker() {
        let mut parser = OpenAiParser::new();

        let chunk = Bytes::from("data: [DONE]\n\n");

        let events = parser.parse_chunk(chunk);
        assert_eq!(events.len(), 0);
    }

    #[test]
    fn openai_parser_handles_error() {
        let mut parser = OpenAiParser::new();

        let chunk = create_chunk(r#"{"type":"error","error":{"message":"Rate limit exceeded"}}"#);

        let events = parser.parse_chunk(chunk);
        assert_eq!(events.len(), 1);

        match &events[0] {
            Err(StreamError::Provider(msg)) => {
                assert_eq!(msg, "Rate limit exceeded");
            }
            _ => panic!("Expected Provider error"),
        }
    }
}
