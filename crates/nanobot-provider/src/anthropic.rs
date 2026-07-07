//! Anthropic Claude provider implementation.
//!
//! This module implements [`LLMProvider`] for Anthropic's Messages API, supporting:
//!
//! - Non-streaming and streaming (SSE) chat completions
//! - System prompts (extracted from `MessageRole::System` messages)
//! - Tool/function calling via Anthropic's native `tool_use` content blocks
//! - Extended thinking (thinking/signature content blocks) for reasoning models
//! - Proxy fallback (retry without proxy on gateway errors)
//!
//! # Wire Format
//!
//! The provider translates nanobot's unified [`ChatMessage`] types into Anthropic's
//! `messages` API format. Notable translations:
//!
//! | nanobot Type          | Anthropic Equivalent                  |
//! |-----------------------|---------------------------------------|
//! | `System` messages     | Top-level `system` field (joined)     |
//! | `User` messages       | `{"role": "user", "content": [...]}`   |
//! | `Assistant` messages  | `{"role": "assistant", "content": [...]}` |
//! | `Tool` messages       | `{"role": "user"}` with `tool_result` blocks |
//! | Thinking blocks       | `thinking` content block               |
//! | Reasoning content     | `thinking` content block (no signature)|
//!
//! # Endpoint
//!
//! Defaults to `https://api.anthropic.com/v1/messages`. The API version header is
//! hard-coded to `2026-02-15`.
//!
//! Spec source: <https://docs.anthropic.com/en/docs/api/messages>

use std::collections::HashMap;

use async_trait::async_trait;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use tracing::trace;

use crate::anthropic_types::{
    AnthropicContentBlock, AnthropicErrorResponse, AnthropicInputContentBlock,
    AnthropicInputMessage, AnthropicMessageRole, AnthropicMessagesPayload,
    AnthropicMessagesResponse, AnthropicThinkingConfig, AnthropicToolDefinition, AnthropicUsage,
};
use crate::proxy::ProxyFallbackHelper;
use crate::proxy::TARGET;
use crate::streaming::{SseAdapter, StreamAdapter, StreamError, StreamResponse};
use crate::{
    AssistantToolCall, ChatMessage, ChatRequest, LLMProvider, LLMResponse, MessageContent,
    MessageRole, ThinkingBlock, ToolCallRequest, UsageStats,
};
use crate::{ProviderError, ProviderResult};

const DEFAULT_API_BASE: &str = "https://api.anthropic.com/v1";
const DEFAULT_ANTHROPIC_VERSION: &str = "2026-02-15";

/// Provider for Anthropic's Claude models via the Messages API.
///
/// Handles authentication (x-api-key + Bearer headers), endpoint construction,
/// message format translation, error classification, and SSE-based streaming.
///
/// # Proxy Fallback
///
/// If environment proxy variables (`HTTP_PROXY`, `HTTPS_PROXY`, etc.) are detected,
/// the provider will automatically retry failed requests without the proxy. This is
/// handled by the internal [`ProxyFallbackHelper`].
///
/// # Thread Safety
///
/// The provider is fully `Send + Sync` and designed to be shared as `Arc<dyn LLMProvider>`.
/// All state is either immutable (configuration) or lock-free (no mutable shared state).
#[derive(Debug)]
pub struct AnthropicProvider {
    api_key: String,
    api_base: Option<String>,

    /// Default model identifier (e.g. "claude-sonnet-4-20250514").
    default_model: String,

    /// Arbitrary extra HTTP headers injected into every request.
    extra_headers: HashMap<String, String>,

    /// Internal helper for proxy fallback logic.
    proxy_helper: ProxyFallbackHelper,
}

#[test]
fn anthropic_messages_include_reasoning_content_as_thinking() {
    let assistant = ChatMessage::assistant(
        Some("Final answer".to_string()),
        None,
        Some("chain of thought".to_string()),
        None,
    );

    let (_, messages) = anthropic_messages_from_chat(vec![assistant]);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, AnthropicMessageRole::Assistant);
    assert_eq!(
        messages[0].content,
        vec![
            AnthropicInputContentBlock::Thinking {
                thinking: "chain of thought".to_string(),
                signature: None,
            },
            AnthropicInputContentBlock::Text {
                text: "Final answer".to_string(),
            },
        ]
    );
}

