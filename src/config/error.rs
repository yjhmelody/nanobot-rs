use std::io;

use thiserror::Error;

/// Errors returned when validating or loading configuration.
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Invalid configuration: {0}")]
    Invalid(String),

    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type ConfigResult<T> = std::result::Result<T, ConfigError>;

impl ConfigError {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }
}
