//! ReAct loop state machine types.
//!
//! Defines the state enum ([`LoopState`]), exit reasons
//! ([`LoopExitReason`]), and the final outcome ([`LoopOutcome`]) for
//! the ReAct execution loop.

use nanobot_types::provider::{ChatMessage, ToolCallRequest, UsageStats};

/// States of the ReAct state machine.
///
/// The loop transitions between these states as follows:
///
/// * `QueryModel` — Sends messages to the LLM. On response, either
///   transitions to `ExecuteTool` (if tool calls are requested) or
///   `Finish` (if the model returns a final answer).
/// * `ExecuteTool` — Executes one tool call at a time. After all calls
///   are done, transitions back to `QueryModel`.
/// * `Finish` — Terminal state, returned as [`LoopOutcome`].
#[derive(Debug, Clone)]
pub enum LoopState {
    /// Query the model for the next action or final answer.
    QueryModel { iteration: usize },
    /// Execute tool calls from the current assistant turn, one by one.
    ExecuteTool {
        iteration: usize,
        step: usize,
        tool_calls: Vec<ToolCallRequest>,
    },
    /// Loop has finished (terminal state).
    Finish { reason: LoopExitReason },
}

/// Reasons why the ReAct loop exited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopExitReason {
    /// Model returned a final answer (no more tool calls).
    Finished,
    /// An LLM provider error occurred.
    ProviderError,
    /// The maximum number of iterations was reached.
    MaxIterations,
    /// The user cancelled the task or a /cancel command was issued.
    Cancelled,
}

/// The complete outcome of a ReAct loop execution.
///
/// Contains the final response (if any), the full message history
/// (including tool results), why the loop exited, iteration count, and
/// token usage statistics.
#[derive(Debug)]
pub struct LoopOutcome {
    /// Final text content from the model (if any).
    pub final_content: Option<String>,
    /// Complete message history from the loop, including system prompt,
    /// assistant messages, tool results, and the final response.
    pub messages: Vec<ChatMessage>,
    /// Why the loop exited.
    pub exit_reason: LoopExitReason,
    /// Number of ReAct iterations executed (model queries).
    pub iterations: usize,
    /// Token usage statistics for the final model call, if available.
    pub usage: Option<UsageStats>,
    /// Aggregated token usage across all model calls in this loop.
    pub loop_usage: Option<UsageStats>,
    /// Optional error detail when exiting due to provider/runtime failure.
    pub error_detail: Option<String>,
}

impl LoopOutcome {
    /// Creates a new `LoopOutcome`.
    pub fn new(
        final_content: Option<String>,
        messages: Vec<ChatMessage>,
        exit_reason: LoopExitReason,
        iterations: usize,
        usage: Option<UsageStats>,
        loop_usage: Option<UsageStats>,
        error_detail: Option<String>,
    ) -> Self {
        Self {
            final_content,
            messages,
            exit_reason,
            iterations,
            usage,
            loop_usage,
            error_detail,
        }
    }
}

/// Result of a single ReAct step (kept for compatibility but not
/// currently used by the executor).
#[derive(Debug)]
pub enum StepResult {
    /// Continue to the next iteration.
    Continue,
    /// Loop finished with a final answer string.
    Finish(String),
    /// An unrecoverable error occurred.
    Error(String),
}
