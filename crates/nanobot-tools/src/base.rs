//! Core tool trait and foundational helpers for the nanobot tool system.
//!
//! This module defines the [`Tool`] trait, which every tool in the system
//! must implement. It also provides utility functions for parsing tool
//! arguments, building JSON Schema property maps, and constructing
//! [`ToolDefinition`] values from `serde_json::Value`.
//!
//! ## Re-exports
//!
//! Key types from `nanobot_types::tools` are re-exported here for
//! convenience: [`ToolContext`], [`ToolDefinition`], [`ToolFunction`],
//! [`JsonSchema`], [`JsonSchemaType`].

use std::collections::BTreeMap;

use async_trait::async_trait;
use serde::de::DeserializeOwned;

use crate::error::{ToolError, ToolResult};
pub use nanobot_types::tools::{
    JsonSchema, JsonSchemaType, ToolContext, ToolDefinition, ToolFunction,
};

/// Runtime contract for all agent tools.
///
/// Every tool in the system -- whether built-in (filesystem, shell, web),
/// dynamically registered (MCP), or user-provided -- must implement this
/// trait. The trait is `Send + Sync` so tools can be shared across threads
/// and called from async contexts safely.
///
/// # Required Methods
///
/// * [`name`](Tool::name) -- The canonical name the LLM uses to invoke this tool.
/// * [`definition`](Tool::definition) -- The OpenAI-compatible function definition
///   (name, description, parameters schema).
/// * [`execute`](Tool::execute) -- The core execution logic.
///
/// # Optional Lifecycle Hooks
///
/// * [`start_turn`](Tool::start_turn) -- Reset per-turn state (e.g., `message` tool's
///   `sent_in_turn` flag).
/// * [`sent_in_turn`](Tool::sent_in_turn) -- Query whether this tool sent a message
///   during the current agent turn.
/// * [`cancel_by_session`](Tool::cancel_by_session) -- Cancel any session-scoped
///   background tasks (e.g., spawned subagents).
///
/// # Example
///
/// ```ignore
/// use async_trait::async_trait;
/// use nanobot_tools::base::{Tool, ToolContext, ToolDefinition};
/// use nanobot_tools::{ToolResult, ToolError};
/// use std::sync::Arc;
///
/// struct GreetTool;
///
/// #[async_trait]
/// impl Tool for GreetTool {
///     fn name(&self) -> &str { "greet" }
///     fn definition(&self) -> Arc<ToolDefinition> { /* ... */ }
///     async fn execute(&self, args_json: &str, ctx: &ToolContext) -> ToolResult<String> {
///         Ok("Hello!".to_string())
///     }
/// }
/// ```
#[async_trait]
pub trait Tool: Send + Sync {
    /// Returns the stable function name exposed to the LLM.
    ///
    /// This name is used by the model to invoke the tool and must match
    /// the `name` field in the tool definition. Tool names should be
    /// snake_case and unique within a registry.
    fn name(&self) -> &str;

    /// Returns the OpenAI-compatible function definition for this tool.
    ///
    /// The definition includes the tool name, description, and parameter
    /// schema. Returns an `Arc` for cheap cloning (8 bytes vs ~184 bytes
    /// for the full struct), which is important since the agent loop may
    /// call `definition()` frequently (e.g., once per LLM request) and
    /// needs to send the definition to the model.
    fn definition(&self) -> std::sync::Arc<ToolDefinition>;

    /// Executes the tool with raw JSON arguments and runtime context.
    ///
    /// # Arguments
    ///
    /// * `args_json` - A JSON string matching the tool's parameter schema.
    /// * `ctx` - Runtime context (channel, chat_id, session_key, message_id).
    ///
    /// # Returns
    ///
    /// A string result that will be sent back to the LLM as text. This
    /// keeps the tool contract simple: tools produce text, not structured
    /// data (though the text may be JSON-serialized).
    ///
    /// # Errors
    ///
    /// Returns a [`ToolError`] if execution fails. The agent loop wraps
    /// errors as text and continues the turn rather than aborting.
    async fn execute(&self, args_json: &str, ctx: &ToolContext) -> ToolResult<String>;

    /// Optional hook called at the start of each agent turn.
    ///
    /// Tools can use this to reset per-turn state. The default
    /// implementation is a no-op.
    ///
    /// # Examples
    ///
    /// The [`MessageTool`](crate::message::MessageTool) uses this hook to
    /// reset its `sent_in_turn` flag to `false` at the beginning of each
    /// agent turn.
    async fn start_turn(&self) -> ToolResult<()> {
        Ok(())
    }

