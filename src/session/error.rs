use std::io;

use thiserror::Error;

use crate::provider::ProviderError;
use regex::Error as RegexError;

/// Errors returned by session storage and management.
#[derive(Debug, Error)]
pub enum SessionError {
    /// Session not found.
    #[error("Session not found: {0}")]
    NotFound(String),

    /// Provider error during session operations (e.g., consolidation).
    #[error(transparent)]
    Provider(#[from] ProviderError),

    /// I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    /// JSON serialization/deserialization error.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// Regex compilation error.
    #[error("Regex error: {0}")]
    Regex(#[from] RegexError),

    /// Other session-related errors.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub type SessionResult<T> = std::result::Result<T, SessionError>;

impl SessionError {
    pub fn not_found(key: impl Into<String>) -> Self {
        Self::NotFound(key.into())
    }
}