impl AnthropicProvider {
    /// Creates a new Anthropic provider.
    ///
    /// # Arguments
    ///
    /// * `api_key` - Anthropic API key (set as both `x-api-key` and `Bearer` auth headers)
    /// * `api_base` - Optional base URL override (defaults to `https://api.anthropic.com/v1`)
    /// * `default_model` - Model identifier used when `ChatRequest.model` is `None`
    /// * `extra_headers` - Additional HTTP headers to include in every request
    pub fn new(
        api_key: String,
        api_base: Option<String>,
        default_model: String,
        extra_headers: HashMap<String, String>,
    ) -> Self {
        Self {
            api_key,
            api_base,
            default_model,
            extra_headers,
            // ProxyFallbackHelper is constructed eagerly; it checks env vars at
            // init time rather than on every request.
            proxy_helper: ProxyFallbackHelper::new(),
        }
    }

    /// Constructs the `/messages` endpoint URL.
    ///
    /// Handles trailing slashes and avoids duplicating the `/messages` suffix.
    fn endpoint(&self) -> String {
        let base = self
            .api_base
            .clone()
            .unwrap_or_else(|| DEFAULT_API_BASE.to_string());
        let trimmed = base.trim_end_matches('/');

        if trimmed.ends_with("/messages") {
            return trimmed.to_string();
        }

        format!("{}/messages", trimmed)
    }

    /// Builds the HTTP headers for an Anthropic API request.
    ///
    /// Includes:
    /// - `Content-Type: application/json`
    /// - `x-api-key` and `Authorization: Bearer` (both set from `api_key`)
    /// - `anthropic-version: 2026-02-15`
    /// - Any extra headers from configuration
    fn headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        // Anthropic accepts both x-api-key and Bearer auth; we send both for
        // compatibility with proxies and gateways.
        if !self.api_key.trim().is_empty()
            && let Ok(value) = HeaderValue::from_str(self.api_key.trim())
        {
            headers.insert(HeaderName::from_static("x-api-key"), value);
            if let Ok(value) = HeaderValue::from_str(&format!("Bearer {}", self.api_key.trim())) {
                headers.insert(AUTHORIZATION, value);
            }
        }

        headers.insert(
            HeaderName::from_static("anthropic-version"),
            HeaderValue::from_static(DEFAULT_ANTHROPIC_VERSION),
        );

        for (key, value) in &self.extra_headers {
            if let (Ok(name), Ok(header_value)) = (
                HeaderName::from_bytes(key.as_bytes()),
                HeaderValue::from_str(value),
            ) {
                headers.insert(name, header_value);
            }
        }

