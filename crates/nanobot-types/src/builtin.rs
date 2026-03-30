use std::fmt;
use std::str::FromStr;

/// Enumeration of built-in tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuiltinTool {
    ReadFile,
    WriteFile,
    EditFile,
    ListDir,
    Exec,
    WebSearch,
    WebFetch,
    Message,
    Spawn,
    Cron,
}

impl BuiltinTool {
    /// Returns the canonical string name used to identify this tool in tool registries.
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

    /// Returns the full slice of all built-in tools.
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

/// Error returned when a string cannot be parsed as a known built-in tool.
#[derive(Debug)]
pub struct UnknownToolError(pub String);

impl fmt::Display for UnknownToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown built-in tool: {}", self.0)
    }
}

impl std::error::Error for UnknownToolError {}

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
            other => Err(UnknownToolError(other.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn all_tool_names_are_unique() {
        let mut names = HashSet::new();
        for tool in BuiltinTool::core_tools() {
            assert!(names.insert(tool.name()), "duplicate tool name: {}", tool.name());
        }
    }
}
