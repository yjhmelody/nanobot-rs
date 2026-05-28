use serde::{Deserialize, Serialize};

use crate::tool_name::ToolName;

/// Role of a chat message in the provider request/response.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

impl MessageRole {
    /// Returns the lowercase string name for this role as expected by provider APIs.
    pub fn role_name(&self) -> &'static str {
        match self {
            MessageRole::System => "system",
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::Tool => "tool",
        }
    }
}

/// Typed content parts for multi-part messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ContentPart {
    Text { text: String },
}

/// Message content as plain text or structured parts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Parts(Vec<ContentPart>),
}

impl MessageContent {
    /// Returns the text content if this is a `Text` variant, otherwise `None`.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(s) => Some(s),
            Self::Parts(_) => None,
        }
    }
}

/// Function call requested by the assistant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantFunctionCall {
    /// Function/tool name requested by the assistant.
    pub name: String,
    /// JSON-encoded arguments for the function call.
    pub arguments: String,
}

/// Tool call wrapper for function invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantToolCall {
    /// Provider-generated tool call identifier.
    pub id: String,
    #[serde(rename = "type")]
    /// Tool call type (typically "function").
    pub kind: String,
    /// Function call payload for the tool.
    pub function: AssistantFunctionCall,
}

/// Chat message payload used by providers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    /// Message role (system/user/assistant/tool).
    pub role: MessageRole,
    #[serde(default)]
    /// Optional message content (text or structured parts).
    pub content: Option<MessageContent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Tool calls attached to this message (assistant role).
    pub tool_calls: Option<Vec<AssistantToolCall>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Tool call id this tool message responds to.
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Optional tool name (used for tool responses).
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Optional reasoning content from providers that return it.
    pub reasoning_content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Optional structured thinking blocks for long-form reasoning.
    pub thinking_blocks: Option<Vec<ThinkingBlock>>,
}

impl ChatMessage {
    pub fn system_text(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::System,
            content: Some(MessageContent::Text(content.into())),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning_content: None,
            thinking_blocks: None,
        }
    }

    pub fn user_text(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            content: Some(MessageContent::Text(content.into())),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning_content: None,
            thinking_blocks: None,
        }
    }

    pub fn user_parts(parts: Vec<ContentPart>) -> Self {
        Self {
            role: MessageRole::User,
            content: Some(MessageContent::Parts(parts)),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning_content: None,
            thinking_blocks: None,
        }
    }

    pub fn assistant(
        content: Option<String>,
        tool_calls: Option<Vec<AssistantToolCall>>,
        reasoning_content: Option<String>,
        thinking_blocks: Option<Vec<ThinkingBlock>>,
    ) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: content.map(MessageContent::Text),
            tool_calls,
            tool_call_id: None,
            name: None,
            reasoning_content,
            thinking_blocks,
        }
    }

    pub fn tool_result(
        tool_call_id: impl Into<String>,
        name: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            role: MessageRole::Tool,
            content: Some(MessageContent::Text(content.into())),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
            name: Some(name.into()),
            reasoning_content: None,
            thinking_blocks: None,
        }
    }

    pub fn content_as_text(&self) -> Option<&str> {
        self.content.as_ref().and_then(|c| c.as_text())
    }
}

/// Tool call returned by providers in unified response format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRequest {
    /// Provider-generated tool call identifier.
    pub id: String,
    /// Tool name requested by the model.
    pub name: ToolName,
    /// JSON payload for the tool arguments.
    pub arguments_json: String,
}

/// A structured thinking block with optional signature (required by Anthropic extended thinking).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThinkingBlock {
    /// The thinking/reasoning text content.
    pub thinking: String,
    /// Signature for this thinking block, required by Anthropic extended thinking API
    /// to be passed back on subsequent requests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

impl ThinkingBlock {
    /// Creates a new thinking block with text content only (no signature).
    pub fn new(thinking: impl Into<String>) -> Self {
        Self {
            thinking: thinking.into(),
            signature: None,
        }
    }

    /// Creates a new thinking block with both text and an optional signature.
    pub fn with_signature(thinking: impl Into<String>, signature: Option<String>) -> Self {
        Self {
            thinking: thinking.into(),
            signature,
        }
    }
}

impl From<String> for ThinkingBlock {
    fn from(thinking: String) -> Self {
        Self::new(thinking)
    }
}

impl From<&str> for ThinkingBlock {
    fn from(thinking: &str) -> Self {
        Self::new(thinking.to_string())
    }
}

/// Generic reasoning/thinking configuration for LLM providers.
///
/// Supports both Anthropic-style thinking (type/budget_tokens) and
/// OpenAI-compatible reasoning effort in a single provider-agnostic struct.
///
/// # Anthropic (thinking)
/// - `type: "adaptive"` — model decides when to think (Claude 4.6+)
/// - `type: "enabled"` + `budget_tokens: N` — fixed thinking budget (Claude 3.7/4.0-4.5)
/// - omitted — no extended thinking
///
/// # OpenAI-compatible (reasoning_effort)
/// - `effort: "low" | "medium" | "high" | "xhigh"` — reasoning depth
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct ReasoningConfig {
    /// Thinking type for Anthropic extended thinking.
    /// Supported: "adaptive" (4.6+), "enabled" (3.7/4.0-4.5).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    /// Token budget for thinking (used when type="enabled" for Claude 3.7/4.0-4.5).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget_tokens: Option<i32>,
    /// Reasoning effort for OpenAI-compatible providers.
    /// Supported: "low", "medium", "high", "xhigh".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

impl ReasoningConfig {
    /// Returns the effort value for OpenAI-compatible providers.
    pub fn effort(&self) -> Option<&str> {
        self.effort.as_deref().filter(|s| !s.trim().is_empty())
    }

    /// Returns the thinking type for Anthropic.
    pub fn thinking_type(&self) -> Option<&str> {
        self.r#type.as_deref().filter(|s| !s.trim().is_empty())
    }
}

/// Token usage statistics from provider responses.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageStats {
    /// Tokens used by the prompt/request.
    pub prompt_tokens: Option<u64>,
    /// Tokens generated in the completion/response.
    pub completion_tokens: Option<u64>,
    /// Total tokens across prompt and completion.
    pub total_tokens: Option<u64>,
}

impl UsageStats {
    /// Formats a human-readable summary of the usage statistics.
    pub fn format_summary(&self) -> Option<String> {
        let mut parts = Vec::new();
        if let Some(v) = self.prompt_tokens {
            parts.push(format!("prompt={}", v));
        }
        if let Some(v) = self.completion_tokens {
            parts.push(format!("completion={}", v));
        }
        if let Some(v) = self.total_tokens {
            parts.push(format!("total={}", v));
        }
        if parts.is_empty() {
            None
        } else {
            Some(format!("Usage: {}", parts.join(", ")))
        }
    }
}

/// Unified LLM response with optional tool calls.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LLMResponse {
    /// Optional assistant text content.
    pub content: Option<String>,
    #[serde(default)]
    /// Tool calls requested by the model.
    pub tool_calls: Vec<ToolCallRequest>,
    /// Provider finish reason string.
    pub finish_reason: String,
    #[serde(default)]
    /// Usage statistics returned by the provider.
    pub usage: UsageStats,
    #[serde(default)]
    /// Optional reasoning content from providers that return it.
    pub reasoning_content: Option<String>,
    #[serde(default)]
    /// Optional structured thinking blocks for long-form reasoning.
    pub thinking_blocks: Option<Vec<ThinkingBlock>>,
}

impl LLMResponse {
    /// Returns `true` if the model requested one or more tool calls.
    pub fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }
}