        headers
    }

    /// Replaces empty text content with placeholders to satisfy Anthropic's API
    /// requirement that content fields be non-empty strings.
    ///
    /// For assistant messages with tool calls, the content is set to `None` (omitted).
    /// For all other messages with empty text, content becomes `"(empty)"`.
    fn sanitize_messages(messages: Vec<ChatMessage>) -> Vec<ChatMessage> {
        messages
            .into_iter()
            .map(|mut message| {
                if let Some(MessageContent::Text(text)) = &message.content
                    && text.is_empty()
                {
                    if matches!(message.role, MessageRole::Assistant)
                        && message
                            .tool_calls
                            .as_ref()
                            .map(|calls| !calls.is_empty())
                            .unwrap_or(false)
                    {
                        message.content = None;
                    } else {
                        message.content = Some(MessageContent::Text("(empty)".to_string()));
                    }
                }

                message
            })
            .collect()
    }

    /// Builds the Anthropic Messages API request payload from the unified `ChatRequest`.
    ///
    /// Temperature is clamped to `[0.0, 1.0]` per Anthropic's range. System messages
    /// are extracted into the top-level `system` field.
    fn build_payload(&self, model: String, req: ChatRequest) -> AnthropicMessagesPayload {
        let temperature = req.temperature.clamp(0.0, 1.0);
        let messages = Self::sanitize_messages(req.messages);
        let (system, messages) = anthropic_messages_from_chat(messages);

        AnthropicMessagesPayload {
            model,
            system,
            messages,
            max_tokens: req.max_tokens.max(1),
            temperature: Some(temperature),
            tools: req.tools.and_then(|tools| {
                (!tools.is_empty()).then(|| {
                    tools
                        .into_iter()
                        .map(|t| AnthropicToolDefinition::from((*t).clone()))
                        .collect()
                })
            }),
            thinking: req.reasoning_effort.as_ref().and_then(|r| {
                r.thinking_type().map(|t| AnthropicThinkingConfig {
                    r#type: t.to_string(),
                    budget_tokens: r.budget_tokens,
                })
            }),
            stream: None,
        }
    }

    /// Sends an HTTP request to the Anthropic API with tracing.
    ///
    /// Headers and body are logged at `trace` level with sensitive fields redacted.
    async fn send_request<T: serde::Serialize + std::fmt::Debug>(
        &self,
        client: &reqwest::Client,
        request_kind: &str,
        endpoint: &str,
        payload: &T,
    ) -> Result<reqwest::Response, reqwest::Error> {
        let headers = self.headers();
        trace!(
            target: TARGET,
            request_kind,
            method = "POST",
            url = endpoint,
            headers = ?redacted_header_map(&headers),
            body = %format_request_body(payload),
            "sending anthropic http request"
        );

        client
            .post(endpoint)
            .headers(headers)
            .json(payload)
            .send()
            .await
    }

    /// Sends a request with automatic proxy fallback on failure.
    ///
    /// The retry strategy is:
    /// 1. Send via the proxy-respecting client.
    /// 2. If the response is a gateway error (502/503/504), retry via direct client.
    /// 3. If the request fails entirely (connection error, timeout), retry via direct client.
    /// 4. If both attempts fail, return a combined error message.
    async fn send_request_with_proxy_fallback<T: serde::Serialize + std::fmt::Debug>(
        &self,
        endpoint: &str,
        payload: &T,
    ) -> Result<reqwest::Response, String> {
        let primary = self
            .send_request(self.proxy_helper.client(), "primary", endpoint, payload)
            .await;

        match primary {
            Ok(response)
                if self.proxy_helper.is_enabled()
                    && self
                        .proxy_helper
                        .should_retry_response(response.status(), endpoint) =>
            {
                match self
                    .send_request(
                        self.proxy_helper.direct_client(),
                        "direct_retry",
                        endpoint,
                        payload,
                    )
                    .await
                {
                    Ok(retry_response) => Ok(retry_response),
                    Err(err) => {
                        self.proxy_helper.log_retry_failed(endpoint, &err);
                        // Return the original response rather than failing entirely
                        Ok(response)
                    }
                }
            }
            Ok(response) => Ok(response),
            Err(err) if self.proxy_helper.is_enabled() => {
                self.proxy_helper.log_retry_after_error(endpoint, &err);

                match self
                    .send_request(
                        self.proxy_helper.direct_client(),
                        "direct_retry",
                        endpoint,
                        payload,
                    )
                    .await
                {
                    Ok(response) => Ok(response),
                    Err(retry_err) => Err(format!(
                        "Error calling Claude: {}. Direct retry without proxy also failed: {}",
                        err, retry_err
                    )),
                }
            }
            Err(err) => Err(format!("Error calling Claude: {}", err)),
        }
    }
}

#[async_trait]
impl LLMProvider for AnthropicProvider {
    async fn chat(&self, req: ChatRequest) -> ProviderResult<LLMResponse> {
        let model = req
            .model
            .clone()
            .unwrap_or_else(|| self.default_model.clone());
        let endpoint = self.endpoint();
        let payload = self.build_payload(model, req);

        let response = self
            .send_request_with_proxy_fallback(&endpoint, &payload)
            .await
            .map_err(ProviderError::Other)?;

        let status = response.status();
        let body_text = response.text().await.map_err(ProviderError::ApiRequest)?;

        if !status.is_success() {
            let error_msg = format!(
                "HTTP {}: {}",
                status.as_u16(),
                format_error_body(&body_text)
            );

            return Err(match status.as_u16() {
                401 | 403 => ProviderError::Authentication(error_msg),
                429 => ProviderError::RateLimit(error_msg),
                404 => ProviderError::ModelNotAvailable(error_msg),
                500..=599 => ProviderError::Other(error_msg),
                _ => ProviderError::InvalidResponse(error_msg),
            });
        }

        let parsed =
            serde_json::from_str::<AnthropicMessagesResponse>(&body_text).map_err(|e| {
                ProviderError::InvalidResponse(format!("Error parsing response: {}", e))
            })?;

        Ok(parse_messages_response(parsed))
    }

