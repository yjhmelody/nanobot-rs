//! Tool wrapper that exposes ACP agent execution as a callable tool for the LLM.
//!
//! `ACPTool` implements the `nanobot_tools::Tool` trait, allowing the LLM to
//! delegate complex coding tasks to external ACP-compatible agents (e.g.,
//! `codex-acp`) via the `acp_execute` function.
//!
//! ## Configuration
//!
//! The tool is configured through `ACPConfig`, which defines:
//! - `allowed_agents` — which agent IDs the LLM is permitted to invoke
//! - `agents` — per-agent command, args, and environment settings
//!
//! ## Registration
//!
//! `ACPTool` is registered as a custom tool in `RuntimeBundle::build_runtime`
//! when `config.acp` is present.

use std::sync::Arc;
use std::sync::OnceLock;

use async_trait::async_trait;
use serde_json::json;

use crate::acp::client::ACPClient;
use crate::acp::config::ACPConfig;
use nanobot_tools::base::{
    Tool, ToolContext, ToolDefinition, parse_args, tool_definition_from_json,
};
use nanobot_tools::{ToolError, ToolResult};
use nanobot_types::tools::ACPExecuteArgs;

/// The tool name exposed to the LLM.
const ACP_EXECUTE_TOOL_NAME: &str = "acp_execute";
/// Description of the tool for the LLM's tool definition.
const ACP_EXECUTE_DESCRIPTION: &str = "Execute a coding task using an ACP agent. \
Use this for complex coding tasks that require multi-file edits, refactoring, or \
end-to-end feature implementation.";
/// Docstring for the `agent_id` parameter.
const ACP_AGENT_ID_DESC: &str = "ACP agent id used to execute the task";
/// Docstring for the `task` parameter.
const ACP_TASK_DESC: &str = "Coding task to execute by the ACP agent";
/// Docstring for the optional `cwd` parameter.
const ACP_CWD_DESC: &str = "Optional working directory for the ACP agent process";

/// Tool that delegates a coding task to an external ACP-compatible agent.
///
/// # Fields
///
/// * `config` — Configuration containing allowed agent IDs and per-agent
///   command/environment settings.
pub struct ACPTool {
    config: ACPConfig,
}

impl ACPTool {
    /// Create a new `ACPTool` from the given ACP configuration.
    pub fn new(config: ACPConfig) -> Self {
        Self { config }
    }

    /// Parse `ACPExecuteArgs` from the JSON string provided by the LLM.
    ///
    /// Wraps parse errors with the tool name for better diagnostics.
    fn parse_execute_args(&self, args_json: &str) -> ToolResult<ACPExecuteArgs> {
        parse_args::<ACPExecuteArgs>(args_json).map_err(|err| match err {
            ToolError::InvalidArgs { message, .. } => ToolError::invalid_args(self.name(), message),
            other => other,
        })
    }

