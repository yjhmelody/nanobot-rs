//! Top-level error types for the nanobot application binary.
//!
//! `NanobotError` is a unified error enum that wraps errors from every
//! sub-crate (provider, tools, channels, bus, sessions, config, cron, agent,
//! heartbeat, runtime) as well as standard I/O and serialisation errors.
//!
//! ## Design
//!
//! All sub-crate errors are converted via `#[from]` derives so that the `?`
//! operator works seamlessly across crate boundaries. The `Other` variant
//! captures any error not otherwise covered via `anyhow::Error`.
//!
//! ## Retryability
//!
//! `NanobotError::is_retryable()` indicates whether the operation may succeed
//! on retry (provider rate-limits, timeouts, and I/O errors).

use std::io;

use thiserror::Error;

use nanobot_agent::AgentError;
use nanobot_bus::BusError;
use nanobot_config::ConfigError;
use nanobot_cron::CronError;
use nanobot_provider::ProviderError;
use nanobot_session::SessionError;
use nanobot_tools::ToolError;

use crate::heartbeat::HeartbeatError;
use crate::runtime::error::RuntimeError;
use nanobot_channels::ChannelError;

/// Convenience alias for `Result<T, NanobotError>`.
///
/// Used as the return type for most top-level fallible operations in the
/// application binary so that the `?` operator can convert any sub-crate
/// error into the unified `NanobotError` via the `From` impls.
pub type NanobotResult<T> = std::result::Result<T, NanobotError>;

/// Unified error type that aggregates errors from all sub-crates.
///
/// Each variant corresponds to a distinct subsystem. Most are transparent
/// wrappers around the sub-crate's own error type, enabled by `#[from]`
/// so that the `?` operator converts automatically.
#[derive(Debug, Error)]
pub enum NanobotError {
    /// Error from the LLM provider (e.g., Anthropic, OpenAI).
    #[error(transparent)]
    Provider(#[from] ProviderError),

    /// Error during tool execution.
    #[error(transparent)]
    Tool(#[from] ToolError),

    /// Error from a messaging channel (Feishu, Slack, etc.).
    #[error(transparent)]
    Channel(#[from] ChannelError),

    /// Error from the internal message bus.
    #[error(transparent)]
    Bus(#[from] BusError),

    /// Error from the session store (persistence / retrieval).
    #[error(transparent)]
    Session(#[from] SessionError),

    /// Error reading or writing configuration.
    #[error(transparent)]
    Config(#[from] ConfigError),

    /// Error from the cron scheduler subsystem.
    #[error(transparent)]
    Cron(#[from] CronError),

    /// Error from the heartbeat subsystem.
    #[error(transparent)]
    Heartbeat(#[from] HeartbeatError),

    /// Error from the agent loop.
    #[error(transparent)]
    Agent(#[from] AgentError),

    /// Error from runtime bootstrapping.
    #[error(transparent)]
    Runtime(#[from] RuntimeError),

    /// Standard I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    /// JSON serialisation / deserialisation error.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// Generic fallback for errors without a dedicated variant.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl NanobotError {
    /// Returns `true` if the error is potentially transient and may succeed on retry.
    ///
    /// Currently considers provider rate-limits, timeouts, API request failures,
    /// and I/O errors as retryable.
    #[allow(unused)]
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Provider(ProviderError::RateLimit(_))
                | Self::Provider(ProviderError::Timeout(_))
                | Self::Provider(ProviderError::ApiRequest(_))
                | Self::Io(_)
        )
    }
}
