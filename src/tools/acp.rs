//! ACP Tool - Execute coding tasks using ACP agents

use std::collections::BTreeMap;
use async_trait::async_trait;
use serde::Deserialize;

use crate::acp::client::ACPClient;
use crate::acp::config::ACPConfig;
use crate::error::{NanobotError, Result};
use crate::tools::base::{Tool, ToolContext, ToolDefinition, JsonSchema, JsonSchemaType};

#[derive(Debug, Deserialize)]
struct ACPExecuteRequest {
    agent_id: String,
    task: String,
    cwd: Option<String>,
}

pub struct ACPTool {
    config: ACPConfig,
}

impl ACPTool {
    pub fn new(config: ACPConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Tool for ACPTool {
    fn name(&self) -> &str {
        "acp_execute"
    }
    
    fn definition(&self) -> ToolDefinition {
        let mut properties = BTreeMap::new();
        
        properties.insert(
            "agent_id".to_string(),
            JsonSchema {
                schema_type: JsonSchemaType::String,
                description: Some("The ACP agent to use: codex (OpenAI), claude (Anthropic), cursor (IDE), windsurf (Codeium), cline (open-source)".to_string()),
                enum_values: Some(vec![
                    "codex".to_string(),
                    "claude".to_string(),
                    "cursor".to_string(),
                    "windsurf".to_string(),
                    "cline".to_string(),
                ]),
                properties: BTreeMap::new(),
                required: Vec::new(),
                items: None,
                minimum: None,
                maximum: None,
            },
        );
        
        properties.insert(
            "task".to_string(),
            JsonSchema {
                schema_type: JsonSchemaType::String,
                description: Some("The coding task to execute. Be specific and clear.".to_string()),
                properties: BTreeMap::new(),
                required: Vec::new(),
                enum_values: None,
                items: None,
                minimum: None,
                maximum: None,
            },
        );
        
        properties.insert(
            "cwd".to_string(),
            JsonSchema {
                schema_type: JsonSchemaType::String,
                description: Some("Working directory (optional)".to_string()),
                properties: BTreeMap::new(),
                required: Vec::new(),
                enum_values: None,
                items: None,
                minimum: None,
                maximum: None,
            },
        );
        
        ToolDefinition::function(
            self.name(),
            "Execute a coding task using an ACP agent (codex, claude, cursor, windsurf, cline). \
             Use this for complex coding tasks that require multiple file operations, \
             code generation, refactoring, or building complete features.",
            JsonSchema::object(properties, vec!["agent_id", "task"]),
        )
    }
    
    async fn execute(&self, args: &str, _context: &ToolContext) -> Result<String> {
        let req: ACPExecuteRequest = serde_json::from_str(args)
            .map_err(|e| NanobotError::invalid_tool_args(self.name(), format!("Failed to parse arguments: {}", e)))?;
        
        // 验证 agent_id
        if !self.config.allowed_agents.contains(&req.agent_id) {
            return Err(NanobotError::invalid_tool_args(
                self.name(),
                format!(
                    "Agent '{}' is not allowed. Allowed agents: {:?}",
                    req.agent_id,
                    self.config.allowed_agents
                )
            ));
        }
        
        // 获取 agent 配置
        let agent_config = self.config.agents.get(&req.agent_id)
            .ok_or_else(|| NanobotError::invalid_tool_args(
                self.name(),
                format!("Agent '{}' not configured", req.agent_id)
            ))?;
        
        // 创建 ACP Client
        let mut client = ACPClient::spawn(
            req.agent_id.clone(),
            agent_config.command.clone(),
            req.cwd.map(|s| s.into()),
            agent_config.env.clone(),
        ).await
        .map_err(|e| NanobotError::tool_execution(self.name(), e))?;
        
        // 执行任务
        let result = client.execute(&req.task).await
            .map_err(|e| NanobotError::tool_execution(self.name(), e))?;
        
        // 关闭 client
        client.close().await
            .map_err(|e| NanobotError::tool_execution(self.name(), e))?;
        
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_acp_tool_metadata() {
        let config = ACPConfig::default();
        let tool = ACPTool::new(config);
        
        assert_eq!(tool.name(), "acp_execute");
        
        let def = tool.definition();
        assert_eq!(def.function.name, "acp_execute");
        assert!(def.function.description.contains("ACP agent"));
        assert!(def.function.parameters.required.contains(&"agent_id".to_string()));
        assert!(def.function.parameters.required.contains(&"task".to_string()));
    }
}
