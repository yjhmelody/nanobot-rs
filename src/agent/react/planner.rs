//! Model query and response parsing for ReAct loop

use std::sync::Arc;
use tracing::{debug, trace};

use crate::error::Result;
use crate::observability::TARGET_REACT;
use crate::provider::{ChatRequest, LLMProvider};
use crate::tools::base::ToolDefinition;
use crate::types::provider::{ChatMessage, ToolCallRequest};

/// Queries the model and parses responses
pub struct Planner {
    provider: Arc<dyn LLMProvider>,
}

impl Planner {
    pub fn new(provider: Arc<dyn LLMProvider>) -> Self {
        Self { provider }
    }

    /// Query model with current messages and available tools
    pub async fn query(
        &self,
        messages: &[ChatMessage],
        tools: &[Arc<ToolDefinition>],
        config: &ModelConfig,
    ) -> Result<PlannerResponse> {
        debug!(
            target: TARGET_REACT,
            iteration = config.iteration,
            message_count = messages.len(),
            "Querying model"
        );

        let request = ChatRequest {
            session_key: None,
            model: Some(config.model.clone()),
            messages: messages.to_vec(),
            tools: if tools.is_empty() {
                None
            } else {
                Some(tools.to_vec())
            },
            temperature: config.temperature,
            max_tokens: config.max_tokens,
            reasoning_effort: config.reasoning_effort.clone(),
        };

        let response = self.provider.chat(request).await;

        trace!(
            target: TARGET_REACT,
            content_len = response.content.as_ref().map(|s| s.len()).unwrap_or(0),
            tool_calls = response.tool_calls.len(),
            "Model response received"
        );

        Ok(PlannerResponse {
            content: response.content,
            tool_calls: response.tool_calls,
            finish_reason: response.finish_reason,
            reasoning_content: response.reasoning_content,
            thinking_blocks: response.thinking_blocks,
        })
    }
}

/// Configuration for model query
#[derive(Debug, Clone)]
pub struct ModelConfig {
    pub model: String,
    pub temperature: f32,
    pub max_tokens: i32,
    pub reasoning_effort: Option<String>,
    pub iteration: usize,
}

/// Response from model query
#[derive(Debug)]
pub struct PlannerResponse {
    pub content: Option<String>,
    pub tool_calls: Vec<ToolCallRequest>,
    pub finish_reason: String,
    pub reasoning_content: Option<String>,
    pub thinking_blocks: Option<Vec<String>>,
}

impl PlannerResponse {
    /// Check if this is a final answer (no tool calls)
    pub fn is_final(&self) -> bool {
        self.tool_calls.is_empty()
    }

    /// Check if model wants to use tools
    pub fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }
}
