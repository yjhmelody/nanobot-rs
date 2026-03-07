use std::io;

use thiserror::Error;

/// Result type alias using NanobotError.
pub type Result<T> = std::result::Result<T, NanobotError>;

/// Core error types for nanobot-rs.
///
/// This provides type-safe error handling with specific error variants
/// that can be matched and handled differently.
#[derive(Debug, Error)]
pub enum NanobotError {
    /// Tool execution failed.
    #[error("Tool '{tool_name}' execution failed: {source}")]
    ToolExecution {
        tool_name: String,
        #[source]
        source: anyhow::Error,
    },

    /// LLM provider error.
    #[error("LLM provider error: {0}")]
    Provider(#[from] ProviderError),

    /// Configuration error.
    #[error("Configuration error: {0}")]
    Config(String),

    /// Session not found.
    #[error("Session not found: {0}")]
    SessionNotFound(String),

    /// Session operation failed.
    #[error("Session operation failed: {0}")]
    SessionOperation(#[source] anyhow::Error),

    /// I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    /// JSON serialization/deserialization error.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// Invalid tool arguments.
    #[error("Invalid tool arguments for '{tool_name}': {message}")]
    InvalidToolArgs { tool_name: String, message: String },

    /// Tool not found.
    #[error("Tool not found: {0}")]
    ToolNotFound(String),

    /// MCP server error.
    #[error("MCP server '{server_name}' error: {message}")]
    McpServer {
        server_name: String,
        message: String,
    },

    /// Context builder error.
    #[error("Context builder error: {0}")]
    ContextBuilder(String),

    /// Runtime error.
    #[error("Runtime error: {0}")]
    Runtime(String),

    /// Agent loop error.
    #[error("Agent loop error: {0}")]
    AgentLoop(String),

    /// Subagent error.
    #[error("Subagent error: {0}")]
    Subagent(String),

    /// Generic error for cases not covered by specific variants.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// LLM provider specific errors.
#[derive(Debug, Error)]
pub enum ProviderError {
    /// API request failed.
    #[error("API request failed: {0}")]
    ApiRequest(#[from] reqwest::Error),

    /// Invalid API response.
    #[error("Invalid API response: {0}")]
    InvalidResponse(String),

    /// Authentication failed.
    #[error("Authentication failed: {0}")]
    Authentication(String),

    /// Rate limit exceeded.
    #[error("Rate limit exceeded: {0}")]
    RateLimit(String),

    /// Model not found or not available.
    #[error("Model not available: {0}")]
    ModelNotAvailable(String),

    /// Invalid model configuration.
    #[error("Invalid model configuration: {0}")]
    InvalidConfig(String),

    /// Request timeout.
    #[error("Request timeout after {0}s")]
    Timeout(u64),

    /// Generic provider error.
    #[error("Provider error: {0}")]
    Other(String),
}

impl NanobotError {
    /// Creates a tool execution error.
    pub fn tool_execution(tool_name: impl Into<String>, source: anyhow::Error) -> Self {
        Self::ToolExecution {
            tool_name: tool_name.into(),
            source,
        }
    }

    /// Creates an invalid tool arguments error.
    pub fn invalid_tool_args(tool_name: impl Into<String>, message: impl Into<String>) -> Self {
        Self::InvalidToolArgs {
            tool_name: tool_name.into(),
            message: message.into(),
        }
    }

    /// Creates an MCP server error.
    pub fn mcp_server(server_name: impl Into<String>, message: impl Into<String>) -> Self {
        Self::McpServer {
            server_name: server_name.into(),
            message: message.into(),
        }
    }

    /// Creates a configuration error.
    pub fn config(message: impl Into<String>) -> Self {
        Self::Config(message.into())
    }

    /// Creates an agent loop error.
    pub fn agent_loop(message: impl Into<String>) -> Self {
        Self::AgentLoop(message.into())
    }

    /// Creates a subagent error.
    pub fn subagent(message: impl Into<String>) -> Self {
        Self::Subagent(message.into())
    }

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
        matches!(
            self,
            Self::ToolExecution { .. } | Self::InvalidToolArgs { .. } | Self::ToolNotFound(_)
        )
    }

    /// Returns the error chain as a vector of error messages.
    ///
    /// This is useful for debugging and logging the full context of an error.
    pub fn error_chain(&self) -> Vec<String> {
        let mut chain = vec![self.to_string()];

        // Add source errors
        let mut current: &dyn std::error::Error = self;
        while let Some(source) = current.source() {
            chain.push(source.to_string());
            current = source;
        }

        chain
    }

    /// Returns a formatted error message with full context.
    ///
    /// This includes the error chain and any additional context.
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

    /// Returns the error category for metrics and logging.
    pub fn category(&self) -> &'static str {
        match self {
            Self::ToolExecution { .. } | Self::InvalidToolArgs { .. } | Self::ToolNotFound(_) => {
                "tool"
            }
            Self::Provider(_) => "provider",
            Self::Config(_) => "config",
            Self::SessionNotFound(_) | Self::SessionOperation(_) => "session",
            Self::Io(_) => "io",
            Self::Json(_) => "json",
            Self::McpServer { .. } => "mcp",
            Self::ContextBuilder(_) => "context",
            Self::Runtime(_) => "runtime",
            Self::AgentLoop(_) => "agent_loop",
            Self::Subagent(_) => "subagent",
            Self::Other(_) => "other",
        }
    }
}

impl ProviderError {
    /// Creates a rate limit error.
    pub fn rate_limit(message: impl Into<String>) -> Self {
        Self::RateLimit(message.into())
    }

    /// Creates a timeout error.
    pub fn timeout(seconds: u64) -> Self {
        Self::Timeout(seconds)
    }

    /// Creates an authentication error.
    pub fn authentication(message: impl Into<String>) -> Self {
        Self::Authentication(message.into())
    }

    /// Creates an invalid response error.
    pub fn invalid_response(message: impl Into<String>) -> Self {
        Self::InvalidResponse(message.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_execution_error_displays_correctly() {
        let err = NanobotError::tool_execution("read_file", anyhow::anyhow!("file not found"));
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

        let config = NanobotError::Config("test".to_string());
        assert!(!config.is_retryable());
    }

    #[test]
    fn tool_errors_are_identified() {
        let tool_exec = NanobotError::tool_execution("exec", anyhow::anyhow!("failed"));
        assert!(tool_exec.is_tool_error());

        let invalid_args = NanobotError::invalid_tool_args("exec", "bad json");
        assert!(invalid_args.is_tool_error());

        let not_found = NanobotError::ToolNotFound("unknown".to_string());
        assert!(not_found.is_tool_error());

        let config = NanobotError::Config("test".to_string());
        assert!(!config.is_tool_error());
    }

    #[test]
    fn error_chain_captures_nested_errors() {
        let inner = anyhow::anyhow!("inner error");
        let err = NanobotError::tool_execution("test_tool", inner);

        let chain = err.error_chain();
        assert!(chain.len() >= 1);
        assert!(chain[0].contains("test_tool"));
    }

    #[test]
    fn detailed_message_includes_context() {
        let inner = anyhow::anyhow!("root cause");
        let err = NanobotError::tool_execution("test_tool", inner);

        let detailed = err.detailed_message();
        assert!(detailed.contains("test_tool"));
    }

    #[test]
    fn error_category_is_correct() {
        let tool_err = NanobotError::ToolNotFound("test".to_string());
        assert_eq!(tool_err.category(), "tool");

        let provider_err = NanobotError::Provider(ProviderError::timeout(30));
        assert_eq!(provider_err.category(), "provider");

        let config_err = NanobotError::config("test");
        assert_eq!(config_err.category(), "config");

        let session_err = NanobotError::SessionNotFound("test".to_string());
        assert_eq!(session_err.category(), "session");

        let agent_err = NanobotError::agent_loop("test");
        assert_eq!(agent_err.category(), "agent_loop");

        let subagent_err = NanobotError::subagent("test");
        assert_eq!(subagent_err.category(), "subagent");
    }

    #[test]
    fn helper_constructors_work() {
        let config_err = NanobotError::config("invalid config");
        assert!(matches!(config_err, NanobotError::Config(_)));

        let agent_err = NanobotError::agent_loop("loop failed");
        assert!(matches!(agent_err, NanobotError::AgentLoop(_)));

        let subagent_err = NanobotError::subagent("subagent failed");
        assert!(matches!(subagent_err, NanobotError::Subagent(_)));

        let provider_err = ProviderError::invalid_response("bad json");
        assert!(matches!(provider_err, ProviderError::InvalidResponse(_)));
    }
}
