use std::collections::HashMap;

use async_trait::async_trait;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use tracing::debug;
use uuid::Uuid;

use crate::observability::TARGET_PROVIDER;
use crate::provider::registry::find_spec;
use crate::provider::{
    ChatMessage, ChatRequest, LLMProvider, LLMResponse, MessageContent, MessageRole,
    ToolCallRequest, UsageStats,
};
use crate::types::provider_openai::{ChatCompletionPayload, OpenAIChatResponse, ThinkingBlock};

#[derive(Debug)]
pub struct OpenAICompatProvider {
    api_key: String,
    api_base: Option<String>,
    default_model: String,
    provider_name: String,
    extra_headers: HashMap<String, String>,
    client: reqwest::Client,
}

impl OpenAICompatProvider {
    pub fn new(
        api_key: String,
        api_base: Option<String>,
        default_model: String,
        provider_name: String,
        extra_headers: HashMap<String, String>,
    ) -> Self {
        // Build client with TLS support
        // Note: By default, reqwest uses system proxy settings from environment variables
        // If you need to disable proxy, set NO_PROXY=* environment variable
        let client = reqwest::Client::builder()
            .use_rustls_tls()
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            api_key,
            api_base,
            default_model,
            provider_name,
            extra_headers,
            client,
        }
    }

    fn resolve_model(&self, model: &str) -> String {
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

        // If the URL already contains a complete endpoint path, use it as-is
        if trimmed.ends_with("/chat/completions") || trimmed.ends_with("/responses") {
            return trimmed.to_string();
        }

        // Use /v1/responses for OpenAI provider (both official and compatible gateways)
        // Use /v1/chat/completions for other providers
        if self.provider_name == "openai" {
            format!("{}/responses", trimmed)
        } else {
            format!("{}/chat/completions", trimmed)
        }
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
}

