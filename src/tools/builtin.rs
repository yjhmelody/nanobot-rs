use std::fmt;
use std::str::FromStr;

/// Enumeration of built-in tools.
///
/// This provides compile-time checking of tool names and eliminates
/// string matching errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuiltinTool {
    /// Read file contents
    ReadFile,
    /// Write file contents
    WriteFile,
    /// Edit file contents
    EditFile,
    /// List directory contents
    ListDir,
    /// Execute shell command
    Exec,
    /// Search the web
    WebSearch,
    /// Fetch web content
    WebFetch,
    /// Send message to user
    Message,
    /// Spawn subagent
    Spawn,
    /// Schedule cron job
    Cron,
}

impl BuiltinTool {
    /// Returns the tool name as a string.
    pub const fn name(&self) -> &'static str {
        match self {
            Self::ReadFile => "read_file",
            Self::WriteFile => "write_file",
            Self::EditFile => "edit_file",
            Self::ListDir => "list_dir",
            Self::Exec => "exec",
            Self::WebSearch => "web_search",
            Self::WebFetch => "web_fetch",
            Self::Message => "message",
            Self::Spawn => "spawn",
            Self::Cron => "cron",
        }
    }

    /// Returns all core tools (excluding optional tools).
    pub const fn core_tools() -> &'static [BuiltinTool] {
        &[
            Self::ReadFile,
            Self::WriteFile,
            Self::EditFile,
            Self::ListDir,
            Self::Exec,
            Self::WebSearch,
            Self::WebFetch,
            Self::Message,
            Self::Spawn,
            Self::Cron,
        ]
    }
}

impl fmt::Display for BuiltinTool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

impl FromStr for BuiltinTool {
    type Err = UnknownToolError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "read_file" => Ok(Self::ReadFile),
            "write_file" => Ok(Self::WriteFile),
            "edit_file" => Ok(Self::EditFile),
            "list_dir" => Ok(Self::ListDir),
            "exec" => Ok(Self::Exec),
            "web_search" => Ok(Self::WebSearch),
            "web_fetch" => Ok(Self::WebFetch),
            "message" => Ok(Self::Message),
            "spawn" => Ok(Self::Spawn),
            "cron" => Ok(Self::Cron),
            _ => Err(UnknownToolError(s.to_string())),
        }
    }
}

/// Error returned when parsing an unknown tool name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownToolError(pub String);

impl fmt::Display for UnknownToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown builtin tool: {}", self.0)
    }
}

impl std::error::Error for UnknownToolError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_str_parses_valid_tool_names() {
        assert_eq!(
            "read_file".parse::<BuiltinTool>().unwrap(),
            BuiltinTool::ReadFile
        );
        assert_eq!("exec".parse::<BuiltinTool>().unwrap(), BuiltinTool::Exec);
        assert_eq!("spawn".parse::<BuiltinTool>().unwrap(), BuiltinTool::Spawn);
    }

    #[test]
    fn from_str_rejects_invalid_tool_names() {
        let result = "invalid_tool".parse::<BuiltinTool>();
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            "unknown builtin tool: invalid_tool"
        );
    }

    #[test]
    fn display_formats_as_tool_name() {
        assert_eq!(BuiltinTool::ReadFile.to_string(), "read_file");
        assert_eq!(BuiltinTool::WebFetch.to_string(), "web_fetch");
    }

    #[test]
    fn all_tools_have_unique_names() {
        use std::collections::HashSet;
        let mut names = HashSet::new();

        for tool in BuiltinTool::core_tools() {
            assert!(
                names.insert(tool.name()),
                "duplicate tool name: {}",
                tool.name()
            );
        }
    }
}
