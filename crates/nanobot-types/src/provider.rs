//! LLM provider message types and response structures.
//!
//! This module defines the data types used to communicate with LLM
//! providers (Anthropic, OpenAI, etc.). It covers:
//!
//! - [`ChatMessage`] — the core unit of conversation history, with role,
//!   content, tool calls, and thinking blocks.
//! - [`LLMResponse`] — a unified response from any provider, containing
//!   text content, tool call requests, usage statistics, and reasoning.
//! - [`ReasoningConfig`] — provider-agnostic configuration for extended
//!   thinking/reasoning.
//! - [`ThinkingBlock`] — structured reasoning steps (e.g., Anthropic
//!   extended thinking).
//!
//! # Design
//!
//! - All types are serialisable to/from JSON via `serde` for wire format
//!   compatibility with provider APIs.
//! - The `#[serde(rename_all = "camelCase")]` convention matches both
//!   Anthropic and OpenAI API conventions.
//! - [`ReasoningConfig`] unifies Anthropic-style thinking (type/budget_tokens)
//!   and OpenAI-compatible reasoning effort (effort) into a single struct,
//!   keeping provider-specific coupling out of the consuming code.

use serde::{Deserialize, Serialize};

use crate::tool_name::ToolName;

/// Role of a chat message in the provider request/response lifecycle.
///
/// Corresponds to the roles defined by standard LLM chat APIs.
///
/// | Variant | API role | Description |
/// |---------|----------|-------------|
/// | `System` | `"system"` | System prompt / instruction |
/// | `User` | `"user"` | Human input |
/// | `Assistant` | `"assistant"` | Model response |
/// | `Tool` | `"tool"` | Tool execution result |
///
/// # Derive rationale
///
/// - `Clone + Copy`: role is tiny and passed by value in message construction.
/// - `PartialEq + Eq`: used in role-based message filtering.
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
    ///
    /// # Examples
    ///
    /// ```
    /// use nanobot_types::provider::MessageRole;
    /// assert_eq!(MessageRole::User.role_name(), "user");
    /// assert_eq!(MessageRole::Assistant.role_name(), "assistant");
    /// ```
    pub fn role_name(&self) -> &'static str {
        match self {
            MessageRole::System => "system",
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::Tool => "tool",
        }
    }
}

/// A single part of a multi-part message.
///
/// Currently only supports `Text`, but this can be extended with new
/// variants (e.g., `Image`, `File`) without breaking existing consumers
/// thanks to `#[non_exhaustive]` and serde's tagged union format.
///
/// # Serde format
///
/// Serialised as a tagged union keyed on `"type"`:
/// ```json
/// {"type": "text", "text": "hello"}
/// ```
///
/// # Derive rationale
///
/// - `Clone`: content parts may be cloned during message construction.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ContentPart {
    /// A plain text segment.
    Text { text: String },
}

/// Message content that is either a plain string or a list of structured parts.
///
/// Uses `#[serde(untagged)]` so that a simple string deserialises to
/// [`Text`](MessageContent::Text) while an array deserialises to
/// [`Parts`](MessageContent::Parts). This matches how provider APIs
/// accept both forms.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    /// Plain string content.
    Text(String),
    /// Structured multi-part content (e.g., text + image).
    Parts(Vec<ContentPart>),
}

impl MessageContent {
    /// Returns the text content if this is the `Text` variant, otherwise `None`.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(s) => Some(s),
            Self::Parts(_) => None,
        }
    }
}

/// A function/tool call requested by the assistant.
///
/// Mirrors the structure returned by provider APIs (e.g., Anthropic's
/// `tool_use` content block or OpenAI's `function_call`).
///
/// # Fields
///
/// * `name` — The tool/function name requested by the model.
/// * `arguments` — JSON-encoded arguments for the function call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantFunctionCall {
    /// Function/tool name requested by the assistant.
    pub name: String,
    /// JSON-encoded arguments for the function call.
    pub arguments: String,
}

/// A tool call wrapper matching provider API response formats.
///
/// Each tool call has a unique `id` (supplied by the provider) and a
/// [`function`](AssistantToolCall::function) payload. The `kind` field
/// is typically `"function"`.
///
/// # Fields
///
/// * `id` — Provider-generated tool call identifier, used to correlate the
///   tool result response.
/// * `kind` — Tool call type (typically `"function"`).
/// * `function` — The function name and arguments.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantToolCall {
    /// Provider-generated tool call identifier.
    pub id: String,
    /// Tool call type (typically `"function"`).
    #[serde(rename = "type")]
    pub kind: String,
    /// Function call payload for the tool.
    pub function: AssistantFunctionCall,
}

