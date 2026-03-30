use thiserror::Error;

/// Errors returned by tool execution and registry operations.
#[derive(Debug, Error)]
pub enum ToolError {
    /// Tool execution failed.
    #[error("Tool '{tool_name}' execution failed: {source}")]
    Execution {
        tool_name: String,
        #[source]
        source: anyhow::Error,
    },

    /// Invalid tool arguments.
    #[error("Invalid tool arguments for '{tool_name}': {message}")]
    InvalidArgs { tool_name: String, message: String },

    /// Tool not found.
    #[error("Tool not found: {0}")]
    NotFound(String),

    /// MCP server error.
    #[error("MCP server '{server_name}' error: {message}")]
    McpServer { server_name: String, message: String },

    /// Tool configuration error.
    #[error("Tool configuration error: {0}")]
    Config(String),
}

pub type ToolResult<T> = std::result::Result<T, ToolError>;

impl ToolError {
    pub fn execution(tool_name: impl Into<String>, source: anyhow::Error) -> Self {
        Self::Execution {
            tool_name: tool_name.into(),
            source,
        }
    }

    pub fn invalid_args(tool_name: impl Into<String>, message: impl Into<String>) -> Self {
        Self::InvalidArgs {
            tool_name: tool_name.into(),
            message: message.into(),
        }
    }

    pub fn not_found(name: impl Into<String>) -> Self {
        Self::NotFound(name.into())
    }

    pub fn mcp_server(server_name: impl Into<String>, message: impl Into<String>) -> Self {
        Self::McpServer {
            server_name: server_name.into(),
            message: message.into(),
        }
    }

    pub fn config(message: impl Into<String>) -> Self {
        Self::Config(message.into())
    }
}

/// Macro to create a tool execution error with automatic tool name capture.
#[macro_export]
macro_rules! tool_error {
    ($tool:expr, $msg:expr) => {
        $crate::error::ToolError::execution($tool, anyhow::anyhow!($msg))
    };
    ($tool:expr, $fmt:expr, $($arg:tt)*) => {
        $crate::error::ToolError::execution($tool, anyhow::anyhow!($fmt, $($arg)*))
    };
}
