use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::bus::MessageId;
use crate::SessionKey;

/// Context information passed into tool execution.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolContext {
    /// Current channel name (e.g. `cli`, `telegram`).
    pub channel: String,
    /// Current conversation id within the channel.
    pub chat_id: String,
    /// Session key used for cancellation and state scoping.
    pub session_key: SessionKey,
    /// Optional source message id for threaded/reply scenarios.
    pub message_id: Option<MessageId>,
}

/// OpenAI-compatible tool definition wrapper.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Always `function` for OpenAI-compatible tool schema.
    #[serde(rename = "type")]
    pub kind: String,
    /// Function schema definition for the tool.
    pub function: ToolFunction,
}

/// Function schema contained in a tool definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolFunction {
    /// Function name exposed to the model.
    pub name: String,
    /// Human-readable tool description.
    pub description: String,
    /// JSON schema for parameters.
    pub parameters: JsonSchema,
}

/// JSON schema types supported by tool parameters.
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

/// JSON schema definition for tool parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonSchema {
    #[serde(rename = "type")]
    /// JSON schema type of this node.
    pub schema_type: JsonSchemaType,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Optional description for the schema node.
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    /// Properties for object schemas.
    pub properties: BTreeMap<String, JsonSchema>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    /// Required property names for object schemas.
    pub required: Vec<String>,
    #[serde(rename = "enum", skip_serializing_if = "Option::is_none")]
    /// Enumerated allowed values, if any.
    pub enum_values: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Item schema for array types.
    pub items: Option<Box<JsonSchema>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Minimum numeric value constraint.
    pub minimum: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Maximum numeric value constraint.
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

/// Arguments for read_file tool.
#[derive(Debug, Deserialize)]
pub struct ReadFileArgs {
    /// Path to read from.
    pub path: String,
}

/// Arguments for write_file tool.
#[derive(Debug, Deserialize)]
pub struct WriteFileArgs {
    /// Path to write to.
    pub path: String,
    /// File contents to write.
    pub content: String,
}

/// Arguments for edit_file tool.
#[derive(Debug, Deserialize)]
pub struct EditFileArgs {
    /// Path of the file to edit.
    pub path: String,
    /// Exact text to replace.
    pub old_text: String,
    /// Replacement text.
    pub new_text: String,
}

/// Arguments for list_dir tool.
#[derive(Debug, Deserialize)]
pub struct ListDirArgs {
    /// Directory path to list.
    pub path: String,
}

/// Arguments for message tool.
#[derive(Debug, Deserialize)]
pub struct MessageArgs {
    /// Text content to send.
    pub content: String,
    /// Optional target channel override.
    pub channel: Option<String>,
    /// Optional target chat id override.
    pub chat_id: Option<String>,
    /// Optional reply-to message id.
    pub message_id: Option<String>,
    /// Optional media attachments.
    pub media: Option<Vec<String>>,
}

/// Actions supported by cron tool.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CronAction {
    Add,
    Once,
    List,
    Remove,
}

/// Arguments for cron tool operations.
#[derive(Debug, Deserialize)]
pub struct CronArgs {
    /// Cron action to perform.
    pub action: CronAction,
    /// Optional message payload.
    pub message: Option<String>,
    /// Interval in seconds for `every`.
    pub every_seconds: Option<i64>,
    /// Cron expression for `cron`.
    pub cron_expr: Option<String>,
    /// Optional timezone for cron expressions.
    pub tz: Option<String>,
    /// Scheduled time for one-shot `at`.
    pub at: Option<String>,
    /// Job id for remove.
    pub job_id: Option<String>,
}

/// Arguments for spawn tool.
#[derive(Debug, Deserialize)]
pub struct SpawnArgs {
    /// Task description for subagent.
    pub task: String,
    /// Optional label for the task.
    pub label: Option<String>,
}

/// Arguments for exec tool.
#[derive(Debug, Deserialize)]
pub struct ExecArgs {
    /// Command string to execute.
    pub command: String,
    /// Optional working directory.
    pub working_dir: Option<String>,
}

/// Arguments for ACP execute tool.
#[derive(Debug, Deserialize)]
pub struct ACPExecuteArgs {
    /// ACP agent identifier.
    pub agent_id: String,
    /// Task prompt to send.
    pub task: String,
    /// Optional working directory for the agent.
    pub cwd: Option<PathBuf>,
}

/// Arguments for web_search tool.
#[derive(Debug, Deserialize)]
pub struct WebSearchArgs {
    /// Search query string.
    pub query: String,
    /// Optional result count limit.
    pub count: Option<i64>,
}

/// Arguments for web_fetch tool.
#[derive(Debug, Deserialize)]
pub struct WebFetchArgs {
    /// URL to fetch.
    pub url: String,
    /// Optional max characters to return.
    pub max_chars: Option<i64>,
}

/// Partial Brave search API response payload.
#[derive(Debug, Deserialize)]
pub struct BraveSearchResponse {
    /// Web search results container.
    pub web: Option<BraveWebData>,
}

/// Brave web search results container.
#[derive(Debug, Deserialize)]
pub struct BraveWebData {
    #[serde(default)]
    /// Search result list.
    pub results: Vec<BraveResult>,
}

/// Single Brave search result item.
#[derive(Debug, Deserialize)]
pub struct BraveResult {
    #[serde(default)]
    /// Result title.
    pub title: String,
    #[serde(default)]
    /// Result URL.
    pub url: String,
    #[serde(default)]
    /// Result description snippet.
    pub description: Option<String>,
}

/// Normalized response for web_fetch tool output.
#[derive(Debug, Serialize)]
pub struct WebFetchResponse {
    /// Requested URL.
    pub url: String,
    #[serde(rename = "finalUrl")]
    /// Final URL after redirects.
    pub final_url: String,
    /// HTTP status code.
    pub status: u16,
    /// Extractor name used to parse content.
    pub extractor: String,
    /// Whether content was truncated.
    pub truncated: bool,
    /// Returned content length.
    pub length: usize,
    /// Extracted text content.
    pub text: String,
}
