//! Error types for LLM provider operations.
//!
//! Defines [`ProviderError`], the unified error enum returned by all provider
//! implementations, and the [`ProviderResult`] type alias for convenience.
//!
//! Each variant maps to a distinct failure category that the agent loop and fallback
//! logic need to distinguish:
//!
//! | Variant            | Retryable? | Example                                     |
//! |--------------------|------------|---------------------------------------------|
//! | `ApiRequest`       | Yes        | Connection reset, TLS error                 |
//! | `Timeout`          | Yes        | Request exceeded timeout                    |
//! | `RateLimit`        | Yes        | HTTP 429 from upstream                      |
//! | `Authentication`   | No         | Invalid or expired API key                  |
//! | `InvalidConfig`    | No         | Missing model, bad provider name            |
//! | `ModelNotAvailable`| No         | HTTP 404 from upstream                      |
//! | `InvalidResponse`  | No         | Malformed JSON from upstream                |
//! | `Other`            | No         | Catch-all for unexpected errors             |
//!
//! The [`is_retryable`](ProviderError::is_retryable) method is used by
//! [`FallbackProvider`](crate::fallback::FallbackProvider) to decide whether
//! to try the next provider in the chain.

use thiserror::Error;

/// Errors returned by LLM providers.
#[derive(Debug, Error)]
pub enum ProviderError {
    /// API request failed.
    #[error("API request failed: {0}")]
    ApiRequest(#[from] reqwest::Error),

    /// Invalid API response.
    #[error("Invalid API response: {0}")]
    InvalidResponse(String),

    /// Authentication failed.
    #[error("Authentication failed: {0}")]
    Authentication(String),

    /// Rate limit exceeded.
    #[error("Rate limit exceeded: {0}")]
    RateLimit(String),

    /// Model not found or not available.
    #[error("Model not available: {0}")]
    ModelNotAvailable(String),

    /// Invalid model configuration.
    #[error("Invalid model configuration: {0}")]
    InvalidConfig(String),

    /// Request timeout.
    #[error("Request timeout after {0}s")]
    Timeout(u64),

    /// Generic provider error.
    #[error("Provider error: {0}")]
    Other(String),
}

/// Convenience alias for `Result<T, ProviderError>`.
pub type ProviderResult<T> = std::result::Result<T, ProviderError>;

impl ProviderError {
    /// Returns `true` if the error is transient and the operation may succeed on retry.
    ///
    /// Retryable errors are:
    /// - Network-level failures (timeouts, connection refused)
    /// - Server-side HTTP errors (5xx)
    /// - Rate limits (429)
    /// - Explicit timeouts
    ///
    /// Non-retryable errors include authentication failures, invalid configuration,
    /// and malformed responses — retrying these would produce the same result.
    pub fn is_retryable(&self) -> bool {
        match self {
            ProviderError::ApiRequest(e) => {
                e.is_timeout() || e.is_connect() || e.status().is_some_and(|s| s.is_server_error())
            }
            ProviderError::Timeout(_) => true,
            ProviderError::RateLimit(_) => true,
            ProviderError::Authentication(_) => false,
            ProviderError::InvalidConfig(_) => false,
            ProviderError::ModelNotAvailable(_) => false,
            ProviderError::InvalidResponse(_) => false,
            ProviderError::Other(_) => false,
        }
    }

    /// Creates a rate limit error.
    pub fn rate_limit(message: impl Into<String>) -> Self {
        Self::RateLimit(message.into())
    }

    /// Creates a timeout error for the given duration in seconds.
    pub fn timeout(seconds: u64) -> Self {
        Self::Timeout(seconds)
    }

    /// Creates an authentication error.
    pub fn authentication(message: impl Into<String>) -> Self {
        Self::Authentication(message.into())
    }

    /// Creates an invalid response error.
    pub fn invalid_response(message: impl Into<String>) -> Self {
        Self::InvalidResponse(message.into())
    }
}
