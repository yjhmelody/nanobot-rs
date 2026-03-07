use serde::de::Error as DeError;
use serde::{Deserialize, Deserializer, Serialize};

use crate::types::provider::ChatMessage;
use crate::types::tools::ToolDefinition;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ChatCompletionPayload {
    pub(crate) model: String,
    pub(crate) messages: Vec<ChatMessage>,
    pub(crate) max_tokens: i32,
    pub(crate) temperature: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tools: Option<Vec<ToolDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tool_choice: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct OpenAIChatResponse {
    pub(crate) choices: Vec<Choice>,
    #[serde(default)]
    pub(crate) usage: Option<Usage>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct Choice {
    pub(crate) message: AssistantMessage,
    #[serde(default)]
    pub(crate) finish_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct AssistantMessage {
    #[serde(default)]
    pub(crate) content: Option<String>,
    #[serde(default)]
    pub(crate) tool_calls: Option<Vec<OpenAIToolCall>>,
    #[serde(default)]
    pub(crate) reasoning_content: Option<String>,
    #[serde(default)]
    pub(crate) thinking_blocks: Option<Vec<ThinkingBlock>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct OpenAIToolCall {
    #[serde(default)]
    pub(crate) id: Option<String>,
    pub(crate) function: OpenAIFunctionCall,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct OpenAIFunctionCall {
    pub(crate) name: String,
    #[serde(rename = "arguments", deserialize_with = "deserialize_arguments_json")]
    pub(crate) arguments_json: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub(crate) enum ThinkingBlock {
    Text(String),
    Structured(StructuredThinkingBlock),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct StructuredThinkingBlock {
    #[serde(default)]
    pub(crate) text: Option<String>,
    #[serde(default)]
    pub(crate) content: Option<String>,
    #[serde(default)]
    pub(crate) summary: Option<String>,
}

impl ThinkingBlock {
    pub(crate) fn into_text(self) -> Option<String> {
        match self {
            Self::Text(s) => (!s.trim().is_empty()).then_some(s),
            Self::Structured(v) => v
                .text
                .or(v.content)
                .or(v.summary)
                .and_then(|s| (!s.trim().is_empty()).then_some(s)),
        }
    }
}

pub(crate) fn deserialize_arguments_json<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = Box::<serde_json::value::RawValue>::deserialize(deserializer)?;
    let payload = raw.get();
    if payload.starts_with('"') {
        serde_json::from_str::<String>(payload).map_err(D::Error::custom)
    } else {
        Ok(payload.to_string())
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct Usage {
    #[serde(default)]
    pub(crate) prompt_tokens: Option<u64>,
    #[serde(default)]
    pub(crate) completion_tokens: Option<u64>,
    #[serde(default)]
    pub(crate) total_tokens: Option<u64>,
}
