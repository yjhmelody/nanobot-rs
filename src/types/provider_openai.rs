use serde::{Deserialize, Serialize};

use crate::types::tools::{JsonSchema, ToolDefinition};

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ResponsesPayload {
    pub(crate) model: String,
    pub(crate) input: Vec<ResponseInputItem>,
    pub(crate) max_output_tokens: i32,
    pub(crate) temperature: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reasoning: Option<ResponseReasoningConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tools: Option<Vec<ResponseToolDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tool_choice: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ResponseReasoningConfig {
    pub(crate) effort: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub(crate) enum ResponseInputItem {
    Message(ResponseInputMessage),
    FunctionCall(ResponseFunctionCallItem),
    FunctionCallOutput(ResponseFunctionCallOutputItem),
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ResponseInputMessage {
    pub(crate) role: String,
    pub(crate) content: Vec<ResponseInputContent>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ResponseInputContent {
    #[serde(rename = "type")]
    pub(crate) kind: &'static str,
    pub(crate) text: String,
}

impl ResponseInputContent {
    pub(crate) fn input_text(text: String) -> Self {
        Self {
            kind: "input_text",
            text,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ResponseFunctionCallItem {
    #[serde(rename = "type")]
    pub(crate) kind: &'static str,
    pub(crate) call_id: String,
    pub(crate) name: String,
    pub(crate) arguments: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ResponseFunctionCallOutputItem {
    #[serde(rename = "type")]
    pub(crate) kind: &'static str,
    pub(crate) call_id: String,
    pub(crate) output: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ResponseToolDefinition {
    #[serde(rename = "type")]
    pub(crate) kind: &'static str,
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) parameters: JsonSchema,
}

impl From<ToolDefinition> for ResponseToolDefinition {
    fn from(value: ToolDefinition) -> Self {
        Self {
            kind: "function",
            name: value.function.name,
            description: value.function.description,
            parameters: value.function.parameters,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct OpenAIResponsesResponse {
    #[serde(default)]
    pub(crate) output: Vec<serde_json::Value>,
    #[serde(default)]
    pub(crate) usage: Option<ResponsesUsage>,
    #[serde(default)]
    pub(crate) error: Option<ResponsesError>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct ResponsesUsage {
    #[serde(default)]
    pub(crate) input_tokens: Option<u64>,
    #[serde(default)]
    pub(crate) output_tokens: Option<u64>,
    #[serde(default)]
    pub(crate) total_tokens: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct ResponsesError {
    #[serde(default)]
    pub(crate) message: Option<String>,
}
