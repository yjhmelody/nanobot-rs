//! Typed tool name representation that distinguishes built-in from dynamic tools.
//!
//! [`ToolName`] is an enum that captures whether a tool was defined at compile time
//! (built-in) or registered at runtime (dynamic, such as MCP tools). This distinction
//! enables compile-time checks against known tool names while still supporting
//! arbitrary tool names from external sources.
//!
//! # Note
//!
//! This type is marked with a `TODO: unused now` comment in the codebase; it may
//! be a candidate for removal if the dynamic tool path no longer needs it. However,
//! it remains `pub` for external consumers that may still reference it.

use serde::{Deserialize, Serialize};
use std::fmt;

use nanobot_types::builtin::BuiltinTool;

// TODO: unused now
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
    /// Converts a string into a `ToolName`, attempting to parse it as a built-in tool first.
    ///
    /// If the string matches one of [`BuiltinTool`]'s names (via `FromStr`), the result
    /// is `ToolName::Builtin`. Otherwise, it is `ToolName::Dynamic`.
    fn from(name: String) -> Self {
        // Try to parse as builtin first so that known tool names always resolve to
        // the Builtin variant, enabling compile-time name checks downstream.
        if let Ok(tool) = name.parse::<BuiltinTool>() {
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

// Manual Serialize impl needed because ToolName is an enum that serializes as a plain
// string (not a tagged or externally-tagged enum). This keeps the serialized form
// compatible with both built-in and dynamic names.
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
