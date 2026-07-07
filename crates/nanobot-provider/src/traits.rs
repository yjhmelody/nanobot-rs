//! Core trait definitions for LLM providers.
//!
//! This module defines:
//!
//! - [`ChatRequest`]: The unified request payload sent to every provider.
//! - [`LLMProvider`]: The trait that every backend (Anthropic, OpenAI-compatible, etc.)
//!   must implement.
//!
//! # Design Rationale
//!
//! - [`ChatRequest`] is provider-agnostic: it uses `nanobot-types` message types and maps
//!   reasoning config generically. Each concrete provider translates this into its own
//!   wire format internally.
//! - [`LLMProvider`] uses `#[async_trait]` for async trait methods, required because
//!   Rust does not yet support `async fn` in traits natively.
//! - [`chat_stream`] has a default implementation that wraps [`chat`] in a single-event
//!   stream, so providers that do not support streaming still satisfy the trait without
//!   boilerplate.
//! - The `Send + Sync` bounds allow providers to be shared as `Arc<dyn LLMProvider>`.

use std::fmt::Debug;
use std::sync::Arc;

use async_trait::async_trait;

use crate::ProviderResult;
use crate::streaming::{StreamError, StreamEvent, StreamResponse};
use nanobot_types::SessionKey;
use nanobot_types::provider::{ChatMessage, LLMResponse, ReasoningConfig};
use nanobot_types::tools::ToolDefinition;

/// Request payload for LLM chat completion.
#[derive(Debug, Clone)]
pub struct ChatRequest {
    /// Session key used to correlate the request with a conversation.
    pub session_key: Option<SessionKey>,
    /// Full message history to send to the provider.
    pub messages: Vec<ChatMessage>,
    /// Tool definitions available to the model, if any.
    pub tools: Option<Vec<Arc<ToolDefinition>>>,
    /// Model identifier override; uses the provider default when `None`.
    pub model: Option<String>,
    /// Maximum number of tokens to generate in the response.
    pub max_tokens: i32,
    /// Sampling temperature (0.0 = deterministic, higher = more creative).
    pub temperature: f32,
    /// Optional reasoning/thinking configuration.
    ///
    /// Provider-agnostic: Anthropic reads `r#type` / `budget_tokens`, OpenAI reads `effort`.
    pub reasoning_effort: Option<ReasoningConfig>,
}

/// Trait for LLM providers supporting both streaming and non-streaming chat completions.
///
/// Implementations must provide the `chat()` method for non-streaming responses.
/// The `chat_stream()` method has a default implementation that wraps the non-streaming
/// response in a single-event stream, but providers should override it for true streaming.
#[async_trait]
pub trait LLMProvider: Send + Sync {
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
