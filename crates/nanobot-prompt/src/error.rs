//! Error types for the prompt management system.
//!
//! Defines `PromptError`, an enum covering the failure modes of prompt loading,
//! parsing, serialization, and rendering. A `PromptResult<T>` type alias is
//! provided as a shorthand throughout the crate.
//!
//! # Design
//!
//! - Uses `thiserror` for ergonomic `From` impls (e.g. `io::Error`,
//!   `toml::de::Error` are automatically converted via `#[from]`).
//! - The `Message` variant is a catch-all for domain-specific errors that do not
//!   map to I/O or TOML failures (e.g. validation errors, missing fields).

use std::io;

use thiserror::Error;

/// Errors returned by prompt rendering, storage, and validation.
///
/// Each variant captures a distinct failure category, allowing callers to
/// handle I/O errors separately from parse errors when desired.
#[derive(Debug, Error)]
pub enum PromptError {
    /// A general prompt-related error message.
    ///
    /// Used for domain-specific failures such as validation errors, missing
    /// required fields, or unexpected state that does not fit into I/O or
    /// TOML categories.
    #[error("Prompt error: {0}")]
    Message(String),

    /// An I/O error occurred (file not found, permission denied, etc.).
    ///
    /// Automatically converted from `std::io::Error` via `#[from]`.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    /// Failed to decode TOML content into a prompt structure.
    ///
    /// Typically raised when a `.toml` file has invalid syntax or does not
    /// match the expected schema.
    #[error("TOML decode error: {0}")]
    TomlDe(#[from] toml::de::Error),

    /// Failed to serialize a prompt structure into TOML.
    ///
    /// Raised when `toml::to_string_pretty` fails, which is rare and usually
    /// indicates a bug in the serialization implementation.
    #[error("TOML encode error: {0}")]
    TomlSer(#[from] toml::ser::Error),
}

/// Convenience alias for `Result<T, PromptError>` used throughout the crate.
pub type PromptResult<T> = std::result::Result<T, PromptError>;

impl PromptError {
    /// Construct a `PromptError::Message` from any type implementing `Into<String>`.
    ///
    /// This is the primary way to create domain-specific errors that are not
    /// I/O or TOML related.
    ///
    /// # Examples
    ///
    /// ```
    /// use nanobot_prompt::PromptError;
    ///
    /// let err = PromptError::message("prompt name cannot be empty");
    /// assert_eq!(err.to_string(), "Prompt error: prompt name cannot be empty");
    /// ```
    pub fn message(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }
}
