//! ReAct loop executor — orchestrates the Reason-Act-Observe state machine.
//!
//! [`ReActExecutor`] drives the core loop: query the model, execute tools,
//! observe results, and repeat until a final answer is produced, the
//! iteration limit is reached, or the user cancels.
//!
//! # State Machine
//!
//! See [`LoopState`] for the state transitions:
//!
//! ```text
//! QueryModel ──(has tool calls)──▶ ExecuteTool ──(all done)──▶ QueryModel
//!      │                               │
//!      └──(is final)──▶ Finish         └──(cancelled)──▶ Finish
//!      └──(is truncated)──▶ QueryModel (next iteration)
//! ```
//!
//! # Cancellation
//!
//! The loop checks the `cancelled` [`AtomicBool`] at the start of each
//! iteration and before every tool execution, enabling prompt preemption.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::{debug, info, warn};

use crate::error::AgentResult;
use nanobot_provider::LLMProvider;
use nanobot_tools::{ToolContext, ToolRegistry};
use nanobot_types::provider::{AssistantToolCall, ChatMessage, UsageStats};

use nanobot_types::text::truncate_text;

use super::planner::{ModelConfig, Planner, ProgressEmitter};
use super::state::{LoopExitReason, LoopOutcome, LoopState};
use super::tool_runner::ToolRunner;

use super::TARGET;

/// Maximum characters of a tool result to include in a progress hint.
const TOOL_RESULT_MAX_CHARS: usize = 480;

/// Drives the ReAct (Reason-Act-Observe) loop for a single turn.
///
/// Wraps a [`Planner`] for model queries and a [`ToolRunner`] for tool
/// execution, cycling through states until termination.
pub struct ReActExecutor {
    planner: Planner,
    tool_runner: ToolRunner,
    max_iterations: usize,
}

impl ReActExecutor {
    /// Creates a new `ReActExecutor`.
    ///
    /// * `provider` — LLM provider for model queries.
    /// * `tools` — Tool registry for executing tool calls.
    /// * `max_iterations` — Maximum ReAct iterations before forcing exit.
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

    /// Runs the complete ReAct loop with the given initial messages.
    ///
    /// # Arguments
    ///
    /// * `messages` — Initial conversation messages (system prompt,
    ///   history, user message).
    /// * `tools` — Tool definitions to provide to the model.
    /// * `config` — Model configuration for each query.
    /// * `context` — Execution context (session key, channel, cancellation
    ///   signal).
    /// * `progress` — Optional progress emitter for streaming updates.
    ///
    /// # Returns
    ///
    /// A [`LoopOutcome`] containing the final response, exit reason,
    /// iteration count, and usage statistics.
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
        let mut loop_usage: Option<UsageStats> = None;
        loop {
            // Check cancellation at the top of every iteration
            if context.is_cancelled() {
                info!(target: TARGET, "ReAct loop cancelled");
                return Ok(LoopOutcome::new(
                    None,
                    messages,
                    LoopExitReason::Cancelled,
                    iterations,
                    last_usage.clone(),
                    loop_usage.clone(),
                    None,
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
                            loop_usage.clone(),
                            None,
                        ));
                    }

                    match self
                        .planner
                        .query(&messages, &tools, &config, progress.as_ref())
                        .await
                    {
                        Ok(response) => {
                            last_usage = Some(response.usage.clone());
                            accumulate_loop_usage(&mut loop_usage, &response.usage);
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
                                        loop_usage.clone(),
                                        None,
                                    ));
                                } else {
                                    return Ok(LoopOutcome::new(
                                        None,
                                        messages,
                                        LoopExitReason::Finished,
                                        iterations,
                                        last_usage.clone(),
                                        loop_usage.clone(),
                                        None,
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
                                loop_usage.clone(),
                                Some(err.to_string()),
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

                    // Check cancellation before each tool execution, so long-running
                    // tool sequences can be interrupted between calls.
                    if context.is_cancelled() {
                        debug!(
                            target: TARGET,
                            iteration,
                            step,
                            "ReAct loop cancelled before tool execution"
                        );
                        return Ok(LoopOutcome::new(
                            None,
                            messages,
                            LoopExitReason::Cancelled,
                            iterations,
                            last_usage.clone(),
                            loop_usage.clone(),
                            None,
                        ));
                    }

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
                        let result_preview =
                            truncate_text(&obs_content_for_hint, TOOL_RESULT_MAX_CHARS);
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
                        loop_usage.clone(),
                        None,
                    ));
                }
            }
        }
    }
}

