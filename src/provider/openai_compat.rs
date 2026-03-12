use std::collections::HashMap;

use async_trait::async_trait;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use tracing::debug;
use uuid::Uuid;

use crate::observability::TARGET_PROVIDER;
use crate::provider::openai_types::{
    OpenAIResponsesResponse, ResponseFunctionCallItem, ResponseFunctionCallOutputItem,
    ResponseInputContent, ResponseInputItem, ResponseInputMessage, ResponseOutputBlock,
    ResponseOutputContent, ResponseReasoningConfig, ResponseReasoningSummary,
    ResponseToolDefinition, ResponsesPayload, ResponsesUsage,
};
use crate::provider::proxy::ProxyFallbackHelper;
use crate::provider::registry::find_spec;
use crate::provider::streaming::{OpenAiAdapter, StreamAdapter, StreamError, StreamResponse};
use crate::provider::{
    ChatMessage, ChatRequest, LLMProvider, LLMResponse, MessageContent, MessageRole,
    ToolCallRequest, UsageStats,
};

#[derive(Debug)]
pub struct OpenAICompatProvider {
    api_key: String,
    api_base: Option<String>,
    default_model: String,
    provider_name: String,
    extra_headers: HashMap<String, String>,
    proxy_helper: ProxyFallbackHelper,
}

impl OpenAICompatProvider {
    pub fn new(
        api_key: String,
        api_base: Option<String>,
        default_model: String,
        provider_name: String,
        extra_headers: HashMap<String, String>,
    ) -> Self {
        Self {
            api_key,
            api_base,
            default_model,
            provider_name,
            extra_headers,
            proxy_helper: ProxyFallbackHelper::new(),
        }
    }

    fn resolve_model(&self, model: &str) -> String {
        if self.provider_name == "openai"
            && let Some(stripped) = model.strip_prefix("openai/")
        {
            return stripped.to_string();
        }

        if let Some(spec) = find_spec(&self.provider_name) {
            if spec.litellm_prefix.is_empty() {
                return model.to_string();
            }

            let mut resolved = model.to_string();
            if spec.strip_model_prefix {
                if let Some((_, tail)) = resolved.split_once('/') {
                    resolved = tail.to_string();
                }
            }

            let canonical =
                canonicalize_explicit_prefix(&resolved, &self.provider_name, spec.litellm_prefix);
            if spec
                .skip_prefixes
                .iter()
                .any(|prefix| canonical.starts_with(prefix))
            {
                canonical
            } else {
                format!("{}/{}", spec.litellm_prefix, canonical)
            }
        } else {
            model.to_string()
        }
    }

    fn endpoint(&self) -> String {
        let base = self
            .api_base
            .clone()
            .unwrap_or_else(|| "https://api.openai.com/v1".to_string());

        let trimmed = base.trim_end_matches('/');

        // If the URL already contains the responses endpoint, use it as-is.
        if trimmed.ends_with("/responses") {
            return trimmed.to_string();
        }

        format!("{}/responses", trimmed)
    }

    fn headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        if !self.api_key.trim().is_empty() {
            let bearer = format!("Bearer {}", self.api_key.trim());
            if let Ok(value) = HeaderValue::from_str(&bearer) {
                headers.insert(AUTHORIZATION, value);
            }
        }

        for (k, v) in &self.extra_headers {
            if let (Ok(name), Ok(value)) = (
                HeaderName::from_bytes(k.as_bytes()),
                HeaderValue::from_str(v),
            ) {
                headers.insert(name, value);
            }
        }

