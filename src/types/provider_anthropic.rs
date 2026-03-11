use serde::{Deserialize, Serialize};

use crate::types::tools::{JsonSchema, ToolDefinition};

/// Anthropic messages API request payload.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct AnthropicMessagesPayload {
    pub(crate) model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) system: Option<String>,
    pub(crate) messages: Vec<AnthropicInputMessage>,
    pub(crate) max_tokens: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tools: Option<Vec<AnthropicToolDefinition>>,
}

/// Input message structure for Anthropic requests.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct AnthropicInputMessage {
    pub(crate) role: &'static str,
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
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
}

/// Tool definition mapping for Anthropic.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct AnthropicToolDefinition {
    pub(crate) name: String,
    pub(crate) description: String,
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
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum AnthropicContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    Thinking {
        #[serde(alias = "text")]
        thinking: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
}

/// Anthropic messages API response payload.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct AnthropicMessagesResponse {
    #[serde(default)]
    pub(crate) content: Vec<AnthropicContentBlock>,
    #[serde(default)]
    pub(crate) stop_reason: Option<String>,
    #[serde(default)]
    pub(crate) usage: Option<AnthropicUsage>,
}

/// Token usage metadata from Anthropic responses.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct AnthropicUsage {
    #[serde(default)]
    pub(crate) input_tokens: Option<u64>,
    #[serde(default)]
    pub(crate) output_tokens: Option<u64>,
    #[serde(default)]
    pub(crate) cache_creation_input_tokens: Option<u64>,
    #[serde(default)]
    pub(crate) cache_read_input_tokens: Option<u64>,
}

/// Error wrapper returned by Anthropic on failure.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct AnthropicErrorResponse {
    #[serde(default)]
    pub(crate) error: Option<AnthropicErrorDetail>,
}

/// Detailed error information from Anthropic.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct AnthropicErrorDetail {
    #[serde(default)]
    pub(crate) message: Option<String>,
}
