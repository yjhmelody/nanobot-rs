use std::collections::HashMap;

use async_trait::async_trait;
use futures::stream;
use nanobot_config::schema::ProviderWireApi;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use tracing::debug;
use uuid::Uuid;

use crate::openai_types::{
    OpenAIResponsesResponse, ResponseFunctionCallItem, ResponseFunctionCallOutputItem,
    ResponseInputContent, ResponseInputItem, ResponseInputMessage, ResponseOutputBlock,
    ResponseOutputContent, ResponseReasoningConfig, ResponseReasoningSummary,
    ResponseToolDefinition, ResponsesPayload, ResponsesUsage,
};
use crate::proxy::ProxyFallbackHelper;
use crate::proxy::TARGET;
use crate::registry::find_spec;
use crate::streaming::{OpenAiAdapter, StreamAdapter, StreamError, StreamEvent, StreamResponse};
use crate::{
    ChatMessage, ChatRequest, LLMProvider, LLMResponse, MessageContent, MessageRole,
    ToolCallRequest, UsageStats,
};
use crate::{ProviderError, ProviderResult};

#[derive(Debug)]
pub struct OpenAICompatProvider {
    api_key: String,
    api_base: Option<String>,
    default_model: String,
    provider_name: String,
    wire_api: ProviderWireApi,
    extra_headers: HashMap<String, String>,
    proxy_helper: ProxyFallbackHelper,
}

