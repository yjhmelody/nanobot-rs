use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

use crate::builtin::BuiltinTool;

/// Represents a tool name, either built-in or dynamic.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ToolName {
    Builtin(BuiltinTool),
    Dynamic(String),
}

impl ToolName {
    /// Returns the string representation of this tool name.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Builtin(tool) => tool.name(),
            Self::Dynamic(name) => name.as_str(),
        }
    }

    /// Returns `true` if this tool name refers to a built-in tool.
    pub const fn is_builtin(&self) -> bool {
        matches!(self, Self::Builtin(_))
    }

    /// Returns `true` if this tool name refers to a dynamic (non-built-in) tool.
    pub const fn is_dynamic(&self) -> bool {
        matches!(self, Self::Dynamic(_))
    }

    /// Returns the inner `BuiltinTool` if this is a built-in variant, otherwise `None`.
    pub const fn as_builtin(&self) -> Option<&BuiltinTool> {
        match self {
            Self::Builtin(tool) => Some(tool),
            Self::Dynamic(_) => None,
        }
    }

    /// Returns the dynamic name string if this is a dynamic variant, otherwise `None`.
    pub fn as_dynamic(&self) -> Option<&str> {
        match self {
            Self::Builtin(_) => None,
            Self::Dynamic(name) => Some(name.as_str()),
        }
    }
}

impl From<BuiltinTool> for ToolName {
    fn from(tool: BuiltinTool) -> Self { Self::Builtin(tool) }
}

impl From<String> for ToolName {
    fn from(name: String) -> Self {
        if let Ok(tool) = BuiltinTool::from_str(&name) {
            Self::Builtin(tool)
        } else {
            Self::Dynamic(name)
        }
    }
}

impl From<&str> for ToolName {
    fn from(name: &str) -> Self { name.to_string().into() }
}

impl fmt::Display for ToolName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl Serialize for ToolName {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ToolName {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(Self::from(s))
    }
}