/// A single message in a conversation with an LLM provider.
///
/// Represents a message from any role (system, user, assistant, tool)
/// and may carry content, tool calls, and/or extended reasoning data.
///
/// # Provider compatibility
///
/// The structure is designed to match both Anthropic Messages API and
/// OpenAI Chat Completions API wire formats:
///
/// - `role` + `content` is the common core.
/// - `tool_calls` matches the assistant role's `tool_calls` array.
/// - `tool_call_id` + `name` matches the tool role's `tool_call_id`.
/// - `reasoning_content` mirrors the `reasoning_content` field returned
///   by some providers (e.g., DeepSeek).
/// - `thinking_blocks` matches Anthropic's extended thinking blocks
///   with optional `signature` fields.
///
/// # Serde notes
///
/// Fields that are only present for certain roles use `skip_serializing_if`
/// to keep the JSON output clean.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    /// Message role (system/user/assistant/tool).
    pub role: MessageRole,
    /// Message content (text or structured parts). `None` for assistant
    /// messages that only contain tool calls.
    #[serde(default)]
    pub content: Option<MessageContent>,
    /// Tool calls attached to this message (assistant role only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<AssistantToolCall>>,
    /// Tool call ID that this tool message responds to (tool role only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Optional tool name (used for tool responses).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Optional reasoning content from providers that return it separately
    /// (e.g., DeepSeek R1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    /// Optional structured thinking blocks for long-form reasoning
    /// (e.g., Anthropic extended thinking).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_blocks: Option<Vec<ThinkingBlock>>,
}

impl ChatMessage {
    /// Creates a new system role message with plain text content.
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

    /// Creates a new user role message with plain text content.
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

    /// Creates a new user role message with structured multi-part content.
    ///
    /// Used when the message includes images or other non-text attachments.
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

    /// Creates a new assistant role message.
    ///
    /// # Arguments
    ///
    /// * `content` — Optional text content of the assistant's response.
    /// * `tool_calls` — Optional tool calls requested by the assistant.
    /// * `reasoning_content` — Optional reasoning text from providers that
    ///   return it separately.
    /// * `thinking_blocks` — Optional extended thinking blocks (Anthropic).
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

    /// Creates a new tool role message containing the result of a tool
    /// execution.
    ///
    /// # Arguments
    ///
    /// * `tool_call_id` — The provider-generated ID of the tool call being
    ///   responded to.
    /// * `name` — The name of the tool that was executed.
    /// * `content` — The tool's result as text (typically JSON or error
    ///   message).
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

    /// Returns the text content of this message, if the content is a
    /// `Text` variant. Returns `None` for `Parts` content or messages
    /// without content.
    pub fn content_as_text(&self) -> Option<&str> {
        self.content.as_ref().and_then(|c| c.as_text())
    }
}

/// A tool call request returned by the LLM in a unified, provider-agnostic format.
///
/// This is the normalised representation used internally, as opposed to the
/// provider-specific [`AssistantToolCall`] which mirrors the raw API response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRequest {
    /// Provider-generated tool call identifier, used to correlate the tool result.
    pub id: String,
    /// Tool name requested by the model, parsed into [`ToolName`] for
    /// routing to built-in or dynamic tools.
    pub name: ToolName,
    /// JSON-encoded payload for the tool arguments.
    pub arguments_json: String,
}

/// A structured thinking block with optional signature.
///
/// Required by Anthropic's extended thinking API. The `signature` field
/// must be passed back on subsequent requests when the conversation
/// continues.
///
/// # Provider notes
///
/// Anthropic requires that thinking blocks returned in the model's response
/// are mirrored back in the next request's messages array, complete with
/// their signatures. [`with_signature`](ThinkingBlock::with_signature)
/// captures this data round-trip.
///
/// # Derive rationale
///
/// - `PartialEq`: needed for comparing thinking blocks in tests.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThinkingBlock {
    /// The thinking/reasoning text content.
    pub thinking: String,
    /// Signature for this thinking block, required by Anthropic extended
    /// thinking API to be passed back on subsequent requests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

impl ThinkingBlock {
    /// Creates a new thinking block with text content only (no signature).
    ///
    /// Use this for initial construction or when signatures are not required.
    pub fn new(thinking: impl Into<String>) -> Self {
        Self {
            thinking: thinking.into(),
            signature: None,
        }
    }

