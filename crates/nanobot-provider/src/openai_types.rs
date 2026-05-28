use serde::{Deserialize, Serialize};

use nanobot_types::tools::{JsonSchema, ToolDefinition};

/// OpenAI-compatible responses API request payload.
///
/// Spec source: https://platform.openai.com/docs/api-reference/responses/create
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ResponsesPayload {
    /// Model identifier.
    pub(crate) model: String,
    /// Input items (messages, tool calls, or tool outputs).
    pub(crate) input: Vec<ResponseInputItem>,
    /// Maximum output tokens to generate.
    pub(crate) max_output_tokens: i32,
    /// Sampling temperature.
    pub(crate) temperature: f32,
    /// Optional reasoning config.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reasoning: Option<ResponseReasoningConfig>,
    /// Optional tool definitions for function calling.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tools: Option<Vec<ResponseToolDefinition>>,
    /// Tool choice policy (e.g., "auto").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tool_choice: Option<String>,
    /// Enable streaming responses.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) stream: Option<bool>,
}

/// Reasoning configuration for responses API.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ResponseReasoningConfig {
    /// Reasoning effort (provider-specific string).
    pub(crate) effort: String,
}

/// Input item for responses API (message, tool call, or output).
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub(crate) enum ResponseInputItem {
    Message(ResponseInputMessage),
    FunctionCall(ResponseFunctionCallItem),
    FunctionCallOutput(ResponseFunctionCallOutputItem),
}

/// Input message content for responses API.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ResponseInputMessage {
    /// Role string ("system", "user", "assistant", "tool").
    pub(crate) role: String,
    /// Content parts for the message.
    pub(crate) content: Vec<ResponseInputContent>,
}

/// Input content part for responses API.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ResponseInputContent {
    /// Content type tag ("input_text").
    #[serde(rename = "type")]
    pub(crate) kind: &'static str,
    /// Content text.
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

/// Function call request item in responses API.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ResponseFunctionCallItem {
    /// Item type tag ("function_call").
    #[serde(rename = "type")]
    pub(crate) kind: &'static str,
    /// Tool call id.
    pub(crate) call_id: String,
    /// Tool/function name.
    pub(crate) name: String,
    /// JSON arguments string.
    pub(crate) arguments: String,
}

/// Function call output item for responses API.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ResponseFunctionCallOutputItem {
    /// Item type tag ("function_call_output").
    #[serde(rename = "type")]
    pub(crate) kind: &'static str,
    /// Tool call id.
    pub(crate) call_id: String,
    /// Tool output text.
    pub(crate) output: String,
}

/// Tool definition mapping for responses API.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ResponseToolDefinition {
    /// Tool definition type tag ("function").
    #[serde(rename = "type")]
    pub(crate) kind: &'static str,
    /// Tool/function name.
    pub(crate) name: String,
    /// Tool/function description.
    pub(crate) description: String,
    /// JSON schema for tool parameters.
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

/// Response output block from OpenAI-compatible API.
///
/// Spec source: https://platform.openai.com/docs/api-reference/responses/object
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ResponseOutputBlock {
    Message {
        /// Content parts for the output message.
        content: Vec<ResponseOutputContent>,
    },
    FunctionCall {
        /// Tool call id (some providers return "id").
        #[serde(alias = "id")]
        call_id: Option<String>,
        /// Tool/function name.
        name: String,
        /// Tool/function arguments payload.
        arguments: serde_json::Value,
    },
    Reasoning {
        /// Reasoning summary blocks.
        summary: Vec<ResponseReasoningSummary>,
    },
}

/// Output content parts returned by responses API.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ResponseOutputContent {
    /// Model output text content.
    OutputText { text: String },
    /// Echoed input text content.
    InputText { text: String },
}

/// Reasoning summaries returned by responses API.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ResponseReasoningSummary {
    /// Text summary item.
    SummaryText { text: String },
}

/// OpenAI-compatible responses API response payload.
///
/// Spec source: https://platform.openai.com/docs/api-reference/responses/object
#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct OpenAIResponsesResponse {
    /// Output items emitted by the model.
    #[serde(default)]
    pub(crate) output: Vec<ResponseOutputBlock>,
    /// Usage statistics, when provided.
    #[serde(default)]
    pub(crate) usage: Option<ResponsesUsage>,
    /// Error payload, when provided.
    #[serde(default)]
    pub(crate) error: Option<ResponsesError>,
}

