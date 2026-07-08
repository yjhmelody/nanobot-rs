//! Tool definition, schema, and argument types.
//!
//! This module provides the data types used to define tools for LLM
//! consumption and to parse tool arguments from model-generated JSON.
//!
//! # Structure
//!
//! - [`ToolDefinition`], [`ToolFunction`], [`JsonSchema`], and
//!   [`JsonSchemaType`] form the schema layer, generating
//!   OpenAI-compatible tool definitions for LLM tool calling.
//! - `*Args` structs (e.g., [`ReadFileArgs`], [`WriteFileArgs`]) define
//!   the typed deserialisation targets for each built-in tool's arguments.
//! - Response types (e.g., [`WebFetchResponse`], [`BraveSearchResponse`])
//!   define the expected JSON structure from external APIs.
//! - [`ToolContext`] carries runtime context into tool execution.
//!
//! # Design
//!
//! - All `*Args` structs implement `Deserialize` (not `Serialize`) since
//!   they are only used for parsing LLM-generated JSON.
//! - [`JsonSchema`] uses a builder pattern with `with_*` methods for
//!   ergonomic construction from Rust code.
//! - `#[serde(deny_unknown_fields)]` is intentionally _not_ used on
//!   `*Args` structs, since LLM-generated JSON may include extra fields
//!   that should be silently ignored.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::SessionKey;
use crate::bus::MessageId;

/// Context information passed into tool execution.
///
/// Provides runtime context about the current conversation to every tool
/// execution. Tools use this for scoping (session key), routing (channel),
/// and reply threading (message ID).
///
/// # Fields
///
/// * `channel` — The channel name where the current turn is happening
///   (e.g., `"cli"`, `"telegram"`).
/// * `chat_id` — The conversation/chat ID within the channel.
/// * `session_key` — The session key for the current conversation, used
///   for cancellation and state scoping.
/// * `message_id` — The source message ID, if available, for threaded
///   replies.
///
/// # Derive rationale
///
/// - `Clone`: tool context may be cloned when shared across tool calls
///   in the same turn.
/// - `Default`: used for testing and for tools that don't need context.
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

/// An OpenAI-compatible tool definition for LLM function calling.
///
/// Wraps a [`ToolFunction`] with a `type` field (always `"function"`).
/// This matches the format expected by OpenAI, Anthropic, and other
/// major LLM providers.
///
/// # Serde format
///
/// ```json
/// {"type": "function", "function": {...}}
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Always `"function"` for OpenAI-compatible tool schema.
    #[serde(rename = "type")]
    pub kind: String,
    /// The actual function schema definition.
    pub function: ToolFunction,
}

/// The function schema contained within a [`ToolDefinition`].
///
/// Defines the tool's name, description, and JSON Schema parameters
/// that the LLM uses to generate valid tool call arguments.
///
/// # Fields
///
/// * `name` — The function name exposed to the model.
/// * `description` — Human-readable description of what the tool does.
/// * `parameters` — JSON Schema object describing the expected arguments.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolFunction {
    /// Function name exposed to the model.
    pub name: String,
    /// Human-readable tool description.
    pub description: String,
    /// JSON schema for parameters.
    pub parameters: JsonSchema,
}

/// JSON Schema types supported by tool parameter definitions.
///
/// Mirrors the subset of JSON Schema types used in LLM tool definitions.
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

/// A JSON Schema node for tool parameter definitions.
///
/// Supports nested schemas (via `properties` for objects and `items` for
/// arrays), enumerated values, and numeric constraints. Built using a
/// fluent builder pattern with `with_*` methods.
///
/// # Serde notes
///
/// The `#[serde(rename = "type")]` on `schema_type` and
/// `#[serde(rename = "enum")]` on `enum_values` match the JSON Schema
/// specification keyword names.
///
/// # Builder pattern
///
/// ```
/// use std::collections::BTreeMap;
/// use nanobot_types::tools::{JsonSchema, JsonSchemaType};
/// let schema = JsonSchema::object(
///     BTreeMap::from([("name".into(), JsonSchema::string(Some("The name")))]),
///     vec!["name"],
/// );
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonSchema {
    /// JSON Schema type of this node (e.g., `Object`, `String`).
    #[serde(rename = "type")]
    pub schema_type: JsonSchemaType,
    /// Optional description for the schema node.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Properties for object schemas (keyed by property name).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub properties: BTreeMap<String, JsonSchema>,
    /// Required property names for object schemas.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required: Vec<String>,
    /// Enumerated list of allowed values, if any.
    #[serde(rename = "enum", skip_serializing_if = "Option::is_none")]
    pub enum_values: Option<Vec<String>>,
    /// Item schema for array types.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<Box<JsonSchema>>,
    /// Minimum numeric value constraint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum: Option<i64>,
    /// Maximum numeric value constraint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maximum: Option<i64>,
}

