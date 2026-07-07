//! Error and result types for the cron subsystem.
//!
//! [`CronError`] is a unified error enum that wraps the common failure modes:
//! string messages, I/O errors, JSON parse errors, and opaque `anyhow` errors.
//! [`CronResult<T>`] is a convenience alias for `Result<T, CronError>`.

use std::io;

use thiserror::Error;

/// Errors returned by the cron subsystem.
///
/// Each variant represents a distinct failure category:
///
/// | Variant | Source | When |
/// |---------|--------|------|
/// | `Message` | Explicit string | Business-logic validation failures |
/// | `Io` | `std::io::Error` | Filesystem operations (read/write store) |
/// | `Json` | `serde_json::Error` | Store file deserialization |
/// | `Other` | `anyhow::Error` | Wrap any other error type |
///
/// # Conversions
///
/// - `io::Error` and `serde_json::Error` are automatically converted via `From`.
/// - Use [`CronError::message`] for string-based errors.
/// - Use `?` on `anyhow::Error` values (transparent `From` impl).
#[derive(Debug, Error)]
pub enum CronError {
    /// A string-based error message for validation or logic failures.
    #[error("Cron error: {0}")]
    Message(String),

    /// An I/O error from reading or writing the store file.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    /// A JSON serialization/deserialization error from the store file.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// Wraps any other error via `anyhow`.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Convenience alias for `Result<T, CronError>`.
pub type CronResult<T> = std::result::Result<T, CronError>;

impl CronError {
    /// Creates a `CronError::Message` from any `Into<String>` value.
    ///
    /// This is the idiomatic way to produce a cron error from a static string
    /// or formatted string.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// return Err(CronError::message("invalid schedule"));
    /// return Err(CronError::message(format!("unknown job: {}", id)));
    /// ```
    pub fn message(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }
}
