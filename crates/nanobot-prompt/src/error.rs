use std::io;

use thiserror::Error;

/// Errors returned by prompt rendering and storage.
#[derive(Debug, Error)]
pub enum PromptError {
    #[error("Prompt error: {0}")]
    Message(String),

    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("TOML decode error: {0}")]
    TomlDe(#[from] toml::de::Error),

    #[error("TOML encode error: {0}")]
    TomlSer(#[from] toml::ser::Error),
}

pub type PromptResult<T> = std::result::Result<T, PromptError>;

impl PromptError {
    pub fn message(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }
}
