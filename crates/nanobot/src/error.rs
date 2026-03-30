use std::io;

use thiserror::Error;

use nanobot_agent::AgentError;
use nanobot_bus::BusError;
use nanobot_config::ConfigError;
use nanobot_cron::CronError;
use nanobot_provider::ProviderError;
use nanobot_session::SessionError;
use nanobot_tools::ToolError;

use crate::channels::ChannelError;
use crate::heartbeat::HeartbeatError;
use crate::runtime::error::RuntimeError;

/// Result type alias using NanobotError.
pub type NanobotResult<T> = std::result::Result<T, NanobotError>;

/// Core error types for nanobot.
#[derive(Debug, Error)]
pub enum NanobotError {
    /// LLM provider error.
    #[error(transparent)]
    Provider(#[from] ProviderError),

    /// Tool error.
    #[error(transparent)]
    Tool(#[from] ToolError),

    /// Channel error.
    #[error(transparent)]
    Channel(#[from] ChannelError),

    /// Message bus error.
    #[error(transparent)]
    Bus(#[from] BusError),

    /// Session error.
    #[error(transparent)]
    Session(#[from] SessionError),

    /// Configuration error.
    #[error(transparent)]
    Config(#[from] ConfigError),

    /// Cron subsystem error.
    #[error(transparent)]
    Cron(#[from] CronError),

    /// Heartbeat subsystem error.
    #[error(transparent)]
    Heartbeat(#[from] HeartbeatError),

    /// Agent error.
    #[error(transparent)]
    Agent(#[from] AgentError),

    /// Runtime error.
    #[error(transparent)]
    Runtime(#[from] RuntimeError),

    /// I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    /// JSON serialization/deserialization error.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// Generic error for cases not covered by specific variants.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl NanobotError {
    #[allow(unused)]
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Provider(ProviderError::RateLimit(_)) => true,
            Self::Provider(ProviderError::Timeout(_)) => true,
            Self::Provider(ProviderError::ApiRequest(_)) => true,
            Self::Io(_) => true,
            _ => false,
        }
    }
}
