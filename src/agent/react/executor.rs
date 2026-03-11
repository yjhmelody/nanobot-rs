//! ReAct executor - orchestrates the Reason-Act-Observe loop

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::{debug, info, warn};

use crate::error::Result;
use crate::observability::TARGET_REACT;
use crate::provider::LLMProvider;
use crate::tools::{ToolContext, ToolRegistry};
use crate::types::provider::{AssistantToolCall, ChatMessage};

use super::planner::{ModelConfig, Planner};
use super::state::{LoopExitReason, LoopOutcome, LoopState};
use super::tool_runner::ToolRunner;

/// ReAct loop executor
pub struct ReActExecutor {
    planner: Planner,
    tool_runner: ToolRunner,
    max_iterations: usize,
}

impl ReActExecutor {
    pub fn new(
        provider: Arc<dyn LLMProvider>,
        tools: Arc<ToolRegistry>,
        max_iterations: usize,
    ) -> Self {
        Self {
            planner: Planner::new(provider),
            tool_runner: ToolRunner::new(tools),
            max_iterations,
        }
    }

    /// Run the complete ReAct loop
    pub async fn run(
        &self,
        mut messages: Vec<ChatMessage>,
        tools: Vec<crate::tools::base::ToolDefinition>,
        config: ModelConfig,
        context: ExecutionContext,
    ) -> Result<LoopOutcome> {
        let mut state = LoopState::QueryModel { iteration: 0 };
        let mut iterations = 0;

        loop {
            // Check cancellation
            if context.is_cancelled() {
                info!(target: TARGET_REACT, "ReAct loop cancelled");
                return Ok(LoopOutcome::new(
                    None,
                    messages,
                    LoopExitReason::Cancelled,
                    iterations,
                ));
            }

            match state {
                LoopState::QueryModel { iteration } => {
                    iterations = iteration;

                    if iteration >= self.max_iterations {
                        warn!(target: TARGET_REACT, iteration, "Max iterations reached");
                        return Ok(LoopOutcome::new(
                            None,
                            messages,
                            LoopExitReason::MaxIterations,
                            iterations,
                        ));
                    }

                    match self.planner.query(&messages, &tools, &config).await {
                        Ok(response) => {
                            if response.is_final() {
                                // Model returned final answer
                                if let Some(content) = response.content {
                                    messages.push(ChatMessage::assistant(
                                        Some(content.clone()),
                                        None,
                                        response.reasoning_content,
                                        response.thinking_blocks,
                                    ));

                                    return Ok(LoopOutcome::new(
                                        Some(content),
                                        messages,
                                        LoopExitReason::Finished,
                                        iterations,
                                    ));
                                } else {
                                    // Empty response, treat as finished
                                    return Ok(LoopOutcome::new(
                                        None,
                                        messages,
                                        LoopExitReason::Finished,
                                        iterations,
                                    ));
                                }
                            } else {
                                // Model wants to use tools - convert ToolCallRequest to AssistantToolCall
                                debug!(
                                    target: TARGET_REACT,
                                    iteration,
                                    "Model wants to use tools"
                                );
                                let assistant_tool_calls: Vec<AssistantToolCall> = response
                                    .tool_calls
                                    .iter()
                                    .map(|tc| AssistantToolCall {
                                        id: tc.id.clone(),
                                        kind: "function".to_string(),
                                        function: crate::types::provider::AssistantFunctionCall {
                                            name: tc.name.to_string(),
                                            arguments: tc.arguments_json.clone(),
                                        },
                                    })
                                    .collect();

                                state = LoopState::ExecuteTool { iteration, step: 0 };

                                // Add assistant message with tool calls
                                messages.push(ChatMessage::assistant(
                                    response.content,
                                    Some(assistant_tool_calls),
                                    response.reasoning_content,
                                    response.thinking_blocks,
                                ));

                                // Store original tool call requests for execution
                                // We'll retrieve them in ExecuteTool state
                            }
                        }
                        Err(err) => {
                            warn!(target: TARGET_REACT, error = %err, "Provider error");
                            return Ok(LoopOutcome::new(
                                None,
                                messages,
                                LoopExitReason::ProviderError,
                                iterations,
                            ));
                        }
                    }
                }

                LoopState::ExecuteTool { iteration, step } => {
                    debug!(target: TARGET_REACT, iteration, step, "Executing tool");

                    // Get tool calls from last assistant message and convert back to ToolCallRequest
                    let tool_calls: Vec<crate::types::provider::ToolCallRequest> = messages
                        .last()
                        .and_then(|m| m.tool_calls.as_ref())
                        .map(|calls| {
                            calls
                                .iter()
                                .map(|tc| crate::types::provider::ToolCallRequest {
                                    id: tc.id.clone(),
                                    name: tc.function.name.as_str().into(),
                                    arguments_json: tc.function.arguments.clone(),
                                })
                                .collect()
                        })
                        .unwrap_or_default();

                    if tool_calls.is_empty() {
                        warn!(
                            target: TARGET_REACT,
                            "No tool calls found in assistant message"
                        );
                        state = LoopState::QueryModel {
                            iteration: iteration + 1,
                        };
                        continue;
                    }

                    // Execute first tool, get diagnostic if multiple
                    let tool_context = context.to_tool_context();
                    let (observation, diagnostic) = self
                        .tool_runner
                        .execute_with_diagnostic(&tool_calls, &tool_context)
                        .await;

                    // Format observation message
                    let mut obs_content = observation.content;
                    if let Some(diag) = diagnostic {
                        obs_content = format!("{}\n\n{}", diag, obs_content);
                    }

                    // Add tool result message
                    messages.push(ChatMessage::tool_result(
                        observation.tool_call_id,
                        tool_calls[0].name.to_string(),
                        obs_content,
                    ));

                    // Move to next iteration
                    state = LoopState::QueryModel {
                        iteration: iteration + 1,
                    };
                }

                LoopState::Finish { reason } => {
                    return Ok(LoopOutcome::new(None, messages, reason, iterations));
                }
            }
        }
    }
}

/// Execution context for ReAct loop
#[derive(Clone)]
pub struct ExecutionContext {
    pub session_key: crate::types::SessionKey,
    pub channel: String,
    pub chat_id: String,
    pub cancelled: Arc<AtomicBool>,
}

impl ExecutionContext {
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    pub fn to_tool_context(&self) -> ToolContext {
        ToolContext {
            channel: self.channel.clone(),
            chat_id: self.chat_id.clone(),
            session_key: self.session_key.clone(),
            message_id: None,
        }
    }
}
