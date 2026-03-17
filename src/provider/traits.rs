use std::fmt::Debug;
use std::sync::Arc;

use async_trait::async_trait;

use crate::provider::ProviderResult;
use crate::provider::streaming::{StreamError, StreamEvent, StreamResponse};
use crate::tools::base::ToolDefinition;
use crate::types::SessionKey;
use crate::types::provider::{ChatMessage, LLMResponse};

/// Request payload for LLM chat completion.
#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub session_key: Option<SessionKey>,
    pub messages: Vec<ChatMessage>,
    pub tools: Option<Vec<Arc<ToolDefinition>>>,
    pub model: Option<String>,
    pub max_tokens: i32,
    pub temperature: f32,
    pub reasoning_effort: Option<String>,
}

/// Trait for LLM providers supporting both streaming and non-streaming chat completions.
///
/// Implementations must provide the `chat()` method for non-streaming responses.
/// The `chat_stream()` method has a default implementation that wraps the non-streaming
/// response in a single-event stream, but providers should override it for true streaming.
#[async_trait]
pub trait LLMProvider: Send + Sync {
    // TODO: remove this
    /// Returns the default model identifier for this provider.
    fn default_model(&self) -> &str;

    /// Non-streaming chat completion.
    ///
    /// # Arguments
    ///
    /// * `req` - Chat request with messages, tools, and parameters
    ///
    /// # Returns
    ///
    /// Complete LLM response with content, tool calls, and usage stats
    ///
    /// # Errors
    ///
    /// Returns `ProviderError` for network issues, authentication failures, rate limits, etc.
    async fn chat(&self, req: ChatRequest) -> ProviderResult<LLMResponse>;

    /// Streaming chat completion (unified interface).
    ///
    /// Default implementation wraps the non-streaming `chat()` response in a single-event stream.
    /// Providers should override this method to provide true streaming support.
    ///
    /// # Arguments
    ///
    /// * `req` - Chat request with messages, tools, and parameters
    ///
    /// # Returns
    ///
    /// Stream of events (text deltas, tool calls, usage updates, etc.)
    ///
    /// # Errors
    ///
    /// Returns `StreamError` if the stream cannot be created or if a network/parse error occurs
    async fn chat_stream(&self, req: ChatRequest) -> Result<StreamResponse, StreamError> {
        let response = self.chat(req).await.map_err(StreamError::from)?;
        Ok(Box::pin(futures::stream::once(async move {
            Ok(StreamEvent::done(response))
        })))
    }

    /// Resets session state for the given session key.
    ///
    /// Default implementation does nothing. Providers can override to clear caches or state.
    async fn reset_session(&self, _session_key: &SessionKey) {}

    /// Closes the provider and releases resources.
    ///
    /// Default implementation does nothing. Providers can override to clean up connections.
    async fn close(&self) {}
}
