use std::fmt::Debug;

use async_trait::async_trait;

use crate::tools::base::ToolDefinition;
use crate::types::provider::{ChatMessage, LLMResponse};

#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    pub tools: Option<Vec<ToolDefinition>>,
    pub model: Option<String>,
    pub max_tokens: i32,
    pub temperature: f32,
    pub reasoning_effort: Option<String>,
}

#[async_trait]
pub trait LLMProvider: Send + Sync {
    fn default_model(&self) -> &str;

    async fn chat(&self, req: ChatRequest) -> LLMResponse;
}
