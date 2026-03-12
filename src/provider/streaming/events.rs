use std::pin::Pin;

use futures::Stream;

use crate::provider::LLMResponse;

/// Unified streaming event types for LLM responses.
///
/// These events represent incremental updates from streaming LLM providers,
/// allowing real-time display of text generation, tool calls, and metadata.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// Text content delta.
    TextDelta {
        content: String,
        /// Content block index (for multiple content blocks).
        index: usize,
    },

    /// Thinking/reasoning content delta.
    ThinkingDelta { content: String },

    /// Tool call started.
    ToolCallStart {
        id: String,
        name: String,
        index: usize,
    },

    /// Tool call arguments delta.
    ToolCallArgumentsDelta {
        id: String,
        arguments_json: String,
        index: usize,
    },

    /// Tool call ended.
    ToolCallEnd { id: String, index: usize },

    /// Usage statistics update.
    UsageUpdate {
        input_tokens: Option<i32>,
        output_tokens: Option<i32>,
        total_tokens: Option<i32>,
    },

    /// Finish reason update.
    FinishReasonUpdate { reason: String },

    /// Stream completed with full accumulated response.
    Done { response: LLMResponse },

    /// Error event.
    Error { message: String },
}

/// Unified streaming response type.
pub type StreamResponse = Pin<Box<dyn Stream<Item = Result<StreamEvent, StreamError>> + Send>>;

/// Streaming error types.
#[derive(Debug, thiserror::Error)]
pub enum StreamError {
    #[error("Network error: {0}")]
    Network(String),

    #[error("Parse error: {0}")]
    Parse(String),

    #[error("Provider error: {0}")]
    Provider(String),

    #[error("Stream interrupted")]
    Interrupted,
}

impl From<crate::error::ProviderError> for StreamError {
    fn from(err: crate::error::ProviderError) -> Self {
        use crate::error::ProviderError;
        match err {
            ProviderError::ApiRequest(e) => {
                if e.is_timeout() {
                    StreamError::Network(format!("Request timeout: {}", e))
                } else if e.is_connect() {
                    StreamError::Network(format!("Connection failed: {}", e))
                } else {
                    StreamError::Network(e.to_string())
                }
            }
            ProviderError::Timeout(secs) => {
                StreamError::Network(format!("Request timeout after {}s", secs))
            }
            ProviderError::RateLimit(msg) => StreamError::Provider(format!("Rate limit: {}", msg)),
            ProviderError::Authentication(msg) => {
                StreamError::Provider(format!("Authentication failed: {}", msg))
            }
            ProviderError::InvalidResponse(msg) => StreamError::Parse(msg),
            ProviderError::ModelNotAvailable(msg) => {
                StreamError::Provider(format!("Model not available: {}", msg))
            }
            ProviderError::InvalidConfig(msg) => {
                StreamError::Provider(format!("Invalid config: {}", msg))
            }
            ProviderError::Other(msg) => StreamError::Provider(msg),
        }
    }
}

impl StreamEvent {
    /// Creates a text delta event.
    pub fn text_delta(content: impl Into<String>, index: usize) -> Self {
        Self::TextDelta {
            content: content.into(),
            index,
        }
    }

    /// Creates a thinking content delta event.
    pub fn thinking_delta(content: impl Into<String>) -> Self {
        Self::ThinkingDelta {
            content: content.into(),
        }
    }

    /// Creates a tool call start event.
    pub fn tool_call_start(id: impl Into<String>, name: impl Into<String>, index: usize) -> Self {
        Self::ToolCallStart {
            id: id.into(),
            name: name.into(),
            index,
        }
    }

    /// Creates a tool call arguments delta event.
    pub fn tool_call_arguments_delta(
        id: impl Into<String>,
        arguments_json: impl Into<String>,
        index: usize,
    ) -> Self {
        Self::ToolCallArgumentsDelta {
            id: id.into(),
            arguments_json: arguments_json.into(),
            index,
        }
    }

    /// Creates a tool call end event.
    pub fn tool_call_end(id: impl Into<String>, index: usize) -> Self {
        Self::ToolCallEnd {
            id: id.into(),
            index,
        }
    }

    /// Creates a usage statistics update event.
    pub fn usage_update(
        input_tokens: Option<i32>,
        output_tokens: Option<i32>,
        total_tokens: Option<i32>,
    ) -> Self {
        Self::UsageUpdate {
            input_tokens,
            output_tokens,
            total_tokens,
        }
    }

    /// Creates a finish reason update event.
    pub fn finish_reason_update(reason: impl Into<String>) -> Self {
        Self::FinishReasonUpdate {
            reason: reason.into(),
        }
    }

    /// Creates a stream done event.
    pub fn done(response: LLMResponse) -> Self {
        Self::Done { response }
    }

    /// Creates an error event.
    pub fn error(message: impl Into<String>) -> Self {
        Self::Error {
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_event_constructors_work() {
        let event = StreamEvent::text_delta("Hello", 0);
        assert!(
            matches!(event, StreamEvent::TextDelta { content, index } if content == "Hello" && index == 0)
        );

        let event = StreamEvent::thinking_delta("Thinking...");
        assert!(
            matches!(event, StreamEvent::ThinkingDelta { content } if content == "Thinking...")
        );

        let event = StreamEvent::tool_call_start("call_1", "read_file", 0);
        assert!(matches!(
            event,
            StreamEvent::ToolCallStart { id, name, index }
            if id == "call_1" && name == "read_file" && index == 0
        ));

        let event = StreamEvent::usage_update(Some(10), Some(20), Some(30));
        assert!(matches!(
            event,
            StreamEvent::UsageUpdate {
                input_tokens: Some(10),
                output_tokens: Some(20),
                total_tokens: Some(30)
            }
        ));
    }

    #[test]
    fn stream_error_display() {
        let err = StreamError::Network("connection failed".to_string());
        assert_eq!(err.to_string(), "Network error: connection failed");

        let err = StreamError::Parse("invalid json".to_string());
        assert_eq!(err.to_string(), "Parse error: invalid json");

        let err = StreamError::Provider("rate limit".to_string());
        assert_eq!(err.to_string(), "Provider error: rate limit");

        let err = StreamError::Interrupted;
        assert_eq!(err.to_string(), "Stream interrupted");
    }
}
