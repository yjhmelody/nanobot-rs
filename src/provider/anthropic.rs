use std::collections::HashMap;

use async_trait::async_trait;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use tracing::trace;

use crate::error::ProviderError;
use crate::observability::TARGET_PROVIDER;
use crate::provider::anthropic_types::{
    AnthropicContentBlock, AnthropicErrorResponse, AnthropicInputContentBlock,
    AnthropicInputMessage, AnthropicMessagesPayload, AnthropicMessagesResponse,
    AnthropicToolDefinition, AnthropicUsage,
};
use crate::provider::proxy::ProxyFallbackHelper;
use crate::provider::streaming::{SseAdapter, StreamAdapter, StreamError, StreamResponse};
use crate::provider::{
    AssistantToolCall, ChatMessage, ChatRequest, LLMProvider, LLMResponse, MessageContent,
    MessageRole, ToolCallRequest, UsageStats,
};

const DEFAULT_API_BASE: &str = "https://api.anthropic.com/v1";
const DEFAULT_ANTHROPIC_VERSION: &str = "2026-02-15";

#[derive(Debug)]
pub struct AnthropicProvider {
    api_key: String,
    api_base: Option<String>,
    default_model: String,
    extra_headers: HashMap<String, String>,
    proxy_helper: ProxyFallbackHelper,
}

impl AnthropicProvider {
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
            proxy_helper: ProxyFallbackHelper::new(),
        }
    }

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

    fn headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        if !self.api_key.trim().is_empty()
            && let Ok(value) = HeaderValue::from_str(self.api_key.trim())
        {
            headers.insert(HeaderName::from_static("x-api-key"), value);
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
            stream: None,
        }
    }

    async fn send_request(
        &self,
        client: &reqwest::Client,
        request_kind: &str,
        endpoint: &str,
        payload: &serde_json::Value,
    ) -> Result<reqwest::Response, reqwest::Error> {
        let headers = self.headers();
        trace!(
            target: TARGET_PROVIDER,
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

    async fn send_request_with_proxy_fallback(
        &self,
        endpoint: &str,
        payload: &serde_json::Value,
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
    async fn chat(&self, req: ChatRequest) -> Result<LLMResponse, ProviderError> {
        let model = req
            .model
            .clone()
            .unwrap_or_else(|| self.default_model.clone());
        let endpoint = self.endpoint();
        let payload = serde_json::to_value(self.build_payload(model, req)).map_err(|e| {
            ProviderError::InvalidConfig(format!("Error serializing request: {}", e))
        })?;

        let response = self
            .send_request_with_proxy_fallback(&endpoint, &payload)
            .await
            .map_err(|msg| ProviderError::Other(msg))?;

        let status = response.status();
        let body_text = response
            .text()
            .await
            .map_err(|e| ProviderError::ApiRequest(e))?;

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

        let payload_value = serde_json::to_value(payload)
            .map_err(|e| StreamError::Provider(format!("Error serializing request: {}", e)))?;

        let response = self
            .send_request_with_proxy_fallback(&endpoint, &payload_value)
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

    fn default_model(&self) -> &str {
        &self.default_model
    }
}

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
                    anthropic_messages.push(AnthropicInputMessage::new("user", content));
                }
            }
            MessageRole::Assistant => {
                if !pending_tool_results.is_empty() {
                    anthropic_messages.push(AnthropicInputMessage::new(
                        "user",
                        std::mem::take(&mut pending_tool_results),
                    ));
                }

                let mut content = Vec::new();
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
                    anthropic_messages.push(AnthropicInputMessage::new("assistant", content));
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
        anthropic_messages.push(AnthropicInputMessage::new("user", pending_tool_results));
    }

    let system = (!system_parts.is_empty()).then(|| system_parts.join("\n\n"));
    (system, anthropic_messages)
}

fn parse_tool_arguments(tool_call: &AssistantToolCall) -> serde_json::Value {
    serde_json::from_str(&tool_call.function.arguments)
        .unwrap_or_else(|_| serde_json::Value::String(tool_call.function.arguments.clone()))
}

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
            AnthropicContentBlock::Thinking { thinking, .. } => {
                if !thinking.trim().is_empty() {
                    thinking_blocks.push(thinking);
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

fn map_stop_reason(stop_reason: Option<&str>) -> String {
    match stop_reason {
        Some("tool_use") => "tool_calls".to_string(),
        Some("end_turn") | Some("stop_sequence") => "stop".to_string(),
        Some("max_tokens") => "length".to_string(),
        Some(other) => other.to_string(),
        None => "stop".to_string(),
    }
}

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

fn format_error_body(body_text: &str) -> String {
    match serde_json::from_str::<AnthropicErrorResponse>(body_text) {
        Ok(parsed) => parsed
            .error
            .and_then(|error| error.message)
            .unwrap_or_else(|| body_text.to_string()),
        Err(_) => body_text.to_string(),
    }
}

fn redacted_header_map(headers: &HeaderMap) -> HashMap<String, String> {
    headers
        .iter()
        .map(|(name, value)| {
            let rendered = match name.as_str() {
                "x-api-key" => redact_api_key(value),
                _ => value
                    .to_str()
                    .map(str::to_string)
                    .unwrap_or_else(|_| "<non-utf8>".to_string()),
            };
            (name.as_str().to_string(), rendered)
        })
        .collect()
}

fn redact_api_key(value: &HeaderValue) -> String {
    let raw = value.to_str().unwrap_or("<non-utf8>");
    let suffix_len = raw.len().min(6);
    let suffix = &raw[raw.len().saturating_sub(suffix_len)..];
    format!("<redacted:{}>", suffix)
}

fn format_request_body(payload: &serde_json::Value) -> String {
    serde_json::to_string_pretty(payload).unwrap_or_else(|_| payload.to_string())
}

fn message_content_text(content: Option<&MessageContent>) -> Option<String> {
    match content {
        Some(MessageContent::Text(text)) => Some(text.clone()),
        Some(MessageContent::Parts(parts)) => {
            let joined = parts
                .iter()
                .map(|part| match part {
                    crate::provider::ContentPart::Text { text } => text.as_str(),
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

    use crate::provider::{AssistantFunctionCall, ContentPart};
    use crate::types::tools::{JsonSchema, ToolDefinition};

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
            None,
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
        assert_eq!(messages[0].role, "user");
        assert_eq!(
            messages[0].content,
            vec![AnthropicInputContentBlock::Text {
                text: "hello".to_string()
            }]
        );
        assert_eq!(messages[1].role, "assistant");
        assert_eq!(
            messages[1].content,
            vec![
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
        assert_eq!(messages[2].role, "user");
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
            Some(vec!["inspect request".to_string()])
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
                reasoning_effort: Some("high".to_string()),
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
}
