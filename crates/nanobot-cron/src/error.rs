use std::io;

use thiserror::Error;

/// Errors returned by the cron subsystem.
#[derive(Debug, Error)]
pub enum CronError {
    #[error("Cron error: {0}")]
    Message(String),

    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub type CronResult<T> = std::result::Result<T, CronError>;

impl CronError {
    pub fn message(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }
}