    async fn chat_stream(&self, req: ChatRequest) -> Result<StreamResponse, StreamError> {
        let model = req
            .model
            .clone()
            .unwrap_or_else(|| self.default_model.clone());
        let endpoint = self.endpoint();
        let mut payload = self.build_payload(model, req);

        // Enable streaming
        payload.stream = Some(true);

        let response = self
            .send_request_with_proxy_fallback(&endpoint, &payload)
            .await
            .map_err(StreamError::Network)?;

        let status = response.status();
        if !status.is_success() {
            let body_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(StreamError::Provider(format!(
                "HTTP {}: {}",
                status.as_u16(),
                format_error_body(&body_text)
            )));
        }

        // Use SSE adapter to convert response to StreamEvent stream
        let adapter = SseAdapter;
        adapter.adapt_stream(response).await
    }
}

/// Converts nanobot's unified message list into Anthropic's input format.
///
/// Rules applied:
/// - `System` messages are concatenated (with double newline) into the top-level `system` string.
/// - `User` messages become `AnthropicInputMessage` with role `User`.
/// - `Assistant` messages with reasoning content, thinking blocks, tool calls, and text are
///   mapped to corresponding Anthropic content blocks (Thinking, Text, ToolUse).
/// - `Tool` messages are buffered and flushed as `ToolResult` content blocks attached to the
///   next `User` or `Assistant` message. This is required because Anthropic's API does not
///   support a standalone `tool` role — results must be merged into a user message.
///
/// # Pending Tool Results
///
/// The `pending_tool_results` buffer accumulates consecutive `Tool` messages. When a
/// `User` or `Assistant` message follows, the buffered results are prepended to its content.
/// Any remaining results at the end are flushed as a synthetic User message.
fn anthropic_messages_from_chat(
    messages: Vec<ChatMessage>,
) -> (Option<String>, Vec<AnthropicInputMessage>) {
    let mut system_parts = Vec::new();
    let mut anthropic_messages = Vec::new();
    let mut pending_tool_results = Vec::new();

    for message in messages {
        match message.role {
            MessageRole::System => {
                if let Some(text) = message_content_text(message.content.as_ref())
                    && !text.trim().is_empty()
                {
                    system_parts.push(text);
                }
            }
            MessageRole::User => {
                let mut content = std::mem::take(&mut pending_tool_results);
                if let Some(text) = message_content_text(message.content.as_ref())
                    && !text.trim().is_empty()
                {
                    content.push(AnthropicInputContentBlock::Text { text });
                }
                if !content.is_empty() {
                    anthropic_messages.push(AnthropicInputMessage::new(
                        AnthropicMessageRole::User,
                        content,
                    ));
                }
            }
            MessageRole::Assistant => {
                if !pending_tool_results.is_empty() {
                    anthropic_messages.push(AnthropicInputMessage::new(
                        AnthropicMessageRole::User,
                        std::mem::take(&mut pending_tool_results),
                    ));
                }

                let mut content = Vec::new();
                if let Some(thinking_blocks) = message.thinking_blocks {
                    for block in thinking_blocks {
                        if !block.thinking.trim().is_empty() {
                            content.push(AnthropicInputContentBlock::Thinking {
                                thinking: block.thinking,
                                signature: block.signature,
                            });
                        }
                    }
                } else if let Some(reasoning) = message.reasoning_content
                    && !reasoning.trim().is_empty()
                {
                    content.push(AnthropicInputContentBlock::Thinking {
                        thinking: reasoning,
                        signature: None,
                    });
                }
                if let Some(text) = message_content_text(message.content.as_ref())
                    && !text.trim().is_empty()
                {
                    content.push(AnthropicInputContentBlock::Text { text });
                }

                if let Some(tool_calls) = message.tool_calls {
                    content.extend(tool_calls.into_iter().map(|tool_call| {
                        let input = parse_tool_arguments(&tool_call);
                        AnthropicInputContentBlock::ToolUse {
                            id: tool_call.id,
                            name: tool_call.function.name,
                            input,
                        }
                    }));
                }

                if !content.is_empty() {
                    anthropic_messages.push(AnthropicInputMessage::new(
                        AnthropicMessageRole::Assistant,
                        content,
                    ));
                }
            }
            MessageRole::Tool => {
                let Some(tool_use_id) = message.tool_call_id else {
                    continue;
                };
                pending_tool_results.push(AnthropicInputContentBlock::ToolResult {
                    tool_use_id,
                    content: message_content_text(message.content.as_ref()).unwrap_or_default(),
                    is_error: None,
                });
            }
        }
    }

    if !pending_tool_results.is_empty() {
        anthropic_messages.push(AnthropicInputMessage::new(
            AnthropicMessageRole::User,
            pending_tool_results,
        ));
    }

    let system = (!system_parts.is_empty()).then(|| system_parts.join("\n\n"));
    (system, anthropic_messages)
}

