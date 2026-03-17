//! Fallback provider implementation for automatic retry with multiple LLM providers.

use std::sync::Arc;

use async_trait::async_trait;
use tracing::{debug, warn};

use crate::provider::{ProviderError, ProviderResult};
use crate::observability::TARGET_PROVIDER;
use crate::provider::streaming::{StreamError, StreamResponse};
use crate::provider::traits::{ChatRequest, LLMProvider};
use crate::types::SessionKey;
use crate::types::provider::LLMResponse;

/// Provider with fallback support.
///
/// Attempts to use providers in order, falling back to the next provider
/// when retryable errors occur (network issues, timeouts, rate limits).
pub struct FallbackProvider {
    providers: Vec<Arc<dyn LLMProvider>>,
    default_model: String,
}

impl FallbackProvider {
    /// Creates a new fallback provider with a list of providers to try in order.
    ///
    /// # Arguments
    ///
    /// * `providers` - List of providers to try in order (must not be empty)
    /// * `default_model` - Default model to use when not specified in request
    ///
    /// # Panics
    ///
    /// Panics if the providers list is empty.
    pub fn new(providers: Vec<Arc<dyn LLMProvider>>, default_model: String) -> Self {
        assert!(
            !providers.is_empty(),
            "FallbackProvider requires at least one provider"
        );
        Self {
            providers,
            default_model,
        }
    }

    /// Returns the number of configured providers.
    pub fn provider_count(&self) -> usize {
        self.providers.len()
    }
}

#[async_trait]
impl LLMProvider for FallbackProvider {
    fn default_model(&self) -> &str {
        &self.default_model
    }

