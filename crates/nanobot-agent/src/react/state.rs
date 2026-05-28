//! ReAct loop state machine types

use nanobot_types::provider::{ChatMessage, ToolCallRequest, UsageStats};

/// State of the ReAct loop
#[derive(Debug, Clone)]
pub enum LoopState {
    /// Query the model for next action
    QueryModel { iteration: usize },
    /// Execute tool calls from the current assistant turn, one by one
    ExecuteTool {
        iteration: usize,
        step: usize,
        tool_calls: Vec<ToolCallRequest>,
    },
    /// Loop finished
    Finish { reason: LoopExitReason },
}

/// Why the loop exited
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopExitReason {
    /// Model returned final answer (no more tool calls)
    Finished,
    /// LLM provider error
    ProviderError,
    /// Hit max iteration limit
    MaxIterations,
    /// User cancelled or task aborted
    Cancelled,
}

/// Outcome of a complete ReAct loop
#[derive(Debug)]
pub struct LoopOutcome {
    /// Final text content from model (if any)
    pub final_content: Option<String>,
    /// Complete message history
    pub messages: Vec<ChatMessage>,
    /// Why the loop exited
    pub exit_reason: LoopExitReason,
    /// Number of iterations executed
    pub iterations: usize,
    /// Usage statistics for the final model call (if available).
    pub usage: Option<UsageStats>,
    /// Aggregated usage statistics across all model calls in this loop.
    pub loop_usage: Option<UsageStats>,
    /// Optional exit error details when loop exits on provider/runtime failures.
    pub error_detail: Option<String>,
}

impl LoopOutcome {
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

/// Result of a single ReAct step
#[derive(Debug)]
pub enum StepResult {
    /// Continue to next iteration
    Continue,
    /// Loop finished with final answer
    Finish(String),
    /// Unrecoverable error occurred
    Error(String),
}
