//! Built-in tool identifier enum and parsing.
//!
//! This module defines [`BuiltinTool`], the canonical enumeration of all
//! tools that ship with the nanobot agent. Each variant corresponds to a
//! specific tool implementation in the `nanobot-tools` crate.
//!
//! # Design
//!
//! - The enum is `#[non_exhaustive]`-adjacent by design (new variants are
//!   added to the end). The [`core_tools`](BuiltinTool::core_tools) method
//!   is the single source of truth for iterating all variants.
//! - String representation mirrors the snake_case names used in LLM tool
//!   definitions and JSON config files.
//! - Parsing via [`FromStr`] is case-sensitive (lowercase only).

use std::fmt;
use std::str::FromStr;

/// Enumeration of all built-in tools available in the nanobot agent.
///
/// Each variant represents a first-party tool implementation. The string
/// name (returned by [`name`](BuiltinTool::name)) is used as the identifier
/// in tool registries, LLM tool definitions, and JSON serialisation.
///
/// # Variants
///
/// | Variant | Name | Purpose |
/// |---------|------|---------|
/// | `ReadFile` | `read_file` | Read file contents |
/// | `WriteFile` | `write_file` | Write content to a file |
/// | `EditFile` | `edit_file` | Apply a text replacement |
/// | `ListDir` | `list_dir` | List directory entries |
/// | `Exec` | `exec` | Run a shell command |
/// | `WebSearch` | `web_search` | Search the web |
/// | `WebFetch` | `web_fetch` | Fetch a URL |
/// | `Spawn` | `spawn` | Spawn a sub-agent |
/// | `Cron` | `cron` | Manage scheduled jobs |
///
/// # Derive rationale
///
/// - `Clone + Copy`: small enum, often passed by value.
/// - `PartialEq + Eq + Hash`: used as hash map keys and for deduplication.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuiltinTool {
    // The snake_case name constants ensure a single source of truth for string
    // representations, avoiding typos in tool routing and JSON serialisation.
    ReadFile,
    WriteFile,
    EditFile,
    ListDir,
    Exec,
    WebSearch,
    WebFetch,
    Spawn,
    Cron,
}

impl BuiltinTool {
    /// Returns the canonical snake_case string name for this tool.
    ///
    /// This name is used as the identifier in:
    /// - LLM tool definitions (JSON schema `name` field)
    /// - Tool registry lookups
    /// - [`ToolName`](crate::tool_name::ToolName) parsing
    ///
    /// # Examples
    ///
    /// ```
    /// use nanobot_types::builtin::BuiltinTool;
    /// assert_eq!(BuiltinTool::ReadFile.name(), "read_file");
    /// assert_eq!(BuiltinTool::Exec.name(), "exec");
    /// ```
    pub const fn name(&self) -> &'static str {
        match self {
            Self::ReadFile => "read_file",
            Self::WriteFile => "write_file",
            Self::EditFile => "edit_file",
            Self::ListDir => "list_dir",
            Self::Exec => "exec",
            Self::WebSearch => "web_search",
            Self::WebFetch => "web_fetch",
            Self::Spawn => "spawn",
            Self::Cron => "cron",
        }
    }

    /// Returns a slice of all built-in tool variants.
    ///
    /// This is the single source of truth for iterating every known built-in
    /// tool. It is used during registration and validation.
    ///
    /// # Examples
    ///
    /// ```
    /// use nanobot_types::builtin::BuiltinTool;
    /// assert_eq!(BuiltinTool::core_tools().len(), 9);
    /// ```
    pub const fn core_tools() -> &'static [BuiltinTool] {
        &[
            Self::ReadFile,
            Self::WriteFile,
            Self::EditFile,
            Self::ListDir,
            Self::Exec,
            Self::WebSearch,
            Self::WebFetch,
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

/// Error returned when a string cannot be parsed as a known built-in tool name.
///
/// Returned by [`BuiltinTool::from_str`] when the input does not match any
/// variant in [`BuiltinTool`].
///
/// # Fields
///
/// * `0` — The input string that could not be matched.
#[derive(Debug)]
pub struct UnknownToolError(pub String);

impl fmt::Display for UnknownToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown built-in tool: {}", self.0)
    }
}

impl std::error::Error for UnknownToolError {}

/// Parses a snake_case string into a [`BuiltinTool`] variant.
///
/// The match is case-sensitive and requires an exact string match against
/// the canonical names (e.g., `"read_file"`, `"web_search"`).
///
/// # Errors
///
/// Returns [`UnknownToolError`] if the string does not match any known tool name.
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
            assert!(
                names.insert(tool.name()),
                "duplicate tool name: {}",
                tool.name()
            );
        }
    }
}
