//! ACP tool for delegating coding tasks to ACP agents.

use async_trait::async_trait;
use serde_json::json;

use crate::acp::client::ACPClient;
use crate::acp::config::{ACPConfig, AgentConfig};
use crate::error::{NanobotError, Result};
use crate::tools::base::{
    Tool, ToolContext, ToolDefinition, parse_args, tool_definition_from_json,
};
use crate::types::tools::ACPExecuteArgs;

const ACP_EXECUTE_TOOL_NAME: &str = "acp_execute";
const ACP_EXECUTE_DESCRIPTION: &str = "Execute a coding task using an ACP agent. \
Use this for complex coding tasks that require multi-file edits, refactoring, or \
end-to-end feature implementation.";

pub struct ACPTool {
    config: ACPConfig,
}

impl ACPTool {
    pub fn new(config: ACPConfig) -> Self {
        Self { config }
    }

    fn parse_execute_args(&self, args_json: &str) -> Result<ACPExecuteArgs> {
        parse_args::<ACPExecuteArgs>(args_json).map_err(|err| match err {
            NanobotError::InvalidToolArgs { message, .. } => {
                NanobotError::invalid_tool_args(self.name(), message)
            }
            other => other,
        })
    }

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

    fn configured_agents(&self) -> Vec<String> {
        let mut configured = self.config.agents.keys().cloned().collect::<Vec<_>>();
        configured.sort_unstable();
        configured
    }

    fn definition_agent_schema(&self) -> serde_json::Value {
        let allowed_agents = self.allowed_agents();
        let mut schema = json!({
            "type": "string",
            "description": "ACP agent id used to execute the task"
        });
        if !allowed_agents.is_empty() {
            schema["enum"] = json!(allowed_agents);
        }
        schema
    }

    fn definition_description(&self) -> String {
        let allowed_agents = self.allowed_agents();
        if allowed_agents.is_empty() {
            ACP_EXECUTE_DESCRIPTION.to_string()
        } else {
            format!(
                "{} Allowed agents: {}.",
                ACP_EXECUTE_DESCRIPTION,
                allowed_agents.join(", ")
            )
        }
    }

    fn resolve_agent_config(&self, agent_id: &str) -> Result<&AgentConfig> {
        let allowed_agents = self.allowed_agents();
        if allowed_agents.is_empty() {
            return Err(NanobotError::invalid_tool_args(
                self.name(),
                "No ACP agents are allowed. Configure `acp.allowed_agents` first.",
            ));
        }
        if !allowed_agents.iter().any(|allowed| allowed == agent_id) {
            return Err(NanobotError::invalid_tool_args(
                self.name(),
                format!(
                    "Agent '{}' is not allowed. Allowed agents: {}",
                    agent_id,
                    allowed_agents.join(", ")
                ),
            ));
        }

        let agent_config = self.config.agents.get(agent_id).ok_or_else(|| {
            let configured = self.configured_agents();
            let configured_text = if configured.is_empty() {
                "none".to_string()
            } else {
                configured.join(", ")
            };
            NanobotError::invalid_tool_args(
                self.name(),
                format!(
                    "Agent '{}' is not configured. Configured agents: {}",
                    agent_id, configured_text
                ),
            )
        })?;

        if agent_config.command.trim().is_empty() {
            return Err(NanobotError::invalid_tool_args(
                self.name(),
                format!("Agent '{}' is configured with an empty command", agent_id),
            ));
        }

        Ok(agent_config)
    }

    async fn execute_request(&self, request: ACPExecuteArgs) -> Result<String> {
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
        .map_err(|err| NanobotError::tool_execution(self.name(), err))?;

        let mut client = ACPClient::spawn(agent_id, command, session_cwd)
            .await
            .map_err(|err| NanobotError::tool_execution(self.name(), err))?;

        let execution_result = client.execute(&task).await;
        let close_result = client.close().await;

        match (execution_result, close_result) {
            (Ok(output), Ok(())) => Ok(output),
            (Ok(_), Err(close_err)) => Err(NanobotError::tool_execution(
                self.name(),
                anyhow::anyhow!(
                    "ACP execution finished but failed to close process: {}",
                    close_err
                ),
            )),
            (Err(exec_err), Ok(())) => Err(NanobotError::tool_execution(self.name(), exec_err)),
            (Err(exec_err), Err(close_err)) => Err(NanobotError::tool_execution(
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
    fn name(&self) -> &str {
        ACP_EXECUTE_TOOL_NAME
    }

    fn definition(&self) -> ToolDefinition {
        tool_definition_from_json(json!({
            "type": "function",
            "function": {
                "name": self.name(),
                "description": self.definition_description(),
                "parameters": {
                    "type": "object",
                    "properties": {
                        "agent_id": self.definition_agent_schema(),
                        "task": {
                            "type": "string",
                            "description": "Coding task to execute by the ACP agent"
                        },
                        "cwd": {
                            "type": "string",
                            "description": "Optional working directory for the ACP agent process"
                        }
                    },
                    "required": ["agent_id", "task"]
                }
            }
        }))
    }

    async fn execute(&self, args_json: &str, _context: &ToolContext) -> Result<String> {
        let request = self.parse_execute_args(args_json)?;
        self.execute_request(request).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SessionKey;

    #[test]
    fn acp_tool_metadata_has_required_fields() {
        let tool = ACPTool::new(ACPConfig::default());

        assert_eq!(tool.name(), "acp_execute");

        let definition = tool.definition();
        assert_eq!(definition.function.name, "acp_execute");
        assert!(definition.function.description.contains("ACP agent"));
        assert!(
            definition
                .function
                .parameters
                .required
                .contains(&"agent_id".to_string())
        );
        assert!(
            definition
                .function
                .parameters
                .required
                .contains(&"task".to_string())
        );
    }

    #[test]
    fn definition_uses_allowed_agents_from_config() {
        let mut config = ACPConfig::default();
        config.allowed_agents = vec![
            "codex".to_string(),
            "claude".to_string(),
            "codex".to_string(),
            "  ".to_string(),
        ];
        let tool = ACPTool::new(config);

        let definition = tool.definition();
        let agent_schema = definition
            .function
            .parameters
            .properties
            .get("agent_id")
            .expect("agent_id schema");
        assert_eq!(
            agent_schema.enum_values.as_ref(),
            Some(&vec!["claude".to_string(), "codex".to_string()])
        );
    }

    #[test]
    fn resolve_agent_config_rejects_disallowed_agent() {
        let tool = ACPTool::new(ACPConfig::default());
        let err = tool
            .resolve_agent_config("unknown-agent")
            .expect_err("unknown agent should be rejected");
        assert!(err.to_string().contains("not allowed"));
    }

    #[test]
    fn resolve_agent_config_rejects_when_allowed_agents_empty() {
        let mut config = ACPConfig::default();
        config.allowed_agents.clear();
        let tool = ACPTool::new(config);

        let err = tool
            .resolve_agent_config("codex")
            .expect_err("empty allowed agents should fail");
        assert!(err.to_string().contains("No ACP agents are allowed"));
    }

    #[tokio::test]
    async fn execute_returns_tool_scoped_parse_error() {
        let tool = ACPTool::new(ACPConfig::default());
        let err = tool
            .execute(
                r#"{"task":"missing required field agent_id"}"#,
                &ToolContext {
                    channel: "test".to_string(),
                    chat_id: "test".to_string(),
                    session_key: SessionKey::from("test:test"),
                    message_id: None,
                },
            )
            .await
            .expect_err("missing agent_id should fail");
        assert!(err.to_string().contains("acp_execute"));
    }
}