impl ToolDefinition {
    /// Creates a new tool definition with the given name, description, and
    /// parameter schema.
    ///
    /// The `kind` field is automatically set to `"function"`.
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
    /// Creates a JSON Schema of type `Object` with the given properties
    /// and required field list.
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

    /// Creates a JSON Schema of type `String` with an optional description.
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

    /// Creates a JSON Schema of type `Integer` with an optional description.
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

    /// Creates a JSON Schema of type `Array` with the given item schema
    /// and an optional description.
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

    /// Adds an `enum` constraint to this schema, restricting valid values
    /// to the given list.
    ///
    /// This is a builder method that consumes and returns `self`.
    pub fn with_enum(mut self, values: Vec<&str>) -> Self {
        self.enum_values = Some(values.into_iter().map(|s| s.to_string()).collect());
        self
    }

    /// Adds a `minimum` numeric constraint to this schema.
    ///
    /// This is a builder method that consumes and returns `self`.
    pub fn with_minimum(mut self, minimum: i64) -> Self {
        self.minimum = Some(minimum);
        self
    }

    /// Adds a `maximum` numeric constraint to this schema.
    ///
    /// This is a builder method that consumes and returns `self`.
    pub fn with_maximum(mut self, maximum: i64) -> Self {
        self.maximum = Some(maximum);
        self
    }
}

// ---------------------------------------------------------------------------
// Built-in tool argument types
// ---------------------------------------------------------------------------
// Each `*Args` struct below matches the JSON schema that the LLM is expected
// to generate for the corresponding tool. They implement only `Deserialize`
// because they are one-directional: JSON from the LLM → typed Rust struct.
// ---------------------------------------------------------------------------

/// Arguments for the `read_file` tool.
///
/// # Fields
///
/// * `path` — Filesystem path to read from.
#[derive(Debug, Deserialize)]
pub struct ReadFileArgs {
    /// Path to read from.
    pub path: String,
}

/// Arguments for the `write_file` tool.
///
/// # Fields
///
/// * `path` — Filesystem path to write to.
/// * `content` — File contents to write.
#[derive(Debug, Deserialize)]
pub struct WriteFileArgs {
    /// Path to write to.
    pub path: String,
    /// File contents to write.
    pub content: String,
}

/// Arguments for the `edit_file` tool.
///
/// Applies a string replacement in a file at the given `path`.
///
/// # Fields
///
/// * `path` — Path of the file to edit.
/// * `old_text` — Exact text to find and replace (must match exactly).
/// * `new_text` — Replacement text.
#[derive(Debug, Deserialize)]
pub struct EditFileArgs {
    /// Path of the file to edit.
    pub path: String,
    /// Exact text to replace.
    pub old_text: String,
    /// Replacement text.
    pub new_text: String,
}

/// Arguments for the `list_dir` tool.
///
/// # Fields
///
/// * `path` — Directory path to list.
#[derive(Debug, Deserialize)]
pub struct ListDirArgs {
    /// Directory path to list.
    pub path: String,
}

/// Actions supported by the `cron` tool.
///
/// | Variant | Effect |
/// |---------|--------|
/// | `Add` | Create a new recurring cron job |
/// | `Once` | Create a one-shot scheduled job |
/// | `List` | List all registered cron jobs |
/// | `Remove` | Remove a cron job by ID |
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CronAction {
    Add,
    Once,
    List,
    Remove,
}

/// Arguments for the `cron` tool operations.
///
/// The specific fields required depend on [`action`](CronArgs::action):
///
/// | `action` | Key fields |
/// |----------|------------|
/// | `Add` | `message`, `every_seconds` or `cron_expr`, optionally `tz` |
/// | `Once` | `message`, `at` |
/// | `List` | (none) |
/// | `Remove` | `job_id` |
#[derive(Debug, Deserialize)]
pub struct CronArgs {
    /// Cron action to perform.
    pub action: CronAction,
    /// Optional message payload.
    pub message: Option<String>,
    /// Interval in seconds for `every` schedules.
    pub every_seconds: Option<i64>,
    /// Cron expression for cron-based schedules (e.g., `"0 9 * * 1-5"`).
    pub cron_expr: Option<String>,
    /// Optional IANA timezone for cron expressions.
    pub tz: Option<String>,
    /// Scheduled time (human-readable) for one-shot `at` schedules.
    pub at: Option<String>,
    /// Job ID for the `Remove` action.
    pub job_id: Option<String>,
}

