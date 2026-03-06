use std::collections::BTreeMap;

use async_trait::async_trait;
use serde::de::DeserializeOwned;

use crate::error::{NanobotError, Result};
pub use crate::types::tools::{
    JsonSchema, JsonSchemaType, ToolContext, ToolDefinition, ToolFunction,
};

/// Runtime contract for all agent tools.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Stable function name exposed to the model.
    fn name(&self) -> &str;

    /// OpenAI-compatible function definition.
    fn definition(&self) -> ToolDefinition;

    /// Execute tool using raw JSON args with runtime context.
    async fn execute(&self, args_json: &str, ctx: &ToolContext) -> Result<String>;

    /// Optional hook called at the start of each agent turn.
    async fn start_turn(&self) -> Result<()> {
        Ok(())
    }

    /// Optional signal used by tools like `message`.
    /// Returns true if the tool sent a message in the current turn.
    async fn sent_in_turn(&self) -> Result<bool> {
        Ok(false)
    }

    /// Optional cancellation hook for session-scoped background tasks.
    async fn cancel_by_session(&self, _session_key: &str) -> Result<usize> {
        Ok(0)
    }
}

/// Parse raw JSON arguments into a strong-typed argument struct.
pub fn parse_args<T>(args_json: &str) -> Result<T>
where
    T: DeserializeOwned,
{
    serde_json::from_str::<T>(args_json).map_err(|e| {
        NanobotError::invalid_tool_args("unknown", format!("invalid tool arguments: {}", e))
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