        headers
    }

    fn sanitize_messages(messages: Vec<ChatMessage>) -> Vec<ChatMessage> {
        messages
            .into_iter()
            .map(|mut message| {
                if let Some(MessageContent::Text(text)) = &message.content {
                    if text.is_empty() {
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
                }
                message
            })
            .collect()
    }

    async fn send_request(
        &self,
        client: &reqwest::Client,
        request_kind: &str,
        endpoint: &str,
        payload: &serde_json::Value,
    ) -> Result<reqwest::Response, reqwest::Error> {
        let headers = self.headers();
        debug!(
            target: TARGET_PROVIDER,
            request_kind,
            method = "POST",
            url = endpoint,
            headers = ?redacted_header_map(&headers),
            body = %format_request_body(payload),
            "sending provider http request"
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
                        "Error calling LLM: {}. Direct retry without proxy also failed: {}",
                        err, retry_err
                    )),
                }
            }
            Err(err) => Err(format!("Error calling LLM: {}", err)),
        }
    }

    fn build_responses_payload(&self, model: String, req: ChatRequest) -> ResponsesPayload {
        let messages = Self::sanitize_messages(req.messages);
        let tool_choice = req
            .tools
            .as_ref()
            .and_then(|tools| (!tools.is_empty()).then(|| "auto".to_string()));

        ResponsesPayload {
            model,
            input: responses_input_from_messages(messages),
            max_output_tokens: req.max_tokens.max(1),
            temperature: req.temperature,
            reasoning: req
                .reasoning_effort
                .filter(|value| !value.trim().is_empty())
                .map(|effort| ResponseReasoningConfig { effort }),
            tools: req.tools.map(|tools| {
                tools
                    .into_iter()
                    .map(|t| ResponseToolDefinition::from((*t).clone()))
                    .collect()
            }),
            tool_choice,
            stream: None,
        }
    }
}

#[async_trait]
impl LLMProvider for OpenAICompatProvider {
    async fn chat(&self, req: ChatRequest) -> LLMResponse {
        let model = self.resolve_model(req.model.as_deref().unwrap_or(&self.default_model));
        let endpoint = self.endpoint();
        let payload = serde_json::to_value(self.build_responses_payload(model, req));

        let payload = match payload {
            Ok(value) => value,
            Err(err) => return error_response(format!("Error serializing LLM request: {}", err)),
        };

        let response = match self
            .send_request_with_proxy_fallback(&endpoint, &payload)
            .await
        {
            Ok(r) => r,
            Err(message) => return error_response(message),
        };

        let status = response.status();
        let body_text = match response.text().await {
            Ok(t) => t,
            Err(e) => return error_response(format!("Error reading LLM response: {}", e)),
        };

        if !status.is_success() {
            return error_response(format!(
                "Error calling LLM: HTTP {}: {}",
                status.as_u16(),
                body_text
            ));
        }

        match serde_json::from_str::<OpenAIResponsesResponse>(&body_text) {
            Ok(parsed) => parse_responses_response(parsed),
            Err(e) => error_response(format!("Error parsing LLM response: {}", e)),
        }
    }

    async fn chat_stream(&self, req: ChatRequest) -> Result<StreamResponse, StreamError> {
        let model = self.resolve_model(req.model.as_deref().unwrap_or(&self.default_model));
        let endpoint = self.endpoint();
        let mut payload = self.build_responses_payload(model, req);

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
                body_text
            )));
        }

        // Use OpenAI adapter to convert response to StreamEvent stream
        let adapter = OpenAiAdapter;
        adapter.adapt_stream(response).await
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }
}

fn canonicalize_explicit_prefix(model: &str, spec_name: &str, canonical_prefix: &str) -> String {
    if let Some((prefix, tail)) = model.split_once('/') {
        if prefix.replace('-', "_") == spec_name {
            return format!("{}/{}", canonical_prefix, tail);
        }
    }
    model.to_string()
}

fn error_response(message: String) -> LLMResponse {
    LLMResponse {
        content: Some(message),
        tool_calls: Vec::new(),
        finish_reason: "error".to_string(),
        usage: UsageStats::default(),
        reasoning_content: None,
        thinking_blocks: None,
    }
}

