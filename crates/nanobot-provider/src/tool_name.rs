use serde::{Deserialize, Serialize};
use std::fmt;

use nanobot_types::BuiltinTool;

/// Represents a tool name, either built-in or dynamic.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ToolName {
    /// A built-in tool with compile-time checking.
    Builtin(BuiltinTool),
    /// A dynamically registered tool (MCP, custom, etc.).
    Dynamic(String),
}

impl ToolName {
    /// Returns the tool name as a string.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Builtin(tool) => tool.name(),
            Self::Dynamic(name) => name.as_str(),
        }
    }

    /// Checks if this is a built-in tool.
    pub const fn is_builtin(&self) -> bool {
        matches!(self, Self::Builtin(_))
    }

    /// Checks if this is a dynamic tool.
    pub const fn is_dynamic(&self) -> bool {
        matches!(self, Self::Dynamic(_))
    }

    /// Tries to get the built-in tool variant.
    pub const fn as_builtin(&self) -> Option<&BuiltinTool> {
        match self {
            Self::Builtin(tool) => Some(tool),
            Self::Dynamic(_) => None,
        }
    }

    /// Tries to get the dynamic tool name.
    pub fn as_dynamic(&self) -> Option<&str> {
        match self {
            Self::Builtin(_) => None,
            Self::Dynamic(name) => Some(name.as_str()),
        }
    }
}

impl From<BuiltinTool> for ToolName {
    fn from(tool: BuiltinTool) -> Self {
        Self::Builtin(tool)
    }
}

impl From<String> for ToolName {
    fn from(name: String) -> Self {
        use std::str::FromStr;

        // Try to parse as builtin first
        if let Ok(tool) = BuiltinTool::from_str(&name) {
            Self::Builtin(tool)
        } else {
            Self::Dynamic(name)
        }
    }
}

impl From<&str> for ToolName {
    fn from(name: &str) -> Self {
        name.to_string().into()
    }
}

impl fmt::Display for ToolName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl Serialize for ToolName {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ToolName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let name = String::deserialize(deserializer)?;
        Ok(Self::from(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_name_from_builtin_string() {
        let name: ToolName = "read_file".into();
        assert!(name.is_builtin());
        assert_eq!(name.as_str(), "read_file");
        assert_eq!(name.as_builtin(), Some(&BuiltinTool::ReadFile));
    }

    #[test]
    fn tool_name_from_dynamic_string() {
        let name: ToolName = "custom_tool".into();
        assert!(name.is_dynamic());
        assert_eq!(name.as_str(), "custom_tool");
        assert_eq!(name.as_dynamic(), Some("custom_tool"));
    }

    #[test]
    fn tool_name_from_builtin_enum() {
        let name = ToolName::from(BuiltinTool::Exec);
        assert!(name.is_builtin());
        assert_eq!(name.as_str(), "exec");
    }

    #[test]
    fn tool_name_display() {
        let builtin = ToolName::from(BuiltinTool::WebSearch);
        assert_eq!(format!("{}", builtin), "web_search");

        let dynamic = ToolName::Dynamic("custom".to_string());
        assert_eq!(format!("{}", dynamic), "custom");
    }

    #[test]
    fn tool_name_serialization() {
        let name = ToolName::from(BuiltinTool::ReadFile);
        let json = serde_json::to_string(&name).unwrap();
        assert_eq!(json, "\"read_file\"");
    }

    #[test]
    fn tool_name_deserialization() {
        let json = "\"exec\"";
        let name: ToolName = serde_json::from_str(json).unwrap();
        assert!(name.is_builtin());
        assert_eq!(name.as_builtin(), Some(&BuiltinTool::Exec));
    }

    #[test]
    fn tool_name_deserialization_dynamic() {
        let json = "\"custom_tool\"";
        let name: ToolName = serde_json::from_str(json).unwrap();
        assert!(name.is_dynamic());
        assert_eq!(name.as_dynamic(), Some("custom_tool"));
    }
}
