use std::collections::BTreeMap;

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
#[serde(rename_all = "camelCase")]
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

#[derive(Debug, Deserialize)]
pub(crate) struct ReadFileArgs {
    pub(crate) path: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct WriteFileArgs {
    pub(crate) path: String,
    pub(crate) content: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct EditFileArgs {
    pub(crate) path: String,
    pub(crate) old_text: String,
    pub(crate) new_text: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ListDirArgs {
    pub(crate) path: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct MessageArgs {
    pub(crate) content: String,
    pub(crate) channel: Option<String>,
    pub(crate) chat_id: Option<String>,
    pub(crate) message_id: Option<String>,
    pub(crate) media: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum CronAction {
    Add,
    Once,
    List,
    Remove,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CronArgs {
    pub(crate) action: CronAction,
    pub(crate) message: Option<String>,
    pub(crate) every_seconds: Option<i64>,
    pub(crate) cron_expr: Option<String>,
    pub(crate) tz: Option<String>,
    pub(crate) at: Option<String>,
    pub(crate) job_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SpawnArgs {
    pub(crate) task: String,
    pub(crate) label: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ExecArgs {
    pub(crate) command: String,
    pub(crate) working_dir: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct WebSearchArgs {
    pub(crate) query: String,
    pub(crate) count: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct WebFetchArgs {
    pub(crate) url: String,
    pub(crate) max_chars: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct BraveSearchResponse {
    pub(crate) web: Option<BraveWebData>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct BraveWebData {
    #[serde(default)]
    pub(crate) results: Vec<BraveResult>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct BraveResult {
    #[serde(default)]
    pub(crate) title: String,
    #[serde(default)]
    pub(crate) url: String,
    #[serde(default)]
    pub(crate) description: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct WebFetchResponse {
    pub(crate) url: String,
    #[serde(rename = "finalUrl")]
    pub(crate) final_url: String,
    pub(crate) status: u16,
    pub(crate) extractor: String,
    pub(crate) truncated: bool,
    pub(crate) length: usize,
    pub(crate) text: String,
}
