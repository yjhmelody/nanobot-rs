//! Error types for the channel subsystem.
//!
//! [`ChannelError`] is a two-variant enum that cleanly separates
//! configuration-time errors (bad config values) from runtime errors
//! (network failures, API rejections).  All public-facing APIs fallible
//! functions return [`ChannelResult<T>`] which is simply
//! `Result<T, ChannelError>`.
//!
//! # Usage
//! ```ignore
//! use crate::error::ChannelError;
//!
//! fn validate(cfg: &str) -> ChannelResult<()> {
//!     if cfg.is_empty() {
//!         return Err(ChannelError::config("missing config"));
//!     }
//!     Ok(())
//! }
//! ```

use thiserror::Error;

/// Errors returned by channel adapters and channel management.
///
/// Two variants cover the full lifecycle:
/// - [`Config`](ChannelError::Config) — problems detected at construction time
///   (bad token, missing fields, invalid allow-from lists).
/// - [`Adapter`](ChannelError::Adapter) — failures during runtime operation
///   (HTTP errors, API rejections, serialization failures).
#[derive(Debug, Error)]
pub enum ChannelError {
    /// Configuration error for a channel.
    ///
    /// The string explains which configuration value is invalid and why.
    /// These errors are typically fatal at startup.
    #[error("Channel configuration error: {0}")]
    Config(String),

    /// Adapter runtime error.
    ///
    /// Includes both the channel name (for multi-instance setups) and a
    /// human-readable description of what went wrong.
    #[error("Channel '{channel}' error: {message}")]
    Adapter { channel: String, message: String },
}

/// Convenience alias for fallible channel operations.
///
/// Maps to `std::result::Result<T, ChannelError>`.
pub type ChannelResult<T> = std::result::Result<T, ChannelError>;

impl ChannelError {
    /// Build a configuration error.
    pub fn config(message: impl Into<String>) -> Self {
        Self::Config(message.into())
    }

    /// Build a runtime adapter error with channel name context.
    pub fn adapter(channel: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Adapter {
            channel: channel.into(),
            message: message.into(),
        }
    }
}