/// Parses tool call arguments from a JSON string into a `serde_json::Value`.
///
/// If the arguments are not valid JSON, they are wrapped as a plain string value
/// to preserve the original text. This is a fallback for malformed tool call payloads.
fn parse_tool_arguments(tool_call: &AssistantToolCall) -> serde_json::Value {
    serde_json::from_str(&tool_call.function.arguments)
        .unwrap_or_else(|_| serde_json::Value::String(tool_call.function.arguments.clone()))
}

/// Converts an [`AnthropicMessagesResponse`] into the unified [`LLMResponse`] format.
///
/// Distribution of content across `LLMResponse` fields:
/// - `Text` blocks → joined into `content` (double newline separated)
/// - `ToolUse` blocks → `tool_calls`
/// - `Thinking` blocks → `thinking_blocks`
/// - `stop_reason` → mapped to "tool_calls", "stop", or "length"
/// - `usage` → mapped to `UsageStats`
fn parse_messages_response(resp: AnthropicMessagesResponse) -> LLMResponse {
    let mut content_blocks = Vec::new();
    let mut tool_calls = Vec::new();
    let mut thinking_blocks = Vec::new();

    for block in resp.content {
        match block {
            AnthropicContentBlock::Text { text } => {
                if !text.trim().is_empty() {
                    content_blocks.push(text);
                }
            }
            AnthropicContentBlock::ToolUse { id, name, input } => {
                let arguments_json = serde_json::to_string(&input).unwrap_or_default();
                tool_calls.push(ToolCallRequest {
                    id,
                    name: name.into(),
                    arguments_json,
                });
            }
            AnthropicContentBlock::Thinking {
                thinking,
                signature,
            } => {
                if !thinking.trim().is_empty() {
                    thinking_blocks.push(ThinkingBlock::with_signature(thinking, signature));
                }
            }
        }
    }

    let content = (!content_blocks.is_empty()).then(|| content_blocks.join("\n\n"));
    let thinking_blocks = (!thinking_blocks.is_empty()).then_some(thinking_blocks);

    LLMResponse {
        content,
        tool_calls,
        finish_reason: map_stop_reason(resp.stop_reason.as_deref()),
        usage: map_usage(resp.usage),
        reasoning_content: None,
        thinking_blocks,
    }
}

/// Maps Anthropic stop reasons to unified finish reason strings.
///
/// | Anthropic          | Unified    |
/// |--------------------|------------|
/// | `tool_use`         | `tool_calls` |
/// | `end_turn`         | `stop`     |
/// | `stop_sequence`    | `stop`     |
/// | `max_tokens`       | `length`   |
fn map_stop_reason(stop_reason: Option<&str>) -> String {
    match stop_reason {
        Some("tool_use") => "tool_calls".to_string(),
        Some("end_turn") | Some("stop_sequence") => "stop".to_string(),
        Some("max_tokens") => "length".to_string(),
        Some(other) => other.to_string(),
        None => "stop".to_string(),
    }
}

/// Maps Anthropic usage data to unified `UsageStats`.
///
/// Computes `total_tokens` as `input_tokens + output_tokens` when both are available.
fn map_usage(usage: Option<AnthropicUsage>) -> UsageStats {
    match usage {
        Some(usage) => {
            let total_tokens = match (usage.input_tokens, usage.output_tokens) {
                (Some(input), Some(output)) => Some(input + output),
                _ => None,
            };

            UsageStats {
                prompt_tokens: usage.input_tokens,
                completion_tokens: usage.output_tokens,
                total_tokens,
            }
        }
        None => UsageStats::default(),
    }
}

/// Extracts the human-readable error message from an Anthropic error response body.
///
/// If the body is valid JSON matching [`AnthropicErrorResponse`], the `.error.message`
/// field is returned. Otherwise the raw body text is returned as-is.
fn format_error_body(body_text: &str) -> String {
    match serde_json::from_str::<AnthropicErrorResponse>(body_text) {
        Ok(parsed) => parsed
            .error
            .and_then(|error| error.message)
            .unwrap_or_else(|| body_text.to_string()),
        Err(_) => body_text.to_string(),
    }
}