/// Token usage metadata from responses API.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct ResponsesUsage {
    /// Input token count.
    #[serde(default)]
    pub(crate) input_tokens: Option<u64>,
    /// Output token count.
    #[serde(default)]
    pub(crate) output_tokens: Option<u64>,
    /// Total token count.
    #[serde(default)]
    pub(crate) total_tokens: Option<u64>,
}

/// Error payload returned by responses API.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct ResponsesError {
    /// Error message.
    #[serde(default)]
    pub(crate) message: Option<String>,
}

// OpenAI Responses streaming event types.
// Spec sources:
// https://platform.openai.com/docs/guides/streaming-responses
// https://platform.openai.com/docs/api-reference/responses-streaming/response/output_text/delta
// https://platform.openai.com/docs/guides/function-calling/how-do-i-ensure-the-model-calls-the-correct-function
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub(crate) enum ResponsesStreamEvent {
    #[serde(rename = "response.output_text.delta")]
    OutputTextDelta(ResponsesTextDeltaEvent),
    #[serde(rename = "response.output_text.done")]
    OutputTextDone(ResponsesTextDoneEvent),
    #[serde(rename = "response.reasoning_text.delta")]
    ReasoningTextDelta(ResponsesReasoningTextDeltaEvent),
    #[serde(rename = "response.reasoning_text.done")]
    ReasoningTextDone(ResponsesReasoningTextDoneEvent),
    #[serde(rename = "response.function_call_arguments.delta")]
    FunctionCallArgumentsDelta(ResponsesFunctionCallArgumentsDeltaEvent),
    #[serde(rename = "response.function_call_arguments.done")]
    FunctionCallArgumentsDone(ResponsesFunctionCallArgumentsDoneEvent),
    #[serde(rename = "response.output_item.added")]
    OutputItemAdded(ResponsesOutputItemEvent),
    #[serde(rename = "response.output_item.done")]
    OutputItemDone(ResponsesOutputItemEvent),
    #[serde(rename = "response.completed")]
    ResponseCompleted(ResponsesCompletedEvent),
    #[serde(rename = "error")]
    Error(ResponsesErrorEvent),
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub(crate) struct ResponsesTextDeltaEvent {
    /// Unique event id, when provided by the server.
    #[serde(default)]
    pub(crate) event_id: Option<String>,
    /// Monotonic sequence number for ordered delivery, when provided.
    #[serde(default)]
    pub(crate) sequence_number: Option<u64>,
    /// Response id that this event belongs to.
    #[serde(default)]
    pub(crate) response_id: Option<String>,
    /// Output item id (message or tool call) being streamed.
    pub(crate) item_id: String,
    /// Index of the output item in the response output array.
    pub(crate) output_index: usize,
    /// Index of the content part within the output item.
    pub(crate) content_index: usize,
    /// Delta text content.
    pub(crate) delta: String,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub(crate) struct ResponsesTextDoneEvent {
    /// Unique event id, when provided by the server.
    #[serde(default)]
    pub(crate) event_id: Option<String>,
    /// Monotonic sequence number for ordered delivery, when provided.
    #[serde(default)]
    pub(crate) sequence_number: Option<u64>,
    /// Response id that this event belongs to.
    #[serde(default)]
    pub(crate) response_id: Option<String>,
    /// Output item id (message or tool call) being streamed.
    pub(crate) item_id: String,
    /// Index of the output item in the response output array.
    pub(crate) output_index: usize,
    /// Index of the content part within the output item.
    pub(crate) content_index: usize,
    /// Final text content for the content part.
    pub(crate) text: String,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub(crate) struct ResponsesReasoningTextDeltaEvent {
    /// Unique event id, when provided by the server.
    #[serde(default)]
    pub(crate) event_id: Option<String>,
    /// Monotonic sequence number for ordered delivery, when provided.
    #[serde(default)]
    pub(crate) sequence_number: Option<u64>,
    /// Response id that this event belongs to.
    #[serde(default)]
    pub(crate) response_id: Option<String>,
    /// Output item id (message or tool call) being streamed.
    pub(crate) item_id: String,
    /// Index of the output item in the response output array.
    pub(crate) output_index: usize,
    /// Index of the content part within the output item.
    pub(crate) content_index: usize,
    /// Delta reasoning text content.
    pub(crate) delta: String,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub(crate) struct ResponsesReasoningTextDoneEvent {
    /// Unique event id, when provided by the server.
    #[serde(default)]
    pub(crate) event_id: Option<String>,
    /// Monotonic sequence number for ordered delivery, when provided.
    #[serde(default)]
    pub(crate) sequence_number: Option<u64>,
    /// Response id that this event belongs to.
    #[serde(default)]
    pub(crate) response_id: Option<String>,
    /// Output item id (message or tool call) being streamed.
    pub(crate) item_id: String,
    /// Index of the output item in the response output array.
    pub(crate) output_index: usize,
    /// Index of the content part within the output item.
    pub(crate) content_index: usize,
    /// Final reasoning text content for the content part.
    pub(crate) text: String,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub(crate) struct ResponsesFunctionCallArgumentsDeltaEvent {
    /// Unique event id, when provided by the server.
    #[serde(default)]
    pub(crate) event_id: Option<String>,
    /// Monotonic sequence number for ordered delivery, when provided.
    #[serde(default)]
    pub(crate) sequence_number: Option<u64>,
    /// Response id that this event belongs to.
    #[serde(default)]
    pub(crate) response_id: Option<String>,
    /// Output item id for the function call.
    pub(crate) item_id: String,
    /// Index of the output item in the response output array.
    pub(crate) output_index: usize,
    /// Delta JSON arguments string.
    pub(crate) delta: String,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub(crate) struct ResponsesFunctionCallArgumentsDoneEvent {
    /// Unique event id, when provided by the server.
    #[serde(default)]
    pub(crate) event_id: Option<String>,
    /// Monotonic sequence number for ordered delivery, when provided.
    #[serde(default)]
    pub(crate) sequence_number: Option<u64>,
    /// Response id that this event belongs to.
    #[serde(default)]
    pub(crate) response_id: Option<String>,
    /// Output item id for the function call, when provided.
    #[serde(default)]
    pub(crate) item_id: Option<String>,
    /// Index of the output item in the response output array.
    pub(crate) output_index: usize,
    /// Final JSON arguments string, if provided directly.
    #[serde(default)]
    pub(crate) arguments: Option<String>,
    /// Full output item payload, if provided.
    #[serde(default)]
    pub(crate) item: Option<ResponsesOutputItem>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub(crate) struct ResponsesOutputItemEvent {
    /// Unique event id, when provided by the server.
    #[serde(default)]
    pub(crate) event_id: Option<String>,
    /// Monotonic sequence number for ordered delivery, when provided.
    #[serde(default)]
    pub(crate) sequence_number: Option<u64>,
    /// Response id that this event belongs to.
    #[serde(default)]
    pub(crate) response_id: Option<String>,
    /// Index of the output item in the response output array.
    pub(crate) output_index: usize,
    /// Output item payload (message, function call, etc.).
    pub(crate) item: ResponsesOutputItem,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub(crate) struct ResponsesCompletedEvent {
    /// Unique event id, when provided by the server.
    #[serde(default)]
    pub(crate) event_id: Option<String>,
    /// Full response payload at completion, when provided.
    #[serde(default)]
    pub(crate) response: Option<OpenAIResponsesResponse>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ResponsesErrorEvent {
    /// Structured error object, when provided.
    #[serde(default)]
    pub(crate) error: Option<ResponsesError>,
    /// Fallback error message, when provided.
    #[serde(default)]
    pub(crate) message: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ResponsesOutputItem {
    Message {
        /// Output item id.
        #[serde(default)]
        id: Option<String>,
        /// Status string for the output item.
        #[serde(default)]
        status: Option<String>,
        /// Role for message items.
        #[serde(default)]
        role: Option<String>,
        /// Message content parts.
        #[serde(default)]
        content: Vec<ResponseOutputContent>,
    },
    FunctionCall {
        /// Output item id.
        #[serde(default)]
        id: Option<String>,
        /// Tool call id.
        #[serde(default)]
        call_id: Option<String>,
        /// Tool/function name.
        #[serde(default)]
        name: Option<String>,
        /// JSON arguments string.
        #[serde(default)]
        arguments: Option<String>,
        /// Status string for the output item.
        #[serde(default)]
        status: Option<String>,
    },
    FunctionCallOutput {
        /// Output item id.
        #[serde(default)]
        id: Option<String>,
        /// Tool call id.
        #[serde(default)]
        call_id: Option<String>,
        /// Tool output payload.
        #[serde(default)]
        output: Option<serde_json::Value>,
        /// Status string for the output item.
        #[serde(default)]
        status: Option<String>,
    },
    #[serde(other)]
    Unknown,
}

// ============================================================================
// Chat Completions Request Types
// ============================================================================

/// Chat completions API request payload.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ChatCompletionsPayload {
    pub(crate) model: String,
    pub(crate) messages: Vec<ChatCompletionsRequestMessage>,
    pub(crate) temperature: f32,
    pub(crate) max_tokens: i32,
    pub(crate) stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tools: Option<Vec<ChatCompletionsRequestTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tool_choice: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reasoning_effort: Option<String>,
}

/// A single message in a chat completions request.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ChatCompletionsRequestMessage {
    pub(crate) role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reasoning_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tool_calls: Option<Vec<ChatCompletionsRequestToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tool_call_id: Option<String>,
}

/// A tool call inside an assistant request message.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ChatCompletionsRequestToolCall {
    pub(crate) id: String,
    #[serde(rename = "type")]
    pub(crate) kind: &'static str,
    pub(crate) function: ChatCompletionsRequestFunctionCall,
}

/// Function call inside a request tool call.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ChatCompletionsRequestFunctionCall {
    pub(crate) name: String,
    pub(crate) arguments: String,
}

/// Tool definition for chat completions request (nested under "function").
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ChatCompletionsRequestTool {
    #[serde(rename = "type")]
    pub(crate) kind: &'static str,
    pub(crate) function: ChatCompletionsRequestToolFunction,
}

/// Function metadata inside a request tool definition.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ChatCompletionsRequestToolFunction {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) parameters: JsonSchema,
}

// ============================================================================
// Chat Completions Response Types
// ============================================================================

/// Chat completions API response payload.
#[derive(Debug, Deserialize)]
pub(crate) struct ChatCompletionsResponse {
    pub choices: Vec<ChatCompletionsChoice>,
    #[serde(default)]
    pub usage: Option<ChatCompletionsUsage>,
}

/// A single choice (message) in a chat completions response.
#[derive(Debug, Deserialize)]
pub(crate) struct ChatCompletionsChoice {
    pub message: ChatCompletionsResponseMessage,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

/// Response message from chat completions.
#[derive(Debug, Deserialize)]
pub(crate) struct ChatCompletionsResponseMessage {
    #[serde(default)]
    pub content: Option<ChatCompletionsResponseContent>,
    #[serde(default)]
    pub tool_calls: Option<Vec<ChatCompletionsResponseToolCall>>,
    #[serde(default, alias = "reasoningContent")]
    pub reasoning_content: Option<String>,
}

/// Message content that can be either a plain string or an array of content blocks.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub(crate) enum ChatCompletionsResponseContent {
    Text(String),
    Blocks(Vec<ChatCompletionsResponseContentBlock>),
}

/// A single content block inside a chat completions response message.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ChatCompletionsResponseContentBlock {
    Text {
        text: String,
    },
    Thinking {
        thinking: String,
    },
    #[serde(other)]
    Unknown,
}

/// A tool call inside a chat completions response message.
#[derive(Debug, Deserialize)]
pub(crate) struct ChatCompletionsResponseToolCall {
    pub id: String,
    pub function: ChatCompletionsResponseFunctionCall,
}

/// Function call details inside a response tool call.
#[derive(Debug, Deserialize)]
pub(crate) struct ChatCompletionsResponseFunctionCall {
    pub name: String,
    pub arguments: String,
}

/// Token usage statistics from chat completions.
#[derive(Debug, Deserialize)]
pub(crate) struct ChatCompletionsUsage {
    #[serde(default)]
    pub prompt_tokens: Option<u64>,
    #[serde(default)]
    pub completion_tokens: Option<u64>,
    #[serde(default)]
    pub total_tokens: Option<u64>,
}
