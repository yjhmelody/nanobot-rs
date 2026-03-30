use async_trait::async_trait;

use super::events::{StreamError, StreamResponse};

/// Provider-specific streaming adapter.
///
/// Different LLM providers use different streaming formats:
/// - Anthropic: Server-Sent Events (SSE)
/// - OpenAI: SSE with `data: [DONE]` terminator
/// - Azure OpenAI: Similar to OpenAI
///
/// StreamAdapter is responsible for converting raw HTTP response streams into unified StreamEvent streams.
#[async_trait]
pub trait StreamAdapter: Send + Sync {
    /// Converts a raw HTTP response into a unified StreamEvent stream.
    ///
    /// # Arguments
    ///
    /// * `response` - HTTP response object
    ///
    /// # Returns
    ///
    /// Returns a StreamResponse, which is an async stream of StreamEvents
    ///
    /// # Errors
    ///
    /// Returns StreamError if the stream cannot be created (e.g., malformed response format)
    async fn adapt_stream(
        &self,
        response: reqwest::Response,
    ) -> Result<StreamResponse, StreamError>;
}
