//! ReAct executor - orchestrates the Reason-Act-Observe loop

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::{debug, info, warn};

use crate::error::AgentResult;
use nanobot_provider::LLMProvider;
use nanobot_tools::{ToolContext, ToolRegistry};
use nanobot_types::provider::{AssistantToolCall, ChatMessage, UsageStats};

use super::planner::{ModelConfig, Planner, ProgressEmitter};
use super::state::{LoopExitReason, LoopOutcome, LoopState};
use super::tool_runner::ToolRunner;

use super::TARGET;

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
        tools: Vec<Arc<nanobot_tools::base::ToolDefinition>>,
        config: ModelConfig,
        context: ExecutionContext,
        progress: Option<ProgressEmitter>,
    ) -> AgentResult<LoopOutcome> {
        let mut state = LoopState::QueryModel { iteration: 0 };
        let mut iterations = 0;
        let mut last_usage: Option<UsageStats> = None;
        loop {
            // Check cancellation
            if context.is_cancelled() {
                info!(target: TARGET, "ReAct loop cancelled");
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
                        warn!(target: TARGET, iteration, "Max iterations reached");
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
                            if response.is_truncated() {
                                warn!(
                                    target: TARGET,
                                    iteration,
                                    "Response truncated due to max_tokens limit"
                                );
                                if let Some(content) = response.content {
                                    messages.push(ChatMessage::assistant(
                                        Some(content.clone()),
                                        None,
                                        response.reasoning_content,
                                        response.thinking_blocks,
                                    ));
                                }
                                state = LoopState::QueryModel {
                                    iteration: iteration + 1,
                                };
                                continue;
                            }

                            if response.is_final() {
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
                                    return Ok(LoopOutcome::new(
                                        None,
                                        messages,
                                        LoopExitReason::Finished,
                                        iterations,
                                        last_usage.clone(),
                                    ));
                                }
                            } else {
                                debug!(
                                    target: TARGET,
                                    iteration,
                                    "Model wants to use tools"
                                );
                                let tool_calls: Vec<nanobot_types::provider::ToolCallRequest> =
                                    response
                                        .tool_calls
                                        .iter()
                                        .map(|tc| nanobot_types::provider::ToolCallRequest {
                                            id: tc.id.clone(),
                                            name: tc.name.clone(),
                                            arguments_json: tc.arguments_json.clone(),
                                        })
                                        .collect();

                                let assistant_tool_calls: Vec<AssistantToolCall> = response
                                    .tool_calls
                                    .iter()
                                    .map(|tc| AssistantToolCall {
                                        id: tc.id.clone(),
                                        kind: "function".to_string(),
                                        function: nanobot_types::provider::AssistantFunctionCall {
                                            name: tc.name.to_string(),
                                            arguments: tc.arguments_json.clone(),
                                        },
                                    })
                                    .collect();

                                state = LoopState::ExecuteTool {
                                    iteration,
                                    step: 0,
                                    tool_calls,
                                };

                                messages.push(ChatMessage::assistant(
                                    response.content,
                                    Some(assistant_tool_calls),
                                    response.reasoning_content,
                                    response.thinking_blocks,
                                ));
                            }
                        }
                        Err(err) => {
                            warn!(target: TARGET, error = %err, "Provider error");
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
                LoopState::ExecuteTool {
                    iteration,
                    step,
                    tool_calls,
                } => {
                    debug!(target: TARGET, iteration, step, "Executing tool");

                    if tool_calls.is_empty() || step >= tool_calls.len() {
                        warn!(
                            target: TARGET,
                            "No pending tool calls found in assistant message"
                        );
                        state = LoopState::QueryModel {
                            iteration: iteration + 1,
                        };
                        continue;
                    }

                    let current_call = &tool_calls[step];

                    if let Some(progress) = &progress {
                        progress.send_tool_hint(&format!(
                            "Executing tool: {} (id={})",
                            current_call.name, current_call.id
                        ));
                    }

                    let tool_context = context.to_tool_context();
                    let observation = self
                        .tool_runner
                        .execute_one(current_call, &tool_context)
                        .await;
                    let obs_content = observation.content;
                    let obs_content_for_hint = obs_content.clone();
                    messages.push(ChatMessage::tool_result(
                        observation.tool_call_id,
                        current_call.name.to_string(),
                        obs_content,
                    ));

                    if let Some(progress) = &progress {
                        let result_preview = truncate_tool_result(&obs_content_for_hint);
                        progress.send_tool_hint(&format!(
                            "Tool result: {} (id={}) -> {}",
                            current_call.name, current_call.id, result_preview
                        ));
                    }

                    if step + 1 < tool_calls.len() {
                        state = LoopState::ExecuteTool {
                            iteration,
                            step: step + 1,
                            tool_calls,
                        };
                    } else {
                        state = LoopState::QueryModel {
                            iteration: iteration + 1,
                        };
                    }
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
    pub session_key: nanobot_types::SessionKey,
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
    format!("{}\u{2026}", truncated)
}
