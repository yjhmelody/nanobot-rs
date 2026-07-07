//! Error types for the heartbeat subsystem.
//!
//! `HeartbeatError` distinguishes between provider errors (LLM call failures),
//! response errors (invalid or missing JSON in the LLM's decision), execution
//! errors (handler failures), and JSON parsing errors.

use thiserror::Error;

use nanobot_provider::ProviderError;

/// Errors that can occur during heartbeat execution.
#[derive(Debug, Error)]
pub enum HeartbeatError {
    /// Error from the LLM provider API call.
    #[error("Heartbeat provider error: {0}")]
    Provider(#[from] ProviderError),

    /// The LLM response was invalid (not valid JSON, missing fields, etc.).
    #[error("Heartbeat response error: {0}")]
    Response(String),

    /// The registered `HeartbeatExecuteHandler` returned an error.
    #[error("Heartbeat execution error: {0}")]
    Execution(String),

    /// Error deserialising the LLM's JSON decision.
    #[error("Heartbeat JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Convenience alias for `Result<T, HeartbeatError>`.
pub type HeartbeatResult<T> = std::result::Result<T, HeartbeatError>;

impl HeartbeatError {
    /// Create a `Response` variant from a message.
    pub fn response(message: impl Into<String>) -> Self {
        Self::Response(message.into())
    }

    /// Create an `Execution` variant from a message.
    pub fn execution(message: impl Into<String>) -> Self {
        Self::Execution(message.into())
    }
}
