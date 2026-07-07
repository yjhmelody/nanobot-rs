//! Error types for the session crate.
//!
//! All fallible operations in this crate return [`SessionResult<T>`], which is a
//! type alias for `Result<T, SessionError>`. The error enum wraps lower-level
//! errors from I/O, JSON serialisation, the LLM provider, and regex compilation
//! into a single type that callers can handle with `?` or pattern-match on.
//!
//! # Design
//!
//! - Uses `thiserror` for concise `Display` and `Error` derives.
//! - The `Provider` and `Other` variants are transparent (`#[error(transparent)]`),
//!   preserving the inner error's message without re-wrapping.
//! - A convenience constructor [`SessionError::not_found`] is provided for the
//!   common "session not found" case.
use std::io;

use thiserror::Error;

use nanobot_provider::ProviderError;
use regex::Error as RegexError;

/// Errors returned by session storage and management.
///
/// This enum covers all failure modes across the session crate:
/// - Missing sessions (`NotFound`)
/// - LLM provider failures during consolidation (`Provider`)
/// - Filesystem errors (`Io`)
/// - Serialisation errors (`Json`)
/// - Regex compilation errors (`Regex`)
/// - Catch-all for unexpected errors (`Other`)
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

/// Convenience alias for `Result<T, SessionError>`.
///
/// Used as the return type throughout the session crate so that callers only
/// need to handle a single error type via `?`.
pub type SessionResult<T> = std::result::Result<T, SessionError>;

impl SessionError {
    /// Creates a `NotFound` error for the given session key.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// return Err(SessionError::not_found("telegram:123"));
    /// ```
    pub fn not_found(key: impl Into<String>) -> Self {
        Self::NotFound(key.into())
    }
}