/// Arguments for the `spawn` tool.
///
/// Creates a sub-agent that runs independently with its own session.
///
/// # Fields
///
/// * `task` — Description of the task for the sub-agent to perform.
/// * `label` — Optional human-readable label for tracking.
#[derive(Debug, Deserialize)]
pub struct SpawnArgs {
    /// Task description for subagent.
    pub task: String,
    /// Optional label for the task.
    pub label: Option<String>,
}

/// Arguments for the `exec` tool (shell command execution).
///
/// # Fields
///
/// * `command` — Shell command string to execute.
/// * `working_dir` — Optional working directory override.
#[derive(Debug, Deserialize)]
pub struct ExecArgs {
    /// Command string to execute.
    pub command: String,
    /// Optional working directory.
    pub working_dir: Option<String>,
}

/// Arguments for the ACP (Agent Communication Protocol) execute tool.
///
/// # Fields
///
/// * `agent_id` — ACP agent identifier to invoke.
/// * `task` — Task prompt to send to the remote agent.
/// * `cwd` — Optional working directory for the remote agent.
#[derive(Debug, Deserialize)]
pub struct ACPExecuteArgs {
    /// ACP agent identifier.
    pub agent_id: String,
    /// Task prompt to send.
    pub task: String,
    /// Optional working directory for the agent.
    pub cwd: Option<PathBuf>,
}

/// Arguments for the `web_search` tool.
///
/// # Fields
///
/// * `query` — Search query string.
/// * `count` — Optional limit on the number of results to return.
#[derive(Debug, Deserialize)]
pub struct WebSearchArgs {
    /// Search query string.
    pub query: String,
    /// Optional result count limit.
    pub count: Option<i64>,
}

/// Arguments for the `web_fetch` tool.
///
/// # Fields
///
/// * `url` — URL to fetch and extract text from.
/// * `max_chars` — Optional maximum number of characters to return.
#[derive(Debug, Deserialize)]
pub struct WebFetchArgs {
    /// URL to fetch.
    pub url: String,
    /// Optional max characters to return.
    pub max_chars: Option<i64>,
}

// ---------------------------------------------------------------------------
// External API response types
// ---------------------------------------------------------------------------

/// Partial Brave Search API response payload (deserialisation only).
///
/// Only the fields used by the web search tool are included. Additional
/// fields from the API (e.g., images, news) are ignored.
#[derive(Debug, Deserialize)]
pub struct BraveSearchResponse {
    /// Web search results container.
    pub web: Option<BraveWebData>,
}

/// Container for Brave web search results.
#[derive(Debug, Deserialize)]
pub struct BraveWebData {
    /// List of search result items.
    #[serde(default)]
    pub results: Vec<BraveResult>,
}

/// A single Brave Search result item.
#[derive(Debug, Deserialize)]
pub struct BraveResult {
    /// Result title.
    #[serde(default)]
    pub title: String,
    /// Result URL.
    #[serde(default)]
    pub url: String,
    /// Result description/snippet.
    #[serde(default)]
    pub description: Option<String>,
}

/// Normalised response for the `web_fetch` tool output.
///
/// Contains the fetched content along with metadata about the request.
///
/// Only `Serialize` is implemented (not `Deserialize`), as this is
/// produced by the tool and consumed by downstream code.
#[derive(Debug, Serialize)]
pub struct WebFetchResponse {
    /// Original requested URL.
    pub url: String,
    /// Final URL after redirects.
    #[serde(rename = "finalUrl")]
    pub final_url: String,
    /// HTTP status code of the response.
    pub status: u16,
    /// Name of the extractor used to parse content (e.g., `"readability"`,
    /// `"text"`, `"markdown"`).
    pub extractor: String,
    /// Whether the returned content was truncated due to size limits.
    pub truncated: bool,
    /// Length of the returned content in bytes or characters.
    pub length: usize,
    /// Extracted text content.
    pub text: String,
}
