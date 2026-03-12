//! ReAct executor - orchestrates the Reason-Act-Observe loop

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::{debug, info, warn};

use crate::error::Result;
use crate::observability::TARGET_REACT;
use crate::provider::LLMProvider;
use crate::tools::{ToolContext, ToolRegistry};
use crate::types::provider::{AssistantToolCall, ChatMessage, UsageStats};

use super::planner::{ModelConfig, Planner, ProgressEmitter};
use super::state::{LoopExitReason, LoopOutcome, LoopState};
use super::tool_runner::ToolRunner;

const TOOL_RESULT_MAX_CHARS: usize = 480;

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
        tools: Vec<Arc<crate::tools::base::ToolDefinition>>,
        config: ModelConfig,
        context: ExecutionContext,
        progress: Option<ProgressEmitter>,
    ) -> Result<LoopOutcome> {
        let mut state = LoopState::QueryModel { iteration: 0 };
        let mut iterations = 0;
        let mut last_usage: Option<UsageStats> = None;

        loop {
            // Check cancellation
            if context.is_cancelled() {
                info!(target: TARGET_REACT, "ReAct loop cancelled");
                return Ok(LoopOutcome::new(
                    None,
                    messages,
                    LoopExitReason::Cancelled,
                    iterations,
                    last_usage.clone(),
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
                            last_usage.clone(),
                        ));
                    }

                    match self
                        .planner
                        .query(&messages, &tools, &config, progress.as_ref())
                        .await
                    {
                        Ok(response) => {
                            last_usage = Some(response.usage.clone());
                            // Check if response was truncated due to max_tokens
                            if response.is_truncated() {
                                warn!(
                                    target: TARGET_REACT,
                                    iteration,
                                    "Response truncated due to max_tokens limit"
                                );
                                // Add truncated content to messages
                                if let Some(content) = response.content {
                                    messages.push(ChatMessage::assistant(
                                        Some(content.clone()),
                                        None,
                                        response.reasoning_content,
                                        response.thinking_blocks,
                                    ));
                                }
                                // Continue to next iteration to let model complete the response
                                state = LoopState::QueryModel {
                                    iteration: iteration + 1,
                                };
                                continue;
                            }

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
                                        last_usage.clone(),
                                    ));
                                } else {
                                    // Empty response, treat as finished
                                    return Ok(LoopOutcome::new(
                                        None,
                                        messages,
                                        LoopExitReason::Finished,
                                        iterations,
                                        last_usage.clone(),
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
                                last_usage.clone(),
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

                    if let Some(progress) = &progress {
                        let tc = &tool_calls[0];
                        progress
                            .send_tool_hint(&format!("Running tool: {} (id={})", tc.name, tc.id));
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
                    let obs_content_for_hint = obs_content.clone();
                    messages.push(ChatMessage::tool_result(
                        observation.tool_call_id,
                        tool_calls[0].name.to_string(),
                        obs_content,
                    ));

                    if let Some(progress) = &progress {
                        let result_preview = truncate_tool_result(&obs_content_for_hint);
                        progress.send_tool_hint(&format!(
                            "Tool result: {} (id={}) -> {}",
                            tool_calls[0].name, tool_calls[0].id, result_preview
                        ));
                    }

                    // Move to next iteration
                    state = LoopState::QueryModel {
                        iteration: iteration + 1,
                    };
                }

                LoopState::Finish { reason } => {
                    return Ok(LoopOutcome::new(
                        None,
                        messages,
                        reason,
                        iterations,
                        last_usage.clone(),
                    ));
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

fn truncate_tool_result(value: &str) -> String {
    if value.len() <= TOOL_RESULT_MAX_CHARS {
        return value.to_string();
    }
    let truncated: String = value.chars().take(TOOL_RESULT_MAX_CHARS).collect();
    format!("{}…", truncated)
}
