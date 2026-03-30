use nanobot_session::SessionError;
use nanobot_tools::ToolError;
use thiserror::Error;

/// Errors raised by agent components (context, loop, subagent).
#[derive(Debug, Error)]
pub enum AgentError {
    /// Context builder error.
    #[error("Context builder error: {0}")]
    ContextBuilder(String),

    /// Agent loop error.
    #[error("Agent loop error: {0}")]
    Loop(String),

    /// Subagent error.
    #[error("Subagent error: {0}")]
    Subagent(String),

    /// Session error.
    #[error("Session error: {0}")]
    Session(#[from] SessionError),

    /// Tool error.
    #[error("Tool error: {0}")]
    Tool(#[from] ToolError),

    /// Generic error.
    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

/// Convenience alias for `Result<T, AgentError>`.
pub type AgentResult<T> = std::result::Result<T, AgentError>;

impl AgentError {
    /// Creates a `ContextBuilder` error with the given message.
    pub fn context_builder(message: impl Into<String>) -> Self {
        Self::ContextBuilder(message.into())
    }

    /// Creates a `Loop` error with the given message.
    pub fn loop_error(message: impl Into<String>) -> Self {
        Self::Loop(message.into())
    }

    /// Creates a `Subagent` error with the given message.
    pub fn subagent(message: impl Into<String>) -> Self {
        Self::Subagent(message.into())
    }
}
