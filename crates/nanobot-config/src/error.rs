//! Error types for configuration loading and validation.
//!
//! This module defines [`ConfigError`], the primary error type for the
//! `nanobot-config` crate, and the [`ConfigResult<T>`] convenience alias.
//!
//! # Design
//!
//! - [`ConfigError`] is a simple enum with three variants covering the common
//!   failure modes: invalid values (validation), I/O failures (file read/write),
//!   and JSON deserialisation errors.
//! - The `Invalid` variant is constructed via [`ConfigError::invalid`] and is
//!   used pervasively by [`crate::schema`] validation methods.
//! - The `Io` and `Json` variants use `#[from]` so [`std::io::Error`] and
//!   [`serde_json::Error`] convert automatically via `?`.
//!
//! # Relationships
//!
//! - Re-exported at the crate root via `pub use error::{ConfigError, ConfigResult}`.
//! - Used by [`crate::loader`] (file I/O) and [`crate::schema`] (validation).

use std::io;

use thiserror::Error;

/// Errors returned when validating or loading configuration.
///
/// This is the single error type for the entire `nanobot-config` crate.
///
/// # Variants
///
/// * `Invalid` — Configuration value failed a semantic validation check.
///   Constructed via [`ConfigError::invalid`].
/// * `Io` — An I/O error occurred (file read, create dir, write). Converted
///   automatically from [`std::io::Error`] via `#[from]`.
/// * `Json` — A JSON serialisation or deserialisation error. Converted
///   automatically from [`serde_json::Error`] via `#[from]`.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// The configuration contained an invalid or out-of-range value.
    #[error("Invalid configuration: {0}")]
    Invalid(String),

    /// An I/O error occurred while reading or writing the config file.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    /// A JSON serialisation or deserialisation error occurred.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Convenience alias for `Result<T, ConfigError>`.
///
/// This alias is used as the return type for all fallible operations in
/// `nanobot-config`, including loading, saving, and validating configuration.
pub type ConfigResult<T> = std::result::Result<T, ConfigError>;

impl ConfigError {
    /// Creates an `Invalid` error with the given message.
    ///
    /// This is the primary way validation methods produce errors. The message
    /// should describe what was invalid and, when possible, what the actual
    /// value was.
    ///
    /// # Arguments
    ///
    /// * `message` — A human-readable description of the validation failure.
    ///   Any type implementing `Into<String>` is accepted.
    ///
    /// # Returns
    ///
    /// A `ConfigError::Invalid` variant wrapping the message.
    ///
    /// # Example
    ///
    /// ```
    /// use nanobot_config::ConfigError;
    ///
    /// let err = ConfigError::invalid("max_tokens must be positive, got 0");
    /// assert!(err.to_string().contains("Invalid"));
    /// ```
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }
}
