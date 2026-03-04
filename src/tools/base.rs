use std::collections::BTreeMap;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolContext {
    /// Current channel name (e.g. `cli`, `telegram`).
    pub channel: String,
    /// Current conversation id within the channel.
    pub chat_id: String,
    /// Session key used for cancellation and state scoping.
    pub session_key: String,
    /// Optional source message id for threaded/reply scenarios.
    pub message_id: Option<String>,
}

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

/// Extension point for registering a batch of tools into the registry.
pub trait ToolPlugin {
    fn register(&self, registry: &crate::tools::registry::ToolRegistry) -> Result<()>;
}

/// Parse raw JSON arguments into a strong-typed argument struct.
pub fn parse_args<T>(args_json: &str) -> Result<T>
where
    T: DeserializeOwned,
{
    serde_json::from_str::<T>(args_json).context("invalid tool arguments")
}

/// Helper for building ordered schema properties with less boilerplate.
pub fn schema_props<I, K>(entries: I) -> BTreeMap<String, JsonSchema>
where
    I: IntoIterator<Item = (K, JsonSchema)>,
    K: Into<String>,
{
    entries.into_iter().map(|(k, v)| (k.into(), v)).collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Always `function` for OpenAI-compatible tool schema.
    #[serde(rename = "type")]
    pub kind: String,
    pub function: ToolFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolFunction {
    pub name: String,
    pub description: String,
    pub parameters: JsonSchema,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JsonSchemaType {
    Object,
    String,
    Integer,
    Number,
    Array,
    Boolean,
    Null,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonSchema {
    #[serde(rename = "type")]
    pub schema_type: JsonSchemaType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub properties: BTreeMap<String, JsonSchema>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required: Vec<String>,
    #[serde(rename = "enum", skip_serializing_if = "Option::is_none")]
    pub enum_values: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<Box<JsonSchema>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maximum: Option<i64>,
}

impl ToolDefinition {
    pub fn function(name: &str, description: &str, parameters: JsonSchema) -> Self {
        Self {
            kind: "function".to_string(),
            function: ToolFunction {
                name: name.to_string(),
                description: description.to_string(),
                parameters,
            },
        }
    }
}

impl JsonSchema {
    pub fn object(properties: BTreeMap<String, JsonSchema>, required: Vec<&str>) -> Self {
        Self {
            schema_type: JsonSchemaType::Object,
            description: None,
            properties,
            required: required.into_iter().map(|s| s.to_string()).collect(),
            enum_values: None,
            items: None,
            minimum: None,
            maximum: None,
        }
    }

    pub fn string(description: Option<&str>) -> Self {
        Self {
            schema_type: JsonSchemaType::String,
            description: description.map(|s| s.to_string()),
            properties: BTreeMap::new(),
            required: Vec::new(),
            enum_values: None,
            items: None,
            minimum: None,
            maximum: None,
        }
    }

    pub fn integer(description: Option<&str>) -> Self {
        Self {
            schema_type: JsonSchemaType::Integer,
            description: description.map(|s| s.to_string()),
            properties: BTreeMap::new(),
            required: Vec::new(),
            enum_values: None,
            items: None,
            minimum: None,
            maximum: None,
        }
    }

    pub fn array(items: JsonSchema, description: Option<&str>) -> Self {
        Self {
            schema_type: JsonSchemaType::Array,
            description: description.map(|s| s.to_string()),
            properties: BTreeMap::new(),
            required: Vec::new(),
            enum_values: None,
            items: Some(Box::new(items)),
            minimum: None,
            maximum: None,
        }
    }

    pub fn with_enum(mut self, values: Vec<&str>) -> Self {
        self.enum_values = Some(values.into_iter().map(|s| s.to_string()).collect());
        self
    }

    pub fn with_minimum(mut self, minimum: i64) -> Self {
        self.minimum = Some(minimum);
        self
    }

    pub fn with_maximum(mut self, maximum: i64) -> Self {
        self.maximum = Some(maximum);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Deserialize, PartialEq)]
    struct DemoArgs {
        name: String,
        #[serde(alias = "countValue")]
        count: i64,
    }

    #[test]
    fn parse_args_supports_aliases() {
        let json = r#"{"name":"demo","countValue":7}"#;

        let parsed: DemoArgs = parse_args(json).expect("parse args");
        assert_eq!(
            parsed,
            DemoArgs {
                name: "demo".to_string(),
                count: 7
            }
        );
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
