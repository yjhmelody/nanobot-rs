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
}

pub type AgentResult<T> = std::result::Result<T, AgentError>;

impl AgentError {
    pub fn context_builder(message: impl Into<String>) -> Self {
        Self::ContextBuilder(message.into())
    }

    pub fn loop_error(message: impl Into<String>) -> Self {
        Self::Loop(message.into())
    }

    pub fn subagent(message: impl Into<String>) -> Self {
        Self::Subagent(message.into())
    }
}