    /// Return the sorted, deduplicated list of allowed agent IDs.
    fn allowed_agents(&self) -> Vec<String> {
        let mut allowed = self
            .config
            .allowed_agents
            .iter()
            .map(|agent| agent.trim())
            .filter(|agent| !agent.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        allowed.sort_unstable();
        allowed.dedup();
        allowed
    }

    /// Return the sorted list of configured agent IDs.
    fn configured_agents(&self) -> Vec<String> {
        let mut configured = self.config.agents.keys().cloned().collect::<Vec<_>>();
        configured.sort_unstable();
        configured
    }

    /// Look up an agent's configuration by ID, validating against the allowlist.
    ///
    /// # Errors
    ///
    /// Returns `InvalidArgs` if:
    /// - No agents are allowed (empty allowlist).
    /// - The `agent_id` is not in the allowlist.
    /// - The `agent_id` has no corresponding configuration entry.
    fn resolve_agent_config(&self, agent_id: &str) -> ToolResult<&crate::acp::config::AgentConfig> {
        let allowed = self.allowed_agents();
        if allowed.is_empty() {
            return Err(ToolError::invalid_args(
                self.name(),
                "No ACP agents are allowed. Check the acp.allowed_agents configuration.",
            ));
        }
        if !allowed.iter().any(|a| a == agent_id) {
            let allowed_text = allowed.join(", ");
            return Err(ToolError::invalid_args(
                self.name(),
                format!(
                    "Agent '{}' is not allowed. Allowed agents: {}",
                    agent_id, allowed_text
                ),
            ));
        }
        self.config.agents.get(agent_id).ok_or_else(|| {
            let configured_text = self.configured_agents().join(", ");
            ToolError::invalid_args(
                self.name(),
                format!(
                    "Agent '{}' is not configured. Configured agents: {}",
                    agent_id, configured_text
                ),
            )
        })
    }

    /// Execute the actual ACP request: spawn client, run task, close client.
    ///
    /// Reports separate errors for execution failure and close failure.
    async fn execute_request(&self, request: ACPExecuteArgs) -> ToolResult<String> {
        let ACPExecuteArgs {
            agent_id,
            task,
            cwd,
        } = request;
        let agent_config = self.resolve_agent_config(&agent_id)?;

        let (command, session_cwd) = crate::acp::build_acp_command(
            &agent_config.command,
            &agent_config.args,
            cwd,
            &agent_config.env,
        )
        .map_err(|err| ToolError::execution(self.name(), err))?;

        let mut client = ACPClient::spawn(agent_id, command, session_cwd)
            .await
            .map_err(|err| ToolError::execution(self.name(), err))?;

        let execution_result = client.execute(&task).await;
        let close_result = client.close().await;

        // Combine execution and close errors for better diagnostics.
        match (execution_result, close_result) {
            (Ok(output), Ok(())) => Ok(output),
            (Ok(_), Err(close_err)) => Err(ToolError::execution(
                self.name(),
                anyhow::anyhow!(
                    "ACP execution finished but failed to close process: {}",
                    close_err
                ),
            )),
            (Err(exec_err), Ok(())) => Err(ToolError::execution(self.name(), exec_err)),
            (Err(exec_err), Err(close_err)) => Err(ToolError::execution(
                self.name(),
                anyhow::anyhow!(
                    "ACP execution failed: {}; additionally failed to close process: {}",
                    exec_err,
                    close_err
                ),
            )),
        }
    }
}

#[async_trait]
impl Tool for ACPTool {
    /// Returns `"acp_execute"`.
    fn name(&self) -> &str {
        ACP_EXECUTE_TOOL_NAME
    }

    /// Returns a lazily-initialised `ToolDefinition` describing the parameters.
    ///
    /// The definition is built once and cached in a `OnceLock` because
    /// `Tool::definition()` may be called frequently.
    fn definition(&self) -> Arc<ToolDefinition> {
        static DEF: OnceLock<Arc<ToolDefinition>> = OnceLock::new();
        DEF.get_or_init(|| {
            Arc::new(tool_definition_from_json(json!({
                "type": "function",
                "function": {
                    "name": ACP_EXECUTE_TOOL_NAME,
                    "description": ACP_EXECUTE_DESCRIPTION,
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "agent_id": {
                                "type": "string",
                                "description": ACP_AGENT_ID_DESC
                            },
                            "task": {
                                "type": "string",
                                "description": ACP_TASK_DESC
                            },
                            "cwd": {
                                "type": "string",
                                "description": ACP_CWD_DESC
                            }
                        },
                        "required": ["agent_id", "task"]
                    }
                }
            })))
        })
        .clone()
    }

    /// Execute the ACP agent with the given arguments.
    ///
    /// # Arguments
    ///
    /// * `args_json` — JSON string matching `ACPExecuteArgs` schema.
    /// * `_context` — Tool execution context (unused by this tool).
    async fn execute(&self, args_json: &str, _context: &ToolContext) -> ToolResult<String> {
        let request = self.parse_execute_args(args_json)?;
        self.execute_request(request).await
    }
}