/// Creates a debug-friendly representation of headers with sensitive values redacted.
///
/// Redacts `x-api-key` and `authorization` headers, showing only the last 6 characters.
/// This is used in trace logging to avoid leaking API keys.
fn redacted_header_map(headers: &HeaderMap) -> HashMap<String, String> {
    headers
        .iter()
        .map(|(name, value)| {
            let rendered = match name.as_str() {
                "x-api-key" | "authorization" => redact_api_key(value),
                _ => value
                    .to_str()
                    .map(str::to_string)
                    .unwrap_or_else(|_| "<non-utf8>".to_string()),
            };
            (name.as_str().to_string(), rendered)
        })
        .collect()
}

/// Redacts an API key header value for logging, revealing only the last 6 characters.
///
/// Example output: `<redacted:AbCdEf>`
fn redact_api_key(value: &HeaderValue) -> String {
    let raw = value.to_str().unwrap_or("<non-utf8>");
    let suffix: String = raw
        .chars()
        .rev()
        .take(6)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("<redacted:{}>", suffix)
}

/// Pretty-prints a serializable payload for trace logging.
///
/// If serialization fails, returns `<unprintable>`.
fn format_request_body<T: serde::Serialize>(payload: &T) -> String {
    serde_json::to_string_pretty(payload).unwrap_or_else(|_| "<unprintable>".to_string())
}

