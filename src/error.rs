use std::io;

use thiserror::Error;

use crate::agent::AgentError;
use crate::bus::BusError;
use crate::channels::ChannelError;
use crate::config::ConfigError;
use crate::cron::CronError;
use crate::heartbeat::HeartbeatError;
use crate::prompt::PromptError;
use crate::provider::ProviderError;
use crate::runtime::RuntimeError;
use crate::session::SessionError;
use crate::tools::ToolError;

/// Result type alias using NanobotError.
pub type NanobotResult<T> = std::result::Result<T, NanobotError>;

/// Core error types for nanobot-rs.
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

    /// Prompt system error.
    #[error(transparent)]
    Prompt(#[from] PromptError),

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
    /// Checks if this error is retryable.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Provider(ProviderError::RateLimit(_)) => true,
            Self::Provider(ProviderError::Timeout(_)) => true,
            Self::Provider(ProviderError::ApiRequest(_)) => true,
            Self::Io(_) => true,
            _ => false,
        }
    }

    /// Checks if this error is a tool error.
    pub fn is_tool_error(&self) -> bool {
        matches!(self, Self::Tool(_))
    }

    /// Returns the error chain as a vector of error messages.
    pub fn error_chain(&self) -> Vec<String> {
        let mut chain = vec![self.to_string()];

        let mut current: &dyn std::error::Error = self;
        while let Some(source) = current.source() {
            chain.push(source.to_string());
            current = source;
        }

        chain
    }

    /// Returns a formatted error message with full context.
    pub fn detailed_message(&self) -> String {
        let chain = self.error_chain();
        if chain.len() == 1 {
            chain[0].clone()
        } else {
            format!(
                "{}\n\nCaused by:\n{}",
                chain[0],
                chain[1..]
                    .iter()
                    .enumerate()
                    .map(|(i, msg)| format!("  {}: {}", i + 1, msg))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::ToolError;

    #[test]
    fn tool_execution_error_displays_correctly() {
        let err = NanobotError::from(ToolError::execution(
            "read_file",
            anyhow::anyhow!("file not found"),
        ));
        let msg = err.to_string();
        assert!(msg.contains("read_file"));
        assert!(msg.contains("file not found"));
    }

    #[test]
    fn provider_error_converts_to_nanobot_error() {
        let provider_err = ProviderError::rate_limit("too many requests");
        let err: NanobotError = provider_err.into();
        assert!(matches!(err, NanobotError::Provider(_)));
    }

    #[test]
    fn retryable_errors_are_identified() {
        let rate_limit = NanobotError::Provider(ProviderError::rate_limit("test"));
        assert!(rate_limit.is_retryable());

        let timeout = NanobotError::Provider(ProviderError::timeout(30));
        assert!(timeout.is_retryable());

        let config = NanobotError::Config(ConfigError::invalid("test"));
        assert!(!config.is_retryable());
    }

    #[test]
    fn tool_errors_are_identified() {
        let tool_exec = NanobotError::from(ToolError::execution("exec", anyhow::anyhow!("failed")));
        assert!(tool_exec.is_tool_error());

        let invalid_args = NanobotError::from(ToolError::invalid_args("exec", "bad json"));
        assert!(invalid_args.is_tool_error());

        let not_found = NanobotError::from(ToolError::not_found("unknown"));
        assert!(not_found.is_tool_error());

        let config = NanobotError::Config(ConfigError::invalid("test"));
        assert!(!config.is_tool_error());
    }

    #[test]
    fn error_chain_captures_nested_errors() {
        let inner = anyhow::anyhow!("inner error");
        let err = NanobotError::from(ToolError::execution("test_tool", inner));

        let chain = err.error_chain();
        assert!(chain.len() >= 1);
        assert!(chain[0].contains("test_tool"));
    }

    #[test]
    fn detailed_message_includes_context() {
        let inner = anyhow::anyhow!("root cause");
        let err = NanobotError::from(ToolError::execution("test_tool", inner));

        let detailed = err.detailed_message();
        assert!(detailed.contains("test_tool"));
    }
}
