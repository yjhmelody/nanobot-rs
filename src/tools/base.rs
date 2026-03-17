use std::collections::BTreeMap;

use async_trait::async_trait;
use serde::de::DeserializeOwned;

use crate::tools::{ToolError, ToolResult};
pub use crate::types::tools::{
    JsonSchema, JsonSchemaType, ToolContext, ToolDefinition, ToolFunction,
};

/// Runtime contract for all agent tools.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Stable function name exposed to the model.
    fn name(&self) -> &str;

    /// OpenAI-compatible function definition.
    /// Returns Arc for cheap cloning (8 bytes vs 184 bytes).
    fn definition(&self) -> std::sync::Arc<ToolDefinition>;

    /// Execute tool using raw JSON args with runtime context.
    async fn execute(&self, args_json: &str, ctx: &ToolContext) -> ToolResult<String>;

    /// Optional hook called at the start of each agent turn.
    async fn start_turn(&self) -> ToolResult<()> {
        Ok(())
    }

    /// Optional signal used by tools like `message`.
    /// Returns true if the tool sent a message in the current turn.
    async fn sent_in_turn(&self) -> ToolResult<bool> {
        Ok(false)
    }

    /// Optional cancellation hook for session-scoped background tasks.
    async fn cancel_by_session(&self, _session_key: &str) -> ToolResult<usize> {
        Ok(0)
    }
}

/// Parse raw JSON arguments into a strong-typed argument struct.
pub fn parse_args<T>(args_json: &str) -> ToolResult<T>
where
    T: DeserializeOwned,
{
    serde_json::from_str::<T>(args_json).map_err(|e| {
        ToolError::invalid_args("unknown", format!("invalid tool arguments: {}", e))
    })
}

/// Helper for building ordered schema properties with less boilerplate.
pub fn schema_props<I, K>(entries: I) -> BTreeMap<String, JsonSchema>
where
    I: IntoIterator<Item = (K, JsonSchema)>,
    K: Into<String>,
{
    entries.into_iter().map(|(k, v)| (k.into(), v)).collect()
}

/// Build a ToolDefinition from a JSON value using serde_json::json! macro.
///
/// This is a more concise way to define tool schemas compared to the builder pattern.
///
/// # Example
///
/// ```ignore
/// use serde_json::json;
/// use nanobot_rs::tools::base::tool_definition_from_json;
///
/// let def = tool_definition_from_json(json!({
///     "type": "function",
///     "function": {
///         "name": "read_file",
///         "description": "Read a file",
///         "parameters": {
///             "type": "object",
///             "properties": {
///                 "path": {
///                     "type": "string",
///                     "description": "File path"
///                 }
///             },
///             "required": ["path"]
///         }
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
