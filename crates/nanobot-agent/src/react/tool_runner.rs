//! Tool execution for the ReAct loop.
//!
//! [`ToolRunner`] wraps a [`ToolRegistry`] and provides a clean interface
//! for executing a single tool call and returning an observation. It also
//! provides a diagnostic variant that warns the model when it requests
//! multiple tools in one turn (the executor only runs one at a time).

use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, warn};

use super::TARGET;
use crate::loop_core::AgentLoop;
use nanobot_tools::{ToolContext, ToolRegistry};
use nanobot_types::provider::ToolCallRequest;

/// Executes tool calls and returns observations for the ReAct loop.
pub struct ToolRunner {
    tools: Arc<ToolRegistry>,
}

impl ToolRunner {
    /// Creates a new `ToolRunner` backed by the given tool registry.
    pub fn new(tools: Arc<ToolRegistry>) -> Self {
        Self { tools }
    }

    /// Executes a single tool call and returns the observation (result or
    /// formatted error).
    ///
    /// Errors are wrapped using [`AgentLoop::format_internal_error`] so
    /// they are presented consistently to the model.
    pub async fn execute_one(
        &self,
        tool_call: &ToolCallRequest,
        context: &ToolContext,
    ) -> ToolObservation {
        debug!(
            target: TARGET,
            tool_name = %tool_call.name,
            tool_call_id = %tool_call.id,
            "Executing tool"
        );
        let start = Instant::now();

        match self
            .tools
            .execute(tool_call.name.as_str(), &tool_call.arguments_json, context)
            .await
        {
            Ok(result) => {
                debug!(
                    target: TARGET,
                    tool_name = %tool_call.name,
                    tool_call_id = %tool_call.id,
                    elapsed_ms = start.elapsed().as_millis(),
                    result_len = result.len(),
                    "Tool execution completed"
                );
                ToolObservation {
                    tool_call_id: tool_call.id.clone(),
                    content: result,
                }
            }
            Err(err) => {
                warn!(
                    target: TARGET,
                    tool_name = %tool_call.name,
                    tool_call_id = %tool_call.id,
                    elapsed_ms = start.elapsed().as_millis(),
                    error = %err,
                    "Tool execution failed"
                );
                ToolObservation {
                    tool_call_id: tool_call.id.clone(),
                    content: AgentLoop::format_internal_error(format!("Error: {}", err)),
                }
            }
        }
    }

    /// Executes the first tool call and returns a diagnostic string if
    /// multiple tool calls were provided.
    ///
    /// This is a legacy variant — the current executor calls
    /// [`execute_one`] per call rather than batching.  The diagnostic
    /// warns the model that extra tool calls were ignored.
    pub async fn execute_with_diagnostic(
        &self,
        tool_calls: &[ToolCallRequest],
        context: &ToolContext,
    ) -> (ToolObservation, Option<String>) {
        if tool_calls.is_empty() {
            return (
                ToolObservation {
                    tool_call_id: "none".to_string(),
                    content: "No tool calls provided".to_string(),
                },
                None,
            );
        }

        let diagnostic = if tool_calls.len() > 1 {
            let extra_tools: Vec<_> = tool_calls
                .iter()
                .skip(1)
                .map(|tc| tc.name.to_string())
                .collect();
            Some(format!(
                "[Host diagnostic] You requested {} tool calls, but only one tool can be executed per iteration. \
                The following tools were ignored: {}. Please review the observation below and plan your next action accordingly.",
                tool_calls.len(),
                extra_tools.join(", ")
            ))
        } else {
            None
        };

        let observation = self.execute_one(&tool_calls[0], context).await;
        (observation, diagnostic)
    }
}

/// The result of executing a single tool call.
#[derive(Debug, Clone)]
pub struct ToolObservation {
    /// The tool call ID for correlation with the assistant message.
    pub(crate) tool_call_id: String,
    /// The output content (tool result or formatted error message).
    pub(crate) content: String,
}