fn parse_responses_response(resp: OpenAIResponsesResponse) -> LLMResponse {
    if let Some(error) = resp.error.and_then(|err| err.message)
        && !error.trim().is_empty()
    {
        return error_response(format!("Error calling LLM: {}", error));
    }

    let mut content_blocks = Vec::new();
    let mut tool_calls = Vec::new();
    let mut thinking_blocks = Vec::new();

    for block in resp.output {
        match block {
            ResponseOutputBlock::Message { content } => {
                let texts: Vec<String> = content
                    .into_iter()
                    .filter_map(|c| match c {
                        ResponseOutputContent::OutputText { text }
                        | ResponseOutputContent::InputText { text } => {
                            (!text.trim().is_empty()).then_some(text)
                        }
                    })
                    .collect();
                if !texts.is_empty() {
                    content_blocks.push(texts.join("\n\n"));
                }
            }
            ResponseOutputBlock::FunctionCall {
                call_id,
                name,
                arguments,
            } => {
                let id = call_id.unwrap_or_else(|| Uuid::new_v4().to_string());
                let arguments_json = if let Some(text) = arguments.as_str() {
                    text.to_string()
                } else {
                    serde_json::to_string(&arguments).unwrap_or_default()
                };
                tool_calls.push(ToolCallRequest {
                    id,
                    name: name.into(),
                    arguments_json,
                });
            }
            ResponseOutputBlock::Reasoning { summary } => {
                for item in summary {
                    match item {
                        ResponseReasoningSummary::SummaryText { text } => {
                            if !text.trim().is_empty() {
                                thinking_blocks.push(text);
                            }
                        }
                    }
                }
            }
        }
    }

    let usage = map_responses_usage(resp.usage);
    let content = (!content_blocks.is_empty()).then(|| content_blocks.join("\n\n"));
    let thinking_blocks = (!thinking_blocks.is_empty()).then_some(thinking_blocks);
    let finish_reason = if tool_calls.is_empty() {
        "stop".to_string()
    } else {
        "tool_calls".to_string()
    };

    LLMResponse {
        content,
        tool_calls,
        finish_reason,
        usage,
        reasoning_content: None,
        thinking_blocks,
    }
}

fn map_responses_usage(usage: Option<ResponsesUsage>) -> UsageStats {
    match usage {
        Some(usage) => UsageStats {
            prompt_tokens: usage.input_tokens,
            completion_tokens: usage.output_tokens,
            total_tokens: usage.total_tokens,
        },
        None => UsageStats::default(),
    }
}

fn responses_input_from_messages(messages: Vec<ChatMessage>) -> Vec<ResponseInputItem> {
    let mut input = Vec::new();

    for message in messages {
        match message.role {
            MessageRole::System | MessageRole::User | MessageRole::Assistant => {
                if let Some(text) = message_content_text(message.content.as_ref())
                    && !text.trim().is_empty()
                {
                    input.push(ResponseInputItem::Message(ResponseInputMessage {
                        role: role_to_responses_role(&message.role).to_string(),
                        content: vec![ResponseInputContent::input_text(text)],
                    }));
                }

                if matches!(message.role, MessageRole::Assistant)
                    && let Some(tool_calls) = message.tool_calls
                {
                    for tool_call in tool_calls {
                        input.push(ResponseInputItem::FunctionCall(ResponseFunctionCallItem {
                            kind: "function_call",
                            call_id: tool_call.id,
                            name: tool_call.function.name,
                            arguments: tool_call.function.arguments,
                        }));
                    }
                }
            }
            MessageRole::Tool => {
                let Some(call_id) = message.tool_call_id else {
                    continue;
                };

                let output = message_content_text(message.content.as_ref()).unwrap_or_default();
                input.push(ResponseInputItem::FunctionCallOutput(
                    ResponseFunctionCallOutputItem {
                        kind: "function_call_output",
                        call_id,
                        output,
                    },
                ));
            }
        }
    }

    input
}

fn role_to_responses_role(role: &MessageRole) -> &'static str {
    match role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
    }
}

fn redacted_header_map(headers: &HeaderMap) -> HashMap<String, String> {
    headers
        .iter()
        .map(|(name, value)| {
            let rendered = if name == AUTHORIZATION {
                redact_authorization_header(value)
            } else {
                value
                    .to_str()
                    .map(str::to_string)
                    .unwrap_or_else(|_| "<non-utf8>".to_string())
            };
            (name.as_str().to_string(), rendered)
        })
        .collect()
}

