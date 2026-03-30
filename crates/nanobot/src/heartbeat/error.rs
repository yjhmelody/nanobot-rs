use thiserror::Error;

use nanobot_provider::ProviderError;

#[derive(Debug, Error)]
pub enum HeartbeatError {
    #[error("Heartbeat provider error: {0}")]
    Provider(#[from] ProviderError),

    #[error("Heartbeat response error: {0}")]
    Response(String),

    #[error("Heartbeat execution error: {0}")]
    Execution(String),

    #[error("Heartbeat JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type HeartbeatResult<T> = std::result::Result<T, HeartbeatError>;

impl HeartbeatError {
    pub fn response(message: impl Into<String>) -> Self {
        Self::Response(message.into())
    }

    pub fn execution(message: impl Into<String>) -> Self {
        Self::Execution(message.into())
    }
}
