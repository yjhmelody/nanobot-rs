//! Error types for the agent crate.
//!
//! Defines [`AgentError`] (the unified error enum for all agent components)
//! and the [`AgentResult`] type alias. Wraps errors from sessions and tools
//! via `From` impls so that `?` works seamlessly across crate boundaries.

use nanobot_session::SessionError;
use nanobot_tools::ToolError;
use thiserror::Error;

/// Errors raised by agent components: context assembly, the agent loop,
/// subagent spawning, session persistence, and tool execution.
#[derive(Debug, Error)]
pub enum AgentError {
    /// Error in the context builder (e.g. workspace initialisation).
    #[error("Context builder error: {0}")]
    ContextBuilder(String),

    /// Error in the agent loop or ReAct execution.
    #[error("Agent loop error: {0}")]
    Loop(String),

    /// Error in subagent spawning or execution.
    #[error("Subagent error: {0}")]
    Subagent(String),

    /// Error from the session store (transparently converted from
    /// [`SessionError`]).
    #[error("Session error: {0}")]
    Session(#[from] SessionError),

    /// Error from the tool registry or a tool execution (transparently
    /// converted from [`ToolError`]).
    #[error("Tool error: {0}")]
    Tool(#[from] ToolError),

    /// Catch-all for any other error types, wrapped as [`anyhow::Error`].
    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

/// Convenience alias for `Result<T, AgentError>`.
pub type AgentResult<T> = std::result::Result<T, AgentError>;

impl AgentError {
    /// Creates a [`ContextBuilder`](crate::context::ContextBuilder) error
    /// with the given message.
    pub fn context_builder(message: impl Into<String>) -> Self {
        Self::ContextBuilder(message.into())
    }

    /// Creates a loop error with the given message.
    ///
    /// Used by the ReAct executor and the main message-processing pipeline.
    pub fn loop_error(message: impl Into<String>) -> Self {
        Self::Loop(message.into())
    }

    /// Creates a subagent error with the given message.
    pub fn subagent(message: impl Into<String>) -> Self {
        Self::Subagent(message.into())
    }
}
