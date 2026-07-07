//! Tool name type that distinguishes built-in tools from dynamic ones.
//!
//! This module provides [`ToolName`], an enum that represents a tool
//! name as either a known [`BuiltinTool`] variant or a dynamic string
//! (for MCP-based tools, user-defined tools, etc.).
//!
//! # Design
//!
//! - Parsing from a string (via `From<String>`) first tries
//!   [`BuiltinTool::from_str`]; if that fails, the string is stored as
//!   [`Dynamic`](ToolName::Dynamic). This ensures that built-in tools
//!   are always represented by their canonical variant.
//! - Custom `Serialize`/`Deserialize` impls map to/from plain strings
//!   for JSON compatibility.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

use crate::builtin::BuiltinTool;

/// A tool name that is either a known built-in tool or a dynamic (external) tool.
///
/// This is the canonical type for identifying tools in the system, used
/// in tool registries, routing logic, and LLM tool call requests.
///
/// # Variants
///
/// * `Builtin(BuiltinTool)` — A tool from the [`BuiltinTool`] enumeration
///   (e.g., `read_file`, `web_search`).
/// * `Dynamic(String)` — A tool registered dynamically, typically via MCP
///   or user configuration. The string is its name as exposed to the LLM.
///
/// # Examples
///
/// ```
/// use nanobot_types::tool_name::ToolName;
/// use nanobot_types::builtin::BuiltinTool;
///
/// // Built-in tools are parsed automatically:
/// let name = ToolName::from("read_file");
/// assert!(name.is_builtin());
///
/// // Unknown names become Dynamic:
/// let name = ToolName::from("my_custom_tool");
/// assert!(name.is_dynamic());
/// ```
///
/// # Derive rationale
///
/// - `Clone`: tool names are shared across tool registries and call chains.
/// - `PartialEq + Eq + Hash`: used as keys in tool lookup maps.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ToolName {
    Builtin(BuiltinTool),
    Dynamic(String),
}

impl ToolName {
    /// Returns the string representation of this tool name.
    ///
    /// For built-in tools, returns the canonical snake_case name (e.g.,
    /// `"read_file"`). For dynamic tools, returns the dynamic name string.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Builtin(tool) => tool.name(),
            Self::Dynamic(name) => name.as_str(),
        }
    }

    /// Returns `true` if this tool name refers to a built-in tool.
    ///
    /// Use this to decide whether tool routing should look up the
    /// implementation in the built-in registry.
    pub const fn is_builtin(&self) -> bool {
        matches!(self, Self::Builtin(_))
    }

    /// Returns `true` if this tool name refers to a dynamic (non-built-in) tool.
    ///
    /// Dynamic tools include MCP tools, user-registered tools, and any
    /// other tool not in the [`BuiltinTool`] enumeration.
    pub const fn is_dynamic(&self) -> bool {
        matches!(self, Self::Dynamic(_))
    }

    /// Returns the inner `BuiltinTool` if this is a built-in variant, otherwise `None`.
    ///
    /// Useful for pattern-matching on the specific built-in tool without
    /// destructuring the enum.
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

/// Converts a [`BuiltinTool`] into a [`ToolName::Builtin`] variant.
impl From<BuiltinTool> for ToolName {
    fn from(tool: BuiltinTool) -> Self {
        Self::Builtin(tool)
    }
}

/// Converts a `String` into a [`ToolName`].
///
/// First tries to parse the string as a [`BuiltinTool`]; if successful,
/// returns [`Builtin`](ToolName::Builtin). Otherwise, returns
/// [`Dynamic`](ToolName::Dynamic) with the original string.
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
    fn from(name: &str) -> Self {
        name.to_string().into()
    }
}

impl fmt::Display for ToolName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Serialises as a plain string: the canonical name for built-in tools
/// or the raw string for dynamic tools.
impl Serialize for ToolName {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// Deserialises from a plain string, using the same built-in-first
/// resolution as `From<String>`.
impl<'de> Deserialize<'de> for ToolName {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(Self::from(s))
    }
}