/// Extracts plain text content from an optional `MessageContent`.
///
/// For `MessageContent::Text`, returns the text directly.
/// For `MessageContent::Parts`, concatenates all text parts (non-text parts are skipped).
/// Returns `None` if content is absent or the result is empty.
fn message_content_text(content: Option<&MessageContent>) -> Option<String> {
    match content {
        Some(MessageContent::Text(text)) => Some(text.clone()),
        Some(MessageContent::Parts(parts)) => {
            let joined = parts
                .iter()
                .map(|part| match part {
                    crate::ContentPart::Text { text } => text.as_str(),
                    _ => "",
                })
                .collect::<Vec<_>>()
                .join("");
            (!joined.is_empty()).then_some(joined)
        }
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use super::*;

    use crate::{AssistantFunctionCall, ContentPart, ReasoningConfig};
    use nanobot_types::tools::{JsonSchema, ToolDefinition};

    #[test]
    fn anthropic_messages_extract_system_tools_and_results() {
        let assistant = ChatMessage::assistant(
            Some("Calling tool".to_string()),
            Some(vec![AssistantToolCall {
                id: "toolu_1".to_string(),
                kind: "function".to_string(),
                function: AssistantFunctionCall {
                    name: "read_file".to_string(),
                    arguments: r#"{"path":"Cargo.toml"}"#.to_string(),
                },
            }]),
            None,
            Some(vec![
                ThinkingBlock::new("reasoning-1"),
                ThinkingBlock::new("reasoning-2"),
            ]),
        );
        let tool = ChatMessage::tool_result("toolu_1", "read_file", "contents");
        let user = ChatMessage::user_parts(vec![ContentPart::Text {
            text: "hello".to_string(),
        }]);

        let (system, messages) = anthropic_messages_from_chat(vec![
            ChatMessage::system_text("sys-a"),
            ChatMessage::system_text("sys-b"),
            user,
            assistant,
            tool,
        ]);

        assert_eq!(system.as_deref(), Some("sys-a\n\nsys-b"));
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].role, AnthropicMessageRole::User);
        assert_eq!(
            messages[0].content,
            vec![AnthropicInputContentBlock::Text {
                text: "hello".to_string()
            }]
        );
        assert_eq!(messages[1].role, AnthropicMessageRole::Assistant);
        assert_eq!(
            messages[1].content,
            vec![
                AnthropicInputContentBlock::Thinking {
                    thinking: "reasoning-1".to_string(),
                    signature: None,
                },
                AnthropicInputContentBlock::Thinking {
                    thinking: "reasoning-2".to_string(),
                    signature: None,
                },
                AnthropicInputContentBlock::Text {
                    text: "Calling tool".to_string()
                },
                AnthropicInputContentBlock::ToolUse {
                    id: "toolu_1".to_string(),
                    name: "read_file".to_string(),
                    input: serde_json::json!({"path": "Cargo.toml"}),
                }
            ]
        );
        assert_eq!(messages[2].role, AnthropicMessageRole::User);
        assert_eq!(
            messages[2].content,
            vec![AnthropicInputContentBlock::ToolResult {
                tool_use_id: "toolu_1".to_string(),
                content: "contents".to_string(),
                is_error: None,
            }]
        );
    }

    #[test]
    fn parse_messages_response_maps_text_tool_calls_and_usage() {
        let response = AnthropicMessagesResponse {
            content: vec![
                AnthropicContentBlock::Thinking {
                    thinking: "inspect request".to_string(),
                    signature: Some("sig".to_string()),
                },
                AnthropicContentBlock::ToolUse {
                    id: "toolu_123".to_string(),
                    name: "read_file".to_string(),
                    input: serde_json::json!({"path": "Cargo.toml"}),
                },
                AnthropicContentBlock::Text {
                    text: "ok".to_string(),
                },
            ],
            stop_reason: Some("tool_use".to_string()),
            usage: Some(AnthropicUsage {
                input_tokens: Some(10),
                output_tokens: Some(5),
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            }),
        };

        let out = parse_messages_response(response);
        assert_eq!(out.content.as_deref(), Some("ok"));
        assert_eq!(out.tool_calls.len(), 1);
        assert_eq!(out.tool_calls[0].id, "toolu_123");
        assert_eq!(out.tool_calls[0].name.as_str(), "read_file");
        assert_eq!(out.tool_calls[0].arguments_json, r#"{"path":"Cargo.toml"}"#);
        assert_eq!(out.finish_reason, "tool_calls");
        assert_eq!(out.usage.prompt_tokens, Some(10));
        assert_eq!(out.usage.completion_tokens, Some(5));
        assert_eq!(out.usage.total_tokens, Some(15));
        assert_eq!(
            out.thinking_blocks,
            Some(vec![ThinkingBlock::with_signature(
                "inspect request",
                Some("sig".to_string())
            )])
        );
    }

    #[test]
    fn build_payload_maps_tool_definitions() {
        let provider = AnthropicProvider::new(
            "sk-ant-test".to_string(),
            None,
            "claude-sonnet-4-5".to_string(),
            HashMap::new(),
        );
        let mut properties = BTreeMap::new();
        properties.insert("path".to_string(), JsonSchema::string(Some("path")));

        let payload = provider.build_payload(
            "claude-sonnet-4-5".to_string(),
            ChatRequest {
                session_key: None,
                messages: vec![ChatMessage::user_text("hello")],
                tools: Some(vec![Arc::new(ToolDefinition::function(
                    "read_file",
                    "Read a file",
                    JsonSchema::object(properties, vec!["path"]),
                ))]),
                model: None,
                max_tokens: 1024,
                temperature: 1.5,
                reasoning_effort: Some(ReasoningConfig {
                    effort: Some("high".to_string()),
                    ..Default::default()
                }),
            },
        );

        assert_eq!(payload.temperature, Some(1.0));
        assert!(payload.tools.is_some());
        assert_eq!(payload.tools.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn endpoint_appends_messages_to_base_url() {
        let provider = AnthropicProvider::new(
            "sk-ant-test".to_string(),
            Some("https://api.anthropic.com/v1".to_string()),
            "claude-sonnet-4-5".to_string(),
            HashMap::new(),
        );

        assert_eq!(provider.endpoint(), "https://api.anthropic.com/v1/messages");
    }

    #[test]
    fn endpoint_preserves_explicit_messages_suffix() {
        let provider = AnthropicProvider::new(
            "sk-ant-test".to_string(),
            Some("https://api.anthropic.com/v1/messages".to_string()),
            "claude-sonnet-4-5".to_string(),
            HashMap::new(),
        );

        assert_eq!(provider.endpoint(), "https://api.anthropic.com/v1/messages");
    }

    #[test]
    fn headers_include_both_x_api_key_and_bearer_authorization() {
        let provider = AnthropicProvider::new(
            "sk-ant-test".to_string(),
            None,
            "claude-sonnet-4-5".to_string(),
            HashMap::new(),
        );

        let headers = provider.headers();
        assert_eq!(
            headers
                .get("x-api-key")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default(),
            "sk-ant-test"
        );
        assert_eq!(
            headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default(),
            "Bearer sk-ant-test"
        );
    }
}