/// Execution context propagated through the ReAct loop.
///
/// Carries session metadata and a cancellation signal that the loop polls
/// at each iteration and tool boundary.
#[derive(Clone)]
pub struct ExecutionContext {
    /// The session key for this execution turn.
    pub session_key: nanobot_types::SessionKey,
    /// The originating channel.
    pub channel: String,
    /// The originating chat ID.
    pub chat_id: String,
    /// Cancellation signal shared with the parent [`AgentLoop`].
    pub cancelled: Arc<AtomicBool>,
}

impl ExecutionContext {
    /// Returns `true` if a cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    /// Converts this context into a [`ToolContext`] for tool execution.
    pub fn to_tool_context(&self) -> ToolContext {
        ToolContext {
            channel: self.channel.clone(),
            chat_id: self.chat_id.clone(),
            session_key: self.session_key.clone(),
            message_id: None,
        }
    }
}

/// Extracts the effective total token count from usage stats.
fn effective_total_tokens(usage: &UsageStats) -> Option<u64> {
    usage.total_tokens.or_else(|| {
        usage
            .prompt_tokens
            .zip(usage.completion_tokens)
            .map(|(p, c)| p + c)
    })
}

/// Accumulates token usage across multiple model calls in the same loop,
/// summing prompt, completion, and total token counts.
fn accumulate_loop_usage(acc: &mut Option<UsageStats>, usage: &UsageStats) {
    if usage.prompt_tokens.is_none()
        && usage.completion_tokens.is_none()
        && effective_total_tokens(usage).is_none()
    {
        return;
    }

    let mut current = acc.take().unwrap_or_default();

    if let Some(v) = usage.prompt_tokens {
        current.prompt_tokens = Some(current.prompt_tokens.unwrap_or(0) + v);
    }
    if let Some(v) = usage.completion_tokens {
        current.completion_tokens = Some(current.completion_tokens.unwrap_or(0) + v);
    }
    if let Some(v) = effective_total_tokens(usage) {
        current.total_tokens = Some(current.total_tokens.unwrap_or(0) + v);
    } else if let (Some(p), Some(c)) = (current.prompt_tokens, current.completion_tokens) {
        current.total_tokens = Some(p + c);
    }

    *acc = Some(current);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accumulate_loop_usage_sums_prompt_completion_and_total() {
        let mut acc = None;
        let first = UsageStats {
            prompt_tokens: Some(10),
            completion_tokens: Some(5),
            total_tokens: Some(15),
        };
        let second = UsageStats {
            prompt_tokens: Some(8),
            completion_tokens: Some(7),
            total_tokens: Some(15),
        };

        accumulate_loop_usage(&mut acc, &first);
        accumulate_loop_usage(&mut acc, &second);

        let out = acc.expect("loop usage");
        assert_eq!(out.prompt_tokens, Some(18));
        assert_eq!(out.completion_tokens, Some(12));
        assert_eq!(out.total_tokens, Some(30));
    }

    #[test]
    fn accumulate_loop_usage_derives_total_when_missing() {
        let mut acc = None;
        let usage = UsageStats {
            prompt_tokens: Some(3),
            completion_tokens: Some(2),
            total_tokens: None,
        };

        accumulate_loop_usage(&mut acc, &usage);

        let out = acc.expect("loop usage");
        assert_eq!(out.prompt_tokens, Some(3));
        assert_eq!(out.completion_tokens, Some(2));
        assert_eq!(out.total_tokens, Some(5));
    }
}