    /// Optional signal indicating whether this tool sent a message
    /// to the user in the current agent turn.
    ///
    /// The agent loop uses this to decide whether the LLM needs to
    /// produce a visible response. If no tool sent a message, the
    /// loop assumes the LLM's text response is the answer.
    ///
    /// The default implementation returns `false`.
    async fn sent_in_turn(&self) -> ToolResult<bool> {
        Ok(false)
    }

    /// Optional cancellation hook for session-scoped background tasks.
    ///
    /// Called when a session ends or is interrupted. Tools that spawn
    /// long-running background work (e.g., [`SpawnTool`](crate::spawn::SpawnTool))
    /// should cancel all tasks associated with the given session key.
    ///
    /// Returns the number of tasks that were cancelled. The default
    /// implementation returns `Ok(0)`.
    async fn cancel_by_session(&self, _session_key: &str) -> ToolResult<usize> {
        Ok(0)
    }
}

/// Parses raw JSON tool arguments into a strongly-typed struct.
///
/// This is the standard entry point for argument deserialization in
/// tool `execute` implementations. It converts the raw JSON string
/// passed by the LLM into a `DeserializeOwned` argument struct.
///
/// # Arguments
///
/// * `args_json` - A JSON string representing tool arguments.
///
/// # Returns
///
/// The parsed argument struct of type `T`.
///
/// # Errors
///
/// Returns [`ToolError::InvalidArgs`] if the JSON cannot be deserialized
/// into `T`. This keeps error messages consistent across all tools.
///
/// # Example
///
/// ```ignore
/// use serde::Deserialize;
/// use nanobot_tools::base::parse_args;
///
/// #[derive(Deserialize)]
/// struct MyArgs { name: String, count: Option<i32> }
///
/// let args: MyArgs = parse_args(r#"{"name":"demo","count":7}"#).unwrap();
/// ```
pub fn parse_args<T>(args_json: &str) -> ToolResult<T>
where
    T: DeserializeOwned,
{
    serde_json::from_str::<T>(args_json)
        .map_err(|e| ToolError::invalid_args("unknown", format!("invalid tool arguments: {}", e)))
}

/// Helper for building ordered JSON Schema property maps with less boilerplate.
///
/// Accepts an iterator of `(name, schema)` pairs and returns a
/// `BTreeMap<String, JsonSchema>`. Using `BTreeMap` ensures properties
/// are sorted by name, producing deterministic JSON output.
///
/// # Example
///
/// ```ignore
/// use nanobot_tools::base::{JsonSchema, schema_props};
///
/// let props = schema_props([
///     ("name", JsonSchema::string(Some("The name"))),
///     ("count", JsonSchema::integer(Some("How many"))),
/// ]);
/// ```
pub fn schema_props<I, K>(entries: I) -> BTreeMap<String, JsonSchema>
where
    I: IntoIterator<Item = (K, JsonSchema)>,
    K: Into<String>,
{
    entries.into_iter().map(|(k, v)| (k.into(), v)).collect()
}

/// Builds a [`ToolDefinition`] from a `serde_json::Value`.
///
/// This is a more concise way to define tool schemas inline using the
/// `json!` macro, avoiding manual [`ToolDefinition`] construction.
///
/// # Panics
///
/// Panics if the JSON value does not match the expected shape of a
/// `ToolDefinition`. This is intentional: tool definitions are static
/// and compiled into the binary, so a malformed definition is a
/// programming error that should fail early.
///
/// # Example
///
/// ```ignore
/// use serde_json::json;
/// use nanobot_tools::base::tool_definition_from_json;
///
/// let def = tool_definition_from_json(json!({
///     "type": "function",
///     "function": {
///         "name": "my_tool",
///         "description": "Does something",
///         "parameters": { "type": "object", "properties": {} }
///     }
/// }));
/// ```
pub fn tool_definition_from_json(value: serde_json::Value) -> ToolDefinition {
    serde_json::from_value(value).expect("invalid tool definition JSON")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_args_parses_expected_fields() {
        let json = r#"{"name":"demo","count":7}"#;

        let parsed: serde_json::Value = parse_args(json).expect("parse args");
        assert_eq!(parsed.get("name").and_then(|v| v.as_str()), Some("demo"));
        assert_eq!(parsed.get("count").and_then(|v| v.as_i64()), Some(7));
    }

    #[test]
    fn tool_definition_serializes_to_function_shape() {
        let mut props = BTreeMap::new();
        props.insert("q".to_string(), JsonSchema::string(Some("query")));
        let def =
            ToolDefinition::function("web_search", "search", JsonSchema::object(props, vec!["q"]));
        let value = serde_json::to_string(&def).expect("serialize tool definition");

        assert!(value.contains("\"type\":\"function\""));
        assert!(value.contains("\"name\":\"web_search\""));
        assert!(value.contains("\"required\":[\"q\"]"));
    }
}