fn redact_authorization_header(value: &HeaderValue) -> String {
    let raw = value.to_str().unwrap_or("<non-utf8>");
    if let Some(token) = raw.strip_prefix("Bearer ") {
        let suffix_len = token.len().min(6);
        let suffix = &token[token.len().saturating_sub(suffix_len)..];
        return format!("Bearer <redacted:{}>", suffix);
    }
    "<redacted>".to_string()
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
    use std::sync::Arc;

    use super::*;

    use crate::provider::{AssistantFunctionCall, AssistantToolCall};
    use crate::types::tools::{JsonSchema, ToolDefinition};

    fn make_provider(provider_name: &str) -> OpenAICompatProvider {
        OpenAICompatProvider::new(
            "secret".to_string(),
            Some("https://example.com/v1".to_string()),
            "openai/gpt-4o-mini".to_string(),
            provider_name.to_string(),
            HashMap::new(),
        )
    }

    #[test]
    fn resolve_model_applies_provider_rules() {
        let aihubmix = make_provider("aihubmix");
        assert_eq!(aihubmix.resolve_model("openai/gpt-4.1"), "openai/gpt-4.1");

        let deepseek = make_provider("deepseek");
        assert_eq!(deepseek.resolve_model("deepseek/chat"), "deepseek/chat");

        let unknown = make_provider("unknown-provider");
        assert_eq!(unknown.resolve_model("model-x"), "model-x");

        let openai = make_provider("openai");
        assert_eq!(openai.resolve_model("openai/gpt-5.4"), "gpt-5.4");
    }

    #[test]
    fn canonicalize_prefix_supports_hyphenated_provider_name() {
        let out = canonicalize_explicit_prefix(
            "github-copilot/gpt-4o",
            "github_copilot",
            "github_copilot",
        );
        assert_eq!(out, "github_copilot/gpt-4o");
    }

    #[test]
    fn headers_include_auth_and_valid_extra_headers_only() {
        let mut extra = HashMap::new();
        extra.insert("X-Test".to_string(), "ok".to_string());
        extra.insert("bad header".to_string(), "ignored".to_string());

        let provider = OpenAICompatProvider::new(
            "secret".to_string(),
            None,
            "openai/gpt-4o-mini".to_string(),
            "openrouter".to_string(),
            extra,
        );

        let headers = provider.headers();
        let auth = headers
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert_eq!(auth, "Bearer secret");
        assert_eq!(
            headers
                .get("x-test")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default(),
            "ok"
        );
        assert!(headers.get("bad header").is_none());
    }

    #[test]
    fn sanitize_messages_handles_empty_content() {
        let tool_call = AssistantToolCall {
            id: "tc1".to_string(),
            kind: "function".to_string(),
            function: AssistantFunctionCall {
                name: "read_file".to_string(),
                arguments: r#"{"path":"a.txt"}"#.to_string(),
            },
        };
        let assistant =
            ChatMessage::assistant(Some(String::new()), Some(vec![tool_call]), None, None);
        let user = ChatMessage::user_text("");

        let out = OpenAICompatProvider::sanitize_messages(vec![assistant, user]);
        assert!(out[0].content.is_none());
        assert_eq!(out[1].content_as_text(), Some("(empty)"));
    }

    #[test]
    fn responses_input_from_messages_maps_assistant_tools_and_outputs() {
        let assistant = ChatMessage::assistant(
            Some("Need a file".to_string()),
            Some(vec![AssistantToolCall {
                id: "call_123".to_string(),
                kind: "function".to_string(),
                function: AssistantFunctionCall {
                    name: "read_file".to_string(),
                    arguments: r#"{"path":"Cargo.toml"}"#.to_string(),
                },
            }]),
            None,
            None,
        );
        let tool = ChatMessage::tool_result("call_123", "read_file", "file contents");

        let input =
            responses_input_from_messages(vec![ChatMessage::system_text("sys"), assistant, tool]);
        let value = serde_json::to_value(&input).expect("serialize responses input");

        assert_eq!(value[0]["role"], "system");
        assert_eq!(value[0]["content"][0]["text"], "sys");
        assert_eq!(value[1]["role"], "assistant");
        assert_eq!(value[1]["content"][0]["text"], "Need a file");
        assert_eq!(value[2]["type"], "function_call");
        assert_eq!(value[2]["call_id"], "call_123");
        assert_eq!(value[2]["name"], "read_file");
        assert_eq!(value[3]["type"], "function_call_output");
        assert_eq!(value[3]["call_id"], "call_123");
        assert_eq!(value[3]["output"], "file contents");
    }

    #[test]
    fn build_responses_payload_uses_flat_tools_schema() {
        let provider = make_provider("openai");
        let req = ChatRequest {
            session_key: None,
            messages: vec![ChatMessage::user_text("hi")],
            tools: Some(vec![Arc::new(ToolDefinition::function(
                "read_file",
                "Read a file",
                JsonSchema::object(Default::default(), vec![]),
            ))]),
            model: Some("openai/gpt-5.4".to_string()),
            max_tokens: 128,
            temperature: 0.0,
            reasoning_effort: Some("medium".to_string()),
        };

        let payload = provider.build_responses_payload("gpt-5.4".to_string(), req);
        let value = serde_json::to_value(payload).expect("serialize responses payload");

        assert_eq!(value["model"], "gpt-5.4");
        assert_eq!(value["input"][0]["role"], "user");
        assert_eq!(value["input"][0]["content"][0]["type"], "input_text");
        assert_eq!(value["input"][0]["content"][0]["text"], "hi");
        assert_eq!(value["tools"][0]["type"], "function");
        assert_eq!(value["tools"][0]["name"], "read_file");
        assert!(value["tools"][0].get("function").is_none());
        assert_eq!(value["reasoning"]["effort"], "medium");
    }

    #[test]
    fn parse_responses_response_maps_text_tool_calls_and_usage() {
        let resp = OpenAIResponsesResponse {
            output: vec![
                ResponseOutputBlock::Reasoning {
                    summary: vec![ResponseReasoningSummary::SummaryText {
                        text: "inspect request".to_string(),
                    }],
                },
                ResponseOutputBlock::FunctionCall {
                    call_id: Some("call_123".to_string()),
                    name: "read_file".to_string(),
                    arguments: serde_json::json!({"path": "Cargo.toml"}),
                },
                ResponseOutputBlock::Message {
                    content: vec![ResponseOutputContent::OutputText {
                        text: "ok".to_string(),
                    }],
                },
            ],
            usage: Some(crate::provider::openai_types::ResponsesUsage {
                input_tokens: Some(10),
                output_tokens: Some(5),
                total_tokens: Some(15),
            }),
            error: None,
        };

        let out = parse_responses_response(resp);
        assert_eq!(out.content.as_deref(), Some("ok"));
        assert_eq!(out.tool_calls.len(), 1);
        assert_eq!(out.tool_calls[0].id, "call_123");
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
    fn endpoint_appends_responses_to_base_url() {
        let provider = OpenAICompatProvider::new(
            "test-key".to_string(),
            Some("https://api.example.com/v1".to_string()),
            "gpt-4".to_string(),
            "custom".to_string(),
            HashMap::new(),
        );
        assert_eq!(provider.endpoint(), "https://api.example.com/v1/responses");
    }

    #[test]
    fn endpoint_uses_responses_for_official_openai() {
        let provider = OpenAICompatProvider::new(
            "test-key".to_string(),
            Some("https://api.openai.com/v1".to_string()),
            "gpt-4".to_string(),
            "openai".to_string(),
            HashMap::new(),
        );
        assert_eq!(provider.endpoint(), "https://api.openai.com/v1/responses");
    }

    #[test]
    fn endpoint_uses_responses_for_openai_gateway() {
        let provider = OpenAICompatProvider::new(
            "test-key".to_string(),
            Some("https://gmn.chuangzuoli.com/v1".to_string()),
            "gpt-4".to_string(),
            "openai".to_string(),
            HashMap::new(),
        );
        assert_eq!(
            provider.endpoint(),
            "https://gmn.chuangzuoli.com/v1/responses"
        );
    }

    #[test]
    fn endpoint_uses_responses_for_openai_without_api_base() {
        let provider = OpenAICompatProvider::new(
            "test-key".to_string(),
            None,
            "gpt-4".to_string(),
            "openai".to_string(),
            HashMap::new(),
        );
        assert_eq!(provider.endpoint(), "https://api.openai.com/v1/responses");
    }

    #[test]
    fn endpoint_does_not_duplicate_responses() {
        let provider = OpenAICompatProvider::new(
            "test-key".to_string(),
            Some("https://api.openai.com/v1/responses".to_string()),
            "gpt-4".to_string(),
            "openai".to_string(),
            HashMap::new(),
        );
        assert_eq!(provider.endpoint(), "https://api.openai.com/v1/responses");
    }

    #[test]
    fn endpoint_handles_trailing_slash() {
        let provider = OpenAICompatProvider::new(
            "test-key".to_string(),
            Some("https://api.example.com/v1/".to_string()),
            "gpt-4".to_string(),
            "custom".to_string(),
            HashMap::new(),
        );
        assert_eq!(provider.endpoint(), "https://api.example.com/v1/responses");
    }

    #[test]
    fn endpoint_uses_default_when_no_api_base() {
        let provider = OpenAICompatProvider::new(
            "test-key".to_string(),
            None,
            "gpt-4".to_string(),
            "custom".to_string(),
            HashMap::new(),
        );
        assert_eq!(provider.endpoint(), "https://api.openai.com/v1/responses");
    }
}