#[async_trait]
impl LLMProvider for OpenAICompatProvider {
    async fn chat(&self, req: ChatRequest) -> LLMResponse {
        let model = self.resolve_model(req.model.as_deref().unwrap_or(&self.default_model));
        let max_tokens = req.max_tokens.max(1);
        let tool_choice = req
            .tools
            .as_ref()
            .and_then(|tools| (!tools.is_empty()).then(|| "auto".to_string()));

        let payload = ChatCompletionPayload {
            model,
            messages: Self::sanitize_messages(req.messages),
            max_tokens,
            temperature: req.temperature,
            reasoning_effort: req.reasoning_effort,
            tools: req.tools,
            tool_choice,
        };

        if let Ok(payload_text) = serde_json::to_string(&payload) {
            debug!(target: TARGET_PROVIDER, "provider request: {:#}", payload_text);
        }

        let res = self
            .client
            .post(self.endpoint())
            .headers(self.headers())
            .json(&payload)
            .send()
            .await;

        let response = match res {
            Ok(r) => r,
            Err(e) => return error_response(format!("Error calling LLM: {}", e)),
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

        match serde_json::from_str::<OpenAIChatResponse>(&body_text) {
            Ok(parsed) => parse_response(parsed),
            Err(e) => error_response(format!("Error parsing LLM response: {}", e)),
        }
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

fn parse_response(resp: OpenAIChatResponse) -> LLMResponse {
    let choice = match resp.choices.into_iter().next() {
        Some(c) => c,
        None => return error_response("Error calling LLM: empty choices".to_string()),
    };

    let tool_calls = choice
        .message
        .tool_calls
        .unwrap_or_default()
        .into_iter()
        .map(|tc| ToolCallRequest {
            id: tc.id.unwrap_or_else(|| Uuid::new_v4().to_string()),
            name: tc.function.name.into(),
            arguments_json: tc.function.arguments_json,
        })
        .collect::<Vec<_>>();

    let usage = if let Some(u) = resp.usage {
        UsageStats {
            prompt_tokens: u.prompt_tokens,
            completion_tokens: u.completion_tokens,
            total_tokens: u.total_tokens,
        }
    } else {
        UsageStats::default()
    };

    let thinking_blocks = choice.message.thinking_blocks.and_then(|blocks| {
        let collected = blocks
            .into_iter()
            .filter_map(ThinkingBlock::into_text)
            .collect::<Vec<_>>();
        (!collected.is_empty()).then_some(collected)
    });

    LLMResponse {
        content: choice.message.content,
        tool_calls,
        finish_reason: choice.finish_reason.unwrap_or_else(|| "stop".to_string()),
        usage,
        reasoning_content: choice.message.reasoning_content,
        thinking_blocks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::provider::{AssistantFunctionCall, AssistantToolCall};
    use crate::types::provider_openai::{
        AssistantMessage, Choice, OpenAIFunctionCall, OpenAIToolCall, StructuredThinkingBlock,
        Usage,
    };

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
    fn deserialize_arguments_json_accepts_object_and_string() {
        let from_object: OpenAIFunctionCall =
            serde_json::from_str(r#"{"name":"tool","arguments":{"path":"a.txt","n":1}}"#)
                .expect("deserialize object arguments");
        assert_eq!(from_object.name, "tool");
        assert_eq!(from_object.arguments_json, r#"{"path":"a.txt","n":1}"#);

        let from_string: OpenAIFunctionCall =
            serde_json::from_str(r#"{"name":"tool","arguments":"{\"path\":\"a.txt\"}"}"#)
                .expect("deserialize string arguments");
        assert_eq!(from_string.arguments_json, r#"{"path":"a.txt"}"#);
    }

    #[test]
    fn parse_response_maps_tool_calls_usage_and_thinking_blocks() {
        let resp = OpenAIChatResponse {
            choices: vec![Choice {
                message: AssistantMessage {
                    content: Some("ok".to_string()),
                    tool_calls: Some(vec![OpenAIToolCall {
                        id: None,
                        function: OpenAIFunctionCall {
                            name: "read_file".to_string(),
                            arguments_json: r#"{"path":"a.txt"}"#.to_string(),
                        },
                    }]),
                    reasoning_content: Some("reason".to_string()),
                    thinking_blocks: Some(vec![
                        ThinkingBlock::Structured(StructuredThinkingBlock {
                            text: Some("think-1".to_string()),
                            content: None,
                            summary: None,
                        }),
                        ThinkingBlock::Text("".to_string()),
                        ThinkingBlock::Text("think-2".to_string()),
                    ]),
                },
                finish_reason: Some("stop".to_string()),
            }],
            usage: Some(Usage {
                prompt_tokens: Some(1),
                completion_tokens: Some(2),
                total_tokens: Some(3),
            }),
        };

        let out = parse_response(resp);
        assert_eq!(out.content.as_deref(), Some("ok"));
        assert_eq!(out.tool_calls.len(), 1);
        assert_eq!(out.tool_calls[0].name.as_str(), "read_file");
        assert_eq!(out.tool_calls[0].arguments_json, r#"{"path":"a.txt"}"#);
        assert!(!out.tool_calls[0].id.trim().is_empty());
        assert_eq!(out.usage.prompt_tokens, Some(1));
        assert_eq!(out.usage.completion_tokens, Some(2));
        assert_eq!(out.usage.total_tokens, Some(3));
        assert_eq!(out.reasoning_content.as_deref(), Some("reason"));
        assert_eq!(
            out.thinking_blocks,
            Some(vec!["think-1".to_string(), "think-2".to_string()])
        );
    }

    #[test]
    fn endpoint_appends_chat_completions_to_base_url() {
        let provider = OpenAICompatProvider::new(
            "test-key".to_string(),
            Some("https://api.example.com/v1".to_string()),
            "gpt-4".to_string(),
            "custom".to_string(),
            HashMap::new(),
        );
        assert_eq!(
            provider.endpoint(),
            "https://api.example.com/v1/chat/completions"
        );
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
    fn endpoint_does_not_duplicate_chat_completions() {
        let provider = OpenAICompatProvider::new(
            "test-key".to_string(),
            Some("https://api.example.com/v1/chat/completions".to_string()),
            "gpt-4".to_string(),
            "custom".to_string(),
            HashMap::new(),
        );
        assert_eq!(
            provider.endpoint(),
            "https://api.example.com/v1/chat/completions"
        );
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
        assert_eq!(
            provider.endpoint(),
            "https://api.example.com/v1/chat/completions"
        );
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
        assert_eq!(
            provider.endpoint(),
            "https://api.openai.com/v1/chat/completions"
        );
    }
}
