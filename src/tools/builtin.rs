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
    /// Spawn subagent (optional)
    Spawn,
    /// Schedule cron job (optional)
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
        ]
    }

    /// Returns all optional tools.
    pub const fn optional_tools() -> &'static [BuiltinTool] {
        &[Self::Spawn, Self::Cron]
    }

    /// Returns all filesystem tools.
    pub const fn filesystem_tools() -> &'static [BuiltinTool] {
        &[
            Self::ReadFile,
            Self::WriteFile,
            Self::EditFile,
            Self::ListDir,
        ]
    }

    /// Returns all web tools.
    pub const fn web_tools() -> &'static [BuiltinTool] {
        &[Self::WebSearch, Self::WebFetch]
    }

    /// Checks if this is a filesystem tool.
    pub const fn is_filesystem_tool(&self) -> bool {
        matches!(
            self,
            Self::ReadFile | Self::WriteFile | Self::EditFile | Self::ListDir
        )
    }

    /// Checks if this is a web tool.
    pub const fn is_web_tool(&self) -> bool {
        matches!(self, Self::WebSearch | Self::WebFetch)
    }

    /// Checks if this is an optional tool.
    pub const fn is_optional(&self) -> bool {
        matches!(self, Self::Spawn | Self::Cron)
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
    fn tool_name_returns_correct_string() {
        assert_eq!(BuiltinTool::ReadFile.name(), "read_file");
        assert_eq!(BuiltinTool::Exec.name(), "exec");
        assert_eq!(BuiltinTool::WebSearch.name(), "web_search");
    }

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
    fn core_tools_excludes_optional() {
        let core = BuiltinTool::core_tools();
        assert!(!core.contains(&BuiltinTool::Spawn));
        assert!(!core.contains(&BuiltinTool::Cron));
        assert!(core.contains(&BuiltinTool::ReadFile));
        assert!(core.contains(&BuiltinTool::Message));
    }

    #[test]
    fn optional_tools_only_includes_spawn_and_cron() {
        let optional = BuiltinTool::optional_tools();
        assert_eq!(optional.len(), 2);
        assert!(optional.contains(&BuiltinTool::Spawn));
        assert!(optional.contains(&BuiltinTool::Cron));
    }

    #[test]
    fn filesystem_tools_classification() {
        assert!(BuiltinTool::ReadFile.is_filesystem_tool());
        assert!(BuiltinTool::WriteFile.is_filesystem_tool());
        assert!(BuiltinTool::EditFile.is_filesystem_tool());
        assert!(BuiltinTool::ListDir.is_filesystem_tool());
        assert!(!BuiltinTool::Exec.is_filesystem_tool());
        assert!(!BuiltinTool::WebSearch.is_filesystem_tool());
    }

    #[test]
    fn web_tools_classification() {
        assert!(BuiltinTool::WebSearch.is_web_tool());
        assert!(BuiltinTool::WebFetch.is_web_tool());
        assert!(!BuiltinTool::ReadFile.is_web_tool());
        assert!(!BuiltinTool::Exec.is_web_tool());
    }

    #[test]
    fn optional_tools_classification() {
        assert!(BuiltinTool::Spawn.is_optional());
        assert!(BuiltinTool::Cron.is_optional());
        assert!(!BuiltinTool::ReadFile.is_optional());
        assert!(!BuiltinTool::Message.is_optional());
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

        for tool in BuiltinTool::optional_tools() {
            assert!(
                names.insert(tool.name()),
                "duplicate tool name: {}",
                tool.name()
            );
        }
    }
}
