use serde::{Deserialize, Serialize};

use nanobot_types::tools::{JsonSchema, ToolDefinition};

/// Anthropic messages API request payload.
///
/// Spec source: https://docs.anthropic.com/en/docs/api/messages
#[derive(Debug, Clone, Serialize)]
pub(crate) struct AnthropicMessagesPayload {
    /// Model identifier.
    pub(crate) model: String,
    /// Optional system prompt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) system: Option<String>,
    /// Conversation messages.
    pub(crate) messages: Vec<AnthropicInputMessage>,
    /// Maximum output tokens.
    pub(crate) max_tokens: i32,
    /// Sampling temperature.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) temperature: Option<f32>,
    /// Optional tool definitions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tools: Option<Vec<AnthropicToolDefinition>>,
    /// Enable streaming responses.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) stream: Option<bool>,
}

/// Input message structure for Anthropic requests.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct AnthropicInputMessage {
    /// Role string ("user" or "assistant").
    pub(crate) role: &'static str,
    /// Content blocks for the message.
    pub(crate) content: Vec<AnthropicInputContentBlock>,
}

impl AnthropicInputMessage {
    pub(crate) fn new(role: &'static str, content: Vec<AnthropicInputContentBlock>) -> Self {
        Self { role, content }
    }
}

/// Content blocks accepted by Anthropic input messages.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum AnthropicInputContentBlock {
    Text {
        /// Text content.
        text: String,
    },
    ToolUse {
        /// Tool call id.
        id: String,
        /// Tool name.
        name: String,
        /// Tool input payload.
        input: serde_json::Value,
    },
    ToolResult {
        /// Tool call id being responded to.
        tool_use_id: String,
        /// Tool output content.
        content: String,
        /// Whether the tool result represents an error.
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
}

/// Tool definition mapping for Anthropic.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct AnthropicToolDefinition {
    /// Tool/function name.
    pub(crate) name: String,
    /// Tool/function description.
    pub(crate) description: String,
    /// JSON schema for tool input.
    pub(crate) input_schema: JsonSchema,
}

impl From<ToolDefinition> for AnthropicToolDefinition {
    fn from(value: ToolDefinition) -> Self {
        Self {
            name: value.function.name,
            description: value.function.description,
            input_schema: value.function.parameters,
        }
    }
}

/// Response content block from Anthropic API.
///
/// Spec source: https://docs.anthropic.com/en/docs/api/messages
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum AnthropicContentBlock {
    Text {
        /// Text content.
        text: String,
    },
    ToolUse {
        /// Tool call id.
        id: String,
        /// Tool name.
        name: String,
        /// Tool input payload.
        input: serde_json::Value,
    },
    Thinking {
        /// Thinking content.
        #[serde(alias = "text")]
        thinking: String,
        /// Signature, when provided.
        #[serde(skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
}

/// Anthropic messages API response payload.
///
/// Spec source: https://docs.anthropic.com/en/docs/api/messages
#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct AnthropicMessagesResponse {
    /// Output content blocks.
    #[serde(default)]
    pub(crate) content: Vec<AnthropicContentBlock>,
    /// Stop reason.
    #[serde(default)]
    pub(crate) stop_reason: Option<String>,
    /// Token usage metadata.
    #[serde(default)]
    pub(crate) usage: Option<AnthropicUsage>,
}

/// Token usage metadata from Anthropic responses.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct AnthropicUsage {
    /// Input token count.
    #[serde(default)]
    pub(crate) input_tokens: Option<u64>,
    /// Output token count.
    #[serde(default)]
    pub(crate) output_tokens: Option<u64>,
    /// Cache creation input tokens, when provided.
    #[serde(default)]
    pub(crate) cache_creation_input_tokens: Option<u64>,
    /// Cache read input tokens, when provided.
    #[serde(default)]
    pub(crate) cache_read_input_tokens: Option<u64>,
}

/// Error wrapper returned by Anthropic on failure.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct AnthropicErrorResponse {
    /// Error detail payload.
    #[serde(default)]
    pub(crate) error: Option<AnthropicErrorDetail>,
}

/// Detailed error information from Anthropic.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct AnthropicErrorDetail {
    /// Error message.
    #[serde(default)]
    pub(crate) message: Option<String>,
}

// Anthropic streaming SSE event types.
// Spec source:
// https://docs.anthropic.com/en/docs/build-with-claude/streaming
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum AnthropicStreamEvent {
    MessageStart {
        /// Initial message metadata payload, when provided.
        #[serde(default)]
        message: Option<AnthropicStreamMessage>,
    },
    ContentBlockStart {
        /// Index of the content block.
        index: usize,
        /// Content block metadata.
        content_block: AnthropicStreamContentBlock,
    },
    ContentBlockDelta {
        /// Index of the content block.
        index: usize,
        /// Delta content payload.
        delta: AnthropicStreamContentDelta,
    },
    ContentBlockStop {
        /// Index of the content block.
        index: usize,
    },
    MessageDelta {
        /// Message-level metadata deltas.
        #[serde(default)]
        delta: Option<AnthropicStreamMessageDelta>,
        /// Usage updates for the message.
        #[serde(default)]
        usage: Option<AnthropicStreamUsage>,
    },
    MessageStop,
    Ping,
    Error {
        /// Error detail payload, when provided.
        #[serde(default)]
        error: Option<AnthropicErrorDetail>,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub(crate) struct AnthropicStreamMessage {
    /// Message id.
    #[serde(default)]
    pub(crate) id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum AnthropicStreamContentBlock {
    Text {
        /// Initial text content, when provided.
        #[serde(default)]
        text: Option<String>,
    },
    ToolUse {
        /// Tool call id.
        id: String,
        /// Tool name.
        name: String,
    },
    Thinking {
        /// Initial thinking content, when provided.
        #[serde(default)]
        thinking: Option<String>,
        /// Signature string, when provided.
        #[serde(default)]
        signature: Option<String>,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum AnthropicStreamContentDelta {
    TextDelta {
        /// Delta text content.
        text: String,
    },
    InputJsonDelta {
        /// Delta JSON arguments string for tool input.
        partial_json: String,
    },
    ThinkingDelta {
        /// Delta thinking content.
        thinking: String,
    },
    SignatureDelta {
        /// Delta signature content.
        signature: String,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct AnthropicStreamMessageDelta {
    /// Stop reason for the message, when provided.
    #[serde(default)]
    pub(crate) stop_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct AnthropicStreamUsage {
    /// Output token count, when provided.
    #[serde(default)]
    pub(crate) output_tokens: Option<i32>,
}
