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

pub type ProviderResult<T> = std::result::Result<T, ProviderError>;

impl ProviderError {
    /// Check if this error is retryable (network issues, timeouts, rate limits).
    pub fn is_retryable(&self) -> bool {
        match self {
            ProviderError::ApiRequest(e) => {
                e.is_timeout()
                    || e.is_connect()
                    || e.status().map_or(false, |s| s.is_server_error())
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

    /// Creates a timeout error.
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