    async fn chat(&self, req: ChatRequest) -> ProviderResult<LLMResponse> {
        let mut last_error = None;

        for (index, provider) in self.providers.iter().enumerate() {
            debug!(
                target: TARGET_PROVIDER,
                provider_index = index,
                total_providers = self.providers.len(),
                "Attempting provider"
            );

            match provider.chat(req.clone()).await {
                Ok(response) => {
                    if index > 0 {
                        debug!(
                            target: TARGET_PROVIDER,
                            provider_index = index,
                            "Fallback provider succeeded"
                        );
                    }
                    return Ok(response);
                }
                Err(err) => {
                    if err.is_retryable() {
                        warn!(
                            target: TARGET_PROVIDER,
                            provider_index = index,
                            error = %err,
                            "Provider failed with retryable error, trying next provider"
                        );
                        last_error = Some(err);
                    } else {
                        warn!(
                            target: TARGET_PROVIDER,
                            provider_index = index,
                            error = %err,
                            "Provider failed with non-retryable error, aborting fallback"
                        );
                        return Err(err);
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| ProviderError::Other("All providers failed".to_string())))
    }

    async fn chat_stream(&self, req: ChatRequest) -> Result<StreamResponse, StreamError> {
        let mut last_error = None;

        for (index, provider) in self.providers.iter().enumerate() {
            debug!(
                target: TARGET_PROVIDER,
                provider_index = index,
                total_providers = self.providers.len(),
                "Attempting streaming provider"
            );

            match provider.chat_stream(req.clone()).await {
                Ok(stream) => {
                    if index > 0 {
                        debug!(
                            target: TARGET_PROVIDER,
                            provider_index = index,
                            "Fallback streaming provider succeeded"
                        );
                    }
                    return Ok(stream);
                }
                Err(err) => {
                    let is_retryable = match &err {
                        StreamError::Network(_) => true,
                        StreamError::Provider(msg) => {
                            msg.contains("rate limit") || msg.contains("timeout")
                        }
                        _ => false,
                    };

                    if is_retryable {
                        warn!(
                            target: TARGET_PROVIDER,
                            provider_index = index,
                            error = %err,
                            "Streaming provider failed with retryable error, trying next provider"
                        );
                        last_error = Some(err);
                    } else {
                        warn!(
                            target: TARGET_PROVIDER,
                            provider_index = index,
                            error = %err,
                            "Streaming provider failed with non-retryable error, aborting fallback"
                        );
                        return Err(err);
                    }
                }
            }
        }

        Err(last_error
            .unwrap_or_else(|| StreamError::Provider("All streaming providers failed".to_string())))
    }

    async fn reset_session(&self, session_key: &SessionKey) {
        for provider in &self.providers {
            provider.reset_session(session_key).await;
        }
    }

    async fn close(&self) {
        for provider in &self.providers {
            provider.close().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::provider::UsageStats;

    #[derive(Debug)]
    struct MockProvider {
        name: String,
        should_fail: bool,
        error_type: ErrorType,
    }

    #[derive(Debug, Clone)]
    enum ErrorType {
        None,
        Timeout,
        RateLimit,
        Authentication,
    }

    impl MockProvider {
        fn success(name: &str) -> Self {
            Self {
                name: name.to_string(),
                should_fail: false,
                error_type: ErrorType::None,
            }
        }

        fn with_timeout(name: &str) -> Self {
            Self {
                name: name.to_string(),
                should_fail: true,
                error_type: ErrorType::Timeout,
            }
        }

        fn with_rate_limit(name: &str) -> Self {
            Self {
                name: name.to_string(),
                should_fail: true,
                error_type: ErrorType::RateLimit,
            }
        }

        fn with_auth_error(name: &str) -> Self {
            Self {
                name: name.to_string(),
                should_fail: true,
                error_type: ErrorType::Authentication,
            }
        }
    }

    #[async_trait]
    impl LLMProvider for MockProvider {
        fn default_model(&self) -> &str {
            "mock/model"
        }

        async fn chat(&self, _req: ChatRequest) -> Result<LLMResponse, ProviderError> {
            if self.should_fail {
                Err(match self.error_type {
                    ErrorType::Timeout => ProviderError::Timeout(30),
                    ErrorType::RateLimit => {
                        ProviderError::RateLimit("Rate limit exceeded".to_string())
                    }
                    ErrorType::Authentication => {
                        ProviderError::Authentication("Invalid API key".to_string())
                    }
                    ErrorType::None => unreachable!(),
                })
            } else {
                Ok(LLMResponse {
                    content: Some(format!("Response from {}", self.name)),
                    tool_calls: Vec::new(),
                    finish_reason: "stop".to_string(),
                    usage: UsageStats::default(),
                    reasoning_content: None,
                    thinking_blocks: None,
                })
            }
        }
    }

    fn create_request() -> ChatRequest {
        ChatRequest {
            session_key: None,
            messages: vec![],
            tools: None,
            model: None,
            max_tokens: 100,
            temperature: 0.7,
            reasoning_effort: None,
        }
    }

    #[tokio::test]
    async fn fallback_uses_first_provider_when_successful() {
        let providers: Vec<Arc<dyn LLMProvider>> = vec![
            Arc::new(MockProvider::success("provider1")),
            Arc::new(MockProvider::success("provider2")),
        ];

        let fallback = FallbackProvider::new(providers, "default".to_string());
        let response = fallback.chat(create_request()).await.unwrap();

        assert_eq!(
            response.content,
            Some("Response from provider1".to_string())
        );
    }

    #[tokio::test]
    async fn fallback_tries_second_provider_on_retryable_error() {
        let providers: Vec<Arc<dyn LLMProvider>> = vec![
            Arc::new(MockProvider::with_timeout("provider1")),
            Arc::new(MockProvider::success("provider2")),
        ];

        let fallback = FallbackProvider::new(providers, "default".to_string());
        let response = fallback.chat(create_request()).await.unwrap();

        assert_eq!(
            response.content,
            Some("Response from provider2".to_string())
        );
    }

    #[tokio::test]
    async fn fallback_stops_on_non_retryable_error() {
        let providers: Vec<Arc<dyn LLMProvider>> = vec![
            Arc::new(MockProvider::with_auth_error("provider1")),
            Arc::new(MockProvider::success("provider2")),
        ];

        let fallback = FallbackProvider::new(providers, "default".to_string());
        let result = fallback.chat(create_request()).await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ProviderError::Authentication(_)
        ));
    }

    #[tokio::test]
    async fn fallback_returns_last_error_when_all_fail() {
        let providers: Vec<Arc<dyn LLMProvider>> = vec![
            Arc::new(MockProvider::with_timeout("provider1")),
            Arc::new(MockProvider::with_rate_limit("provider2")),
        ];

        let fallback = FallbackProvider::new(providers, "default".to_string());
        let result = fallback.chat(create_request()).await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ProviderError::RateLimit(_)));
    }

    #[tokio::test]
    async fn fallback_tries_all_providers_with_retryable_errors() {
        let providers: Vec<Arc<dyn LLMProvider>> = vec![
            Arc::new(MockProvider::with_timeout("provider1")),
            Arc::new(MockProvider::with_rate_limit("provider2")),
            Arc::new(MockProvider::success("provider3")),
        ];

        let fallback = FallbackProvider::new(providers, "default".to_string());
        let response = fallback.chat(create_request()).await.unwrap();

        assert_eq!(
            response.content,
            Some("Response from provider3".to_string())
        );
    }

    #[test]
    fn fallback_provider_count_returns_correct_value() {
        let providers: Vec<Arc<dyn LLMProvider>> = vec![
            Arc::new(MockProvider::success("provider1")),
            Arc::new(MockProvider::success("provider2")),
            Arc::new(MockProvider::success("provider3")),
        ];

        let fallback = FallbackProvider::new(providers, "default".to_string());
        assert_eq!(fallback.provider_count(), 3);
    }
}