    /// Creates a new thinking block with both text and an optional signature.
    ///
    /// Pass `Some(sig)` when the block needs to be round-tripped through
    /// Anthropic's API.
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

/// Provider-agnostic configuration for extended thinking and reasoning.
///
/// Supports both major provider families in a single struct:
///
/// # Anthropic (extended thinking)
///
/// | `type` | Effect |
/// |--------|--------|
/// | `"adaptive"` | Model decides when to think (Claude 4.6+) |
/// | `"enabled"` | Fixed thinking budget via `budget_tokens` (Claude 3.7/4.0-4.5) |
/// | omitted | No extended thinking |
///
/// # OpenAI-compatible (reasoning_effort)
///
/// | `effort` | Effect |
/// |----------|--------|
/// | `"low"` | Minimal reasoning |
/// | `"medium"` | Balanced reasoning |
/// | `"high"` | Deep reasoning |
/// | `"xhigh"` | Maximum reasoning (xAI Grok) |
///
/// Fields from both are co-opted by providers as needed
/// (e.g., DeepSeek uses `effort`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct ReasoningConfig {
    /// Thinking type for Anthropic extended thinking.
    ///
    /// Supported: `"adaptive"` (4.6+), `"enabled"` (3.7/4.0-4.5).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    /// Token budget for thinking (used when `type="enabled"` for
    /// Claude 3.7/4.0-4.5).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget_tokens: Option<i32>,
    /// Reasoning effort for OpenAI-compatible providers.
    ///
    /// Supported: `"low"`, `"medium"`, `"high"`, `"xhigh"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

impl ReasoningConfig {
    /// Returns the non-empty `effort` value for OpenAI-compatible providers.
    ///
    /// Returns `None` if `effort` is not set or is an empty/whitespace string.
    pub fn effort(&self) -> Option<&str> {
        self.effort.as_deref().filter(|s| !s.trim().is_empty())
    }

    /// Returns the non-empty `type` value for Anthropic extended thinking.
    ///
    /// Returns `None` if `type` is not set or is an empty/whitespace string.
    pub fn thinking_type(&self) -> Option<&str> {
        self.r#type.as_deref().filter(|s| !s.trim().is_empty())
    }
}

/// Token usage statistics from a provider response.
///
/// Tracks the number of tokens consumed by the prompt, completion, and
/// the total across both. All fields are optional because not all
/// providers report all three values.
///
/// # Derive rationale
///
/// - `Default`: useful for constructing a zero-value stats object before
///   parsing provider response headers.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageStats {
    /// Tokens used by the prompt/request sent to the provider.
    pub prompt_tokens: Option<u64>,
    /// Tokens generated in the completion/response.
    pub completion_tokens: Option<u64>,
    /// Total tokens across prompt and completion.
    pub total_tokens: Option<u64>,
}

impl UsageStats {
    /// Formats a human-readable summary string, e.g.
    /// `"Usage: prompt=150, completion=42, total=192"`.
    ///
    /// Returns `None` if all fields are `None` (no stats available).
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

/// A unified LLM response after merging provider-specific formats.
///
/// This is the canonical response type that the agent loop works with,
/// regardless of which provider produced it. It combines:
///
/// - Optional text `content` (the assistant's reply).
/// - A list of [`ToolCallRequest`]s (zero or more).
/// - A `finish_reason` string from the provider (e.g., `"end_turn"`,
///   `"tool_use"`, `"stop"`).
/// - [`UsageStats`] for token accounting.
/// - Optional extended `reasoning_content` and `thinking_blocks`.
///
/// # Fields
///
/// * `content` — The assistant's text response, if any. May be `None`
///   when the response only contains tool calls.
/// * `tool_calls` — Tool calls requested by the model. Empty if none.
/// * `finish_reason` — Provider-specific reason why generation stopped
///   (e.g., `"end_turn"`, `"max_tokens"`, `"tool_use"`, `"stop"`).
/// * `usage` — Token usage statistics from the provider.
/// * `reasoning_content` — Separate reasoning/chain-of-thought text
///   returned by some providers.
/// * `thinking_blocks` — Structured extended thinking blocks (Anthropic).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LLMResponse {
    /// Optional assistant text content.
    pub content: Option<String>,
    /// Tool calls requested by the model.
    #[serde(default)]
    pub tool_calls: Vec<ToolCallRequest>,
    /// Provider finish reason string.
    pub finish_reason: String,
    /// Usage statistics returned by the provider.
    #[serde(default)]
    pub usage: UsageStats,
    /// Optional reasoning content from providers that return it separately.
    #[serde(default)]
    pub reasoning_content: Option<String>,
    /// Optional structured thinking blocks for long-form reasoning.
    #[serde(default)]
    pub thinking_blocks: Option<Vec<ThinkingBlock>>,
}

impl LLMResponse {
    /// Returns `true` if the model requested one or more tool calls.
    ///
    /// This is a fast check used by the agent loop to decide whether to
    /// execute tools or return the response to the user.
    pub fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }
}
