//! Error types for tool execution and registry operations.
//!
//! Provides [`ToolError`], an enum covering all possible failure modes
//! in the tool system: execution failures, invalid arguments, "not found"
//! for missing tools, MCP server errors, and configuration errors.
//!
//! Also provides [`ToolResult<T>`] as a shorthand for `Result<T, ToolError>`
//! and the [`tool_error!`] macro for ergonomic error construction.

use thiserror::Error;

/// Errors returned by tool execution and registry operations.
///
/// Each variant carries enough context for the caller (typically the agent
/// loop) to produce a helpful error message for the LLM or log.
#[derive(Debug, Error)]
pub enum ToolError {
    /// Tool execution failed at runtime.
    ///
    /// This is the most general error variant, covering unexpected failures
    /// during tool execution (I/O errors, network timeouts, etc.).
    #[error("Tool '{tool_name}' execution failed: {source}")]
    Execution {
        /// The name of the tool that failed.
        tool_name: String,
        /// The underlying error cause.
        #[source]
        source: anyhow::Error,
    },

    /// Invalid tool arguments.
    ///
    /// Returned when the LLM provides arguments that do not match the
    /// tool's parameter schema, or when required fields are missing.
    #[error("Invalid tool arguments for '{tool_name}': {message}")]
    InvalidArgs {
        /// The name of the tool that received invalid args.
        tool_name: String,
        /// A human-readable description of what went wrong.
        message: String,
    },

    /// Tool not found in the registry.
    ///
    /// Returned when the agent tries to call a tool that hasn't been
    /// registered (e.g., a hallucinated tool name).
    #[error("Tool not found: {0}")]
    NotFound(String),

    /// MCP server error.
    ///
    /// Covers connection failures, protocol errors, and tool call failures
    /// from Model Context Protocol servers.
    #[error("MCP server '{server_name}' error: {message}")]
    McpServer {
        /// The name of the MCP server.
        server_name: String,
        /// A human-readable error description.
        message: String,
    },

    /// Tool configuration error.
    ///
    /// Returned when a tool's configuration is invalid or missing
    /// required settings.
    #[error("Tool configuration error: {0}")]
    Config(String),
}

/// Alias for `Result<T, ToolError>`.
pub type ToolResult<T> = std::result::Result<T, ToolError>;

impl ToolError {
    /// Creates an `Execution` error for a named tool.
    pub fn execution(tool_name: impl Into<String>, source: anyhow::Error) -> Self {
        Self::Execution {
            tool_name: tool_name.into(),
            source,
        }
    }

    /// Creates an `InvalidArgs` error for a named tool.
    pub fn invalid_args(tool_name: impl Into<String>, message: impl Into<String>) -> Self {
        Self::InvalidArgs {
            tool_name: tool_name.into(),
            message: message.into(),
        }
    }

    /// Creates a `NotFound` error for a missing tool name.
    pub fn not_found(name: impl Into<String>) -> Self {
        Self::NotFound(name.into())
    }

    /// Creates an `McpServer` error for an MCP server issue.
    pub fn mcp_server(server_name: impl Into<String>, message: impl Into<String>) -> Self {
        Self::McpServer {
            server_name: server_name.into(),
            message: message.into(),
        }
    }

    /// Creates a `Config` error for configuration issues.
    pub fn config(message: impl Into<String>) -> Self {
        Self::Config(message.into())
    }
}

/// Macro to create a tool execution error with automatic tool name capture.
///
/// Provides a concise shorthand for constructing `ToolError::Execution`
/// values. The first argument is the tool name, and the remaining arguments
/// form the error message (passed to `anyhow::anyhow!`).
///
/// # Examples
///
/// ```ignore
/// use nanobot_tools::tool_error;
///
/// // Simple message:
/// let err = tool_error!("read_file", "file not found");
///
/// // With formatting:
/// let path = "/tmp/test.txt";
/// let err = tool_error!("read_file", "failed to read {}: permission denied", path);
/// ```
#[macro_export]
macro_rules! tool_error {
    ($tool:expr, $msg:expr) => {
        $crate::error::ToolError::execution($tool, anyhow::anyhow!($msg))
    };
    ($tool:expr, $fmt:expr, $($arg:tt)*) => {
        $crate::error::ToolError::execution($tool, anyhow::anyhow!($fmt, $($arg)*))
    };
}