impl OpenAICompatProvider {
    pub fn new(
        api_key: String,
        api_base: Option<String>,
        default_model: String,
        provider_name: String,
        wire_api: ProviderWireApi,
        extra_headers: HashMap<String, String>,
    ) -> Self {
        Self {
            api_key,
            api_base,
            default_model,
            provider_name,
            wire_api,
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
            if spec.strip_model_prefix
                && let Some((_, tail)) = resolved.split_once('/')
            {
                resolved = tail.to_string();
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

    fn responses_endpoint(&self) -> String {
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

    fn chat_completions_endpoint(&self) -> String {
        let base = self
            .api_base
            .clone()
            .unwrap_or_else(|| "https://api.openai.com/v1".to_string());

        let trimmed = base.trim_end_matches('/');
        if trimmed.ends_with("/chat/completions") {
            return trimmed.to_string();
        }

        format!("{}/chat/completions", trimmed)
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
                if let Some(MessageContent::Text(text)) = &message.content
                    && text.is_empty()
                {
                    if matches!(message.role, MessageRole::Assistant)
                        && message
                            .tool_calls
                            .as_ref()
                            .is_some_and(|calls| !calls.is_empty())
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

    async fn send_request(
        &self,
        client: &reqwest::Client,
        request_kind: &str,
        endpoint: &str,
        payload: &serde_json::Value,
    ) -> Result<reqwest::Response, reqwest::Error> {
        let headers = self.headers();
        debug!(
            target: TARGET,
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

    fn build_chat_completions_payload(
        &self,
        model: String,
        req: ChatRequest,
        stream: bool,
    ) -> serde_json::Value {
        let messages =
            chat_completions_messages_from_chat_messages(Self::sanitize_messages(req.messages));
        let tools = req.tools.map(|tools| {
            tools
                .into_iter()
                .map(|tool| {
                    serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": tool.function.name,
                            "description": tool.function.description,
                            "parameters": tool.function.parameters,
                        }
                    })
                })
                .collect::<Vec<_>>()
        });

        let mut payload = serde_json::json!({
            "model": model,
            "messages": messages,
            "temperature": req.temperature,
            "max_tokens": req.max_tokens.max(1),
            "stream": stream,
        });

        if let Some(tools) = tools {
            payload["tools"] = serde_json::Value::Array(tools);
            payload["tool_choice"] = serde_json::Value::String("auto".to_string());
        }

        payload
    }
}

#[derive(Debug, serde::Deserialize)]
struct ChatCompletionsResponse {
    choices: Vec<ChatCompletionsChoice>,
    #[serde(default)]
    usage: Option<ChatCompletionsUsage>,
}

#[derive(Debug, serde::Deserialize)]
struct ChatCompletionsChoice {
    message: ChatCompletionsMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct ChatCompletionsMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<ChatCompletionsToolCall>>,
    #[serde(default, alias = "reasoningContent")]
    reasoning_content: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct ChatCompletionsToolCall {
    id: String,
    function: ChatCompletionsFunctionCall,
}

#[derive(Debug, serde::Deserialize)]
struct ChatCompletionsFunctionCall {
    name: String,
    arguments: String,
}

#[derive(Debug, serde::Deserialize)]
struct ChatCompletionsUsage {
    #[serde(default)]
    prompt_tokens: Option<u64>,
    #[serde(default)]
    completion_tokens: Option<u64>,
    #[serde(default)]
    total_tokens: Option<u64>,
}

#[async_trait]
impl LLMProvider for OpenAICompatProvider {
    async fn chat(&self, req: ChatRequest) -> ProviderResult<LLMResponse> {
        let model = self.resolve_model(req.model.as_deref().unwrap_or(&self.default_model));
        let (endpoint, payload) = match self.wire_api {
            ProviderWireApi::Responses => {
                let endpoint = self.responses_endpoint();
                let payload = serde_json::to_value(self.build_responses_payload(model, req))
                    .map_err(|e| {
                        ProviderError::InvalidConfig(format!(
                            "Error serializing LLM request: {}",
                            e
                        ))
                    })?;
                (endpoint, payload)
            }
            ProviderWireApi::ChatCompletions => {
                let endpoint = self.chat_completions_endpoint();
                let payload = self.build_chat_completions_payload(model, req, false);
                (endpoint, payload)
            }
        };

        let response = self
            .send_request_with_proxy_fallback(&endpoint, &payload)
            .await
            .map_err(|msg| {
                if msg.contains("timeout") {
                    ProviderError::Timeout(30)
                } else {
                    ProviderError::Other(msg)
                }
            })?;

        let status = response.status();
        let body_text = response.text().await.map_err(|e| {
            ProviderError::InvalidResponse(format!("Error reading LLM response: {}", e))
        })?;

        if !status.is_success() {
            let error_msg = format!("HTTP {}: {}", status.as_u16(), body_text);

            return Err(match status.as_u16() {
                401 | 403 => ProviderError::Authentication(error_msg),
                429 => ProviderError::RateLimit(error_msg),
                404 => ProviderError::ModelNotAvailable(error_msg),
                500..=599 => ProviderError::Other(error_msg),
                _ => ProviderError::InvalidResponse(error_msg),
            });
        }

        match self.wire_api {
            ProviderWireApi::Responses => {
                let parsed =
                    serde_json::from_str::<OpenAIResponsesResponse>(&body_text).map_err(|e| {
                        ProviderError::InvalidResponse(format!("Error parsing LLM response: {}", e))
                    })?;
                Ok(parse_responses_response(parsed))
            }
            ProviderWireApi::ChatCompletions => {
                let parsed =
                    serde_json::from_str::<ChatCompletionsResponse>(&body_text).map_err(|e| {
                        ProviderError::InvalidResponse(format!("Error parsing LLM response: {}", e))
                    })?;
                Ok(parse_chat_completions_response(parsed))
            }
        }
    }

    async fn chat_stream(&self, req: ChatRequest) -> Result<StreamResponse, StreamError> {
        if self.wire_api == ProviderWireApi::ChatCompletions {
            let response = self.chat(req).await.map_err(StreamError::from)?;
            return Ok(Box::pin(stream::once(async move {
                Ok(StreamEvent::done(response))
            })));
        }

        let model = self.resolve_model(req.model.as_deref().unwrap_or(&self.default_model));
        let endpoint = self.responses_endpoint();
        let mut payload = self.build_responses_payload(model, req);
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
    if let Some((prefix, tail)) = model.split_once('/')
        && prefix.replace('-', "_") == spec_name
    {
        return format!("{}/{}", canonical_prefix, tail);
    }
    model.to_string()
}

fn parse_responses_response(resp: OpenAIResponsesResponse) -> LLMResponse {
    if let Some(error) = resp.error.and_then(|err| err.message)
        && !error.trim().is_empty()
    {
        // Note: This should ideally return an error, but keeping for backward compatibility
        // in the response parsing path. The error should be caught earlier in the flow.
        return LLMResponse {
            content: Some(format!("Error calling LLM: {}", error)),
            tool_calls: Vec::new(),
            finish_reason: "error".to_string(),
            usage: UsageStats::default(),
            reasoning_content: None,
            thinking_blocks: None,
        };
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

fn parse_chat_completions_response(resp: ChatCompletionsResponse) -> LLMResponse {
    let Some(choice) = resp.choices.into_iter().next() else {
        return LLMResponse {
            content: None,
            tool_calls: Vec::new(),
            finish_reason: "stop".to_string(),
            usage: UsageStats::default(),
            reasoning_content: None,
            thinking_blocks: None,
        };
    };

    let tool_calls = choice
        .message
        .tool_calls
        .unwrap_or_default()
        .into_iter()
        .map(|call| ToolCallRequest {
            id: call.id,
            name: call.function.name.into(),
            arguments_json: call.function.arguments,
        })
        .collect::<Vec<_>>();

    let usage = match resp.usage {
        Some(usage) => UsageStats {
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
            total_tokens: usage.total_tokens,
        },
        None => UsageStats::default(),
    };

    LLMResponse {
        content: choice
            .message
            .content
            .filter(|text| !text.trim().is_empty()),
        tool_calls,
        finish_reason: choice.finish_reason.unwrap_or_else(|| "stop".to_string()),
        usage,
        reasoning_content: choice
            .message
            .reasoning_content
            .filter(|text| !text.trim().is_empty()),
        thinking_blocks: None,
    }
}

fn chat_completions_messages_from_chat_messages(
    messages: Vec<ChatMessage>,
) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    for message in messages {
        match message.role {
            MessageRole::Tool => {
                let mut item = serde_json::json!({
                    "role": "tool",
                    "content": message_content_text(message.content.as_ref()).unwrap_or_default(),
                });
                if let Some(call_id) = message.tool_call_id {
                    item["tool_call_id"] = serde_json::Value::String(call_id);
                }
                out.push(item);
            }
            MessageRole::Assistant => {
                let mut item = serde_json::json!({
                    "role": "assistant",
                });
                if let Some(content) = message_content_text(message.content.as_ref())
                    && !content.trim().is_empty()
                {
                    item["content"] = serde_json::Value::String(content);
                } else {
                    item["content"] = serde_json::Value::String(String::new());
                }
                if let Some(reasoning_content) = message.reasoning_content
                    && !reasoning_content.trim().is_empty()
                {
                    item["reasoning_content"] = serde_json::Value::String(reasoning_content);
                }

                if let Some(tool_calls) = message.tool_calls
                    && !tool_calls.is_empty()
                {
                    item["tool_calls"] = serde_json::Value::Array(
                        tool_calls
                            .into_iter()
                            .map(|tool_call| {
                                serde_json::json!({
                                    "id": tool_call.id,
                                    "type": "function",
                                    "function": {
                                        "name": tool_call.function.name,
                                        "arguments": tool_call.function.arguments,
                                    }
                                })
                            })
                            .collect(),
                    );
                }
                out.push(item);
            }
            _ => {
                if let Some(content) = message_content_text(message.content.as_ref()) {
                    out.push(serde_json::json!({
                        "role": role_to_responses_role(&message.role),
                        "content": content,
                    }));
                }
            }
        }
    }
    out
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
        let suffix: String = token
            .chars()
            .rev()
            .take(6)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
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
    use std::sync::Arc;

    use super::*;

    use crate::{AssistantFunctionCall, AssistantToolCall};
    use nanobot_types::tools::{JsonSchema, ToolDefinition};

    fn make_provider(provider_name: &str) -> OpenAICompatProvider {
        OpenAICompatProvider::new(
            "secret".to_string(),
            Some("https://example.com/v1".to_string()),
            "openai/gpt-4o-mini".to_string(),
            provider_name.to_string(),
            ProviderWireApi::Responses,
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
            ProviderWireApi::Responses,
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
    fn chat_completions_messages_include_reasoning_content() {
        let assistant = ChatMessage::assistant(
            Some("Need a file".to_string()),
            None,
            Some("hidden reasoning".to_string()),
            None,
        );
        let messages = chat_completions_messages_from_chat_messages(vec![assistant]);
        let value = serde_json::to_value(messages).expect("serialize chat completions messages");
        assert_eq!(value[0]["role"], "assistant");
        assert_eq!(value[0]["reasoning_content"], "hidden reasoning");
    }

    #[test]
    fn parse_chat_completions_response_preserves_reasoning_content() {
        let response = ChatCompletionsResponse {
            choices: vec![ChatCompletionsChoice {
                message: ChatCompletionsMessage {
                    content: Some("ok".to_string()),
                    tool_calls: None,
                    reasoning_content: Some("thinking trace".to_string()),
                },
                finish_reason: Some("stop".to_string()),
            }],
            usage: None,
        };
        let out = parse_chat_completions_response(response);
        assert_eq!(out.reasoning_content.as_deref(), Some("thinking trace"));
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
            usage: Some(crate::openai_types::ResponsesUsage {
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
            ProviderWireApi::Responses,
            HashMap::new(),
        );
        assert_eq!(
            provider.responses_endpoint(),
            "https://api.example.com/v1/responses"
        );
    }

    #[test]
    fn endpoint_uses_responses_for_official_openai() {
        let provider = OpenAICompatProvider::new(
            "test-key".to_string(),
            Some("https://api.openai.com/v1".to_string()),
            "gpt-4".to_string(),
            "openai".to_string(),
            ProviderWireApi::Responses,
            HashMap::new(),
        );
        assert_eq!(
            provider.responses_endpoint(),
            "https://api.openai.com/v1/responses"
        );
    }

    #[test]
    fn endpoint_uses_responses_for_openai_gateway() {
        let provider = OpenAICompatProvider::new(
            "test-key".to_string(),
            Some("https://gmn.chuangzuoli.com/v1".to_string()),
            "gpt-4".to_string(),
            "openai".to_string(),
            ProviderWireApi::Responses,
            HashMap::new(),
        );
        assert_eq!(
            provider.responses_endpoint(),
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
            ProviderWireApi::Responses,
            HashMap::new(),
        );
        assert_eq!(
            provider.responses_endpoint(),
            "https://api.openai.com/v1/responses"
        );
    }

    #[test]
    fn endpoint_does_not_duplicate_responses() {
        let provider = OpenAICompatProvider::new(
            "test-key".to_string(),
            Some("https://api.openai.com/v1/responses".to_string()),
            "gpt-4".to_string(),
            "openai".to_string(),
            ProviderWireApi::Responses,
            HashMap::new(),
        );
        assert_eq!(
            provider.responses_endpoint(),
            "https://api.openai.com/v1/responses"
        );
    }

    #[test]
    fn endpoint_handles_trailing_slash() {
        let provider = OpenAICompatProvider::new(
            "test-key".to_string(),
            Some("https://api.example.com/v1/".to_string()),
            "gpt-4".to_string(),
            "custom".to_string(),
            ProviderWireApi::Responses,
            HashMap::new(),
        );
        assert_eq!(
            provider.responses_endpoint(),
            "https://api.example.com/v1/responses"
        );
    }

    #[test]
    fn endpoint_uses_default_when_no_api_base() {
        let provider = OpenAICompatProvider::new(
            "test-key".to_string(),
            None,
            "gpt-4".to_string(),
            "custom".to_string(),
            ProviderWireApi::Responses,
            HashMap::new(),
        );
        assert_eq!(
            provider.responses_endpoint(),
            "https://api.openai.com/v1/responses"
        );
    }
}
