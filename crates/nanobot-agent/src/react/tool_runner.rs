//! Tool execution for ReAct loop

use std::sync::Arc;
use tracing::{debug, warn};

use super::TARGET;
use crate::loop_core::AgentLoop;
use nanobot_tools::{ToolContext, ToolRegistry};
use nanobot_types::provider::ToolCallRequest;

/// Executes tool calls and returns observations
pub struct ToolRunner {
    tools: Arc<ToolRegistry>,
}

impl ToolRunner {
    pub fn new(tools: Arc<ToolRegistry>) -> Self {
        Self { tools }
    }

    /// Execute a single tool call and return observation
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

        match self
            .tools
            .execute(tool_call.name.as_str(), &tool_call.arguments_json, context)
            .await
        {
            Ok(result) => ToolObservation {
                tool_call_id: tool_call.id.clone(),
                content: result,
            },
            Err(err) => {
                warn!(
                    target: TARGET,
                    tool_name = %tool_call.name,
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

    /// Execute first tool call, return diagnostic if multiple provided
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

/// Result of executing a tool
#[derive(Debug, Clone)]
pub struct ToolObservation {
    pub(crate) tool_call_id: String,
    pub(crate) content: String,
}
