//! ACP configuration

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ACPConfig {
    pub enabled: bool,
    pub default_agent: String,
    pub allowed_agents: Vec<String>,
    pub agents: HashMap<String, AgentConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub command: String,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

impl Default for ACPConfig {
    fn default() -> Self {
        let mut agents = HashMap::new();
        
        // Codex (OpenAI) - 代码生成专家
        agents.insert("codex".to_string(), AgentConfig {
            command: "codex".to_string(),
            env: HashMap::new(),
        });
        
        // Claude Code (Anthropic) - 长上下文推理
        agents.insert("claude".to_string(), AgentConfig {
            command: "claude".to_string(),
            env: HashMap::new(),
        });
        
        // Cursor - IDE 集成，快速迭代
        agents.insert("cursor".to_string(), AgentConfig {
            command: "cursor".to_string(),
            env: HashMap::new(),
        });
        
        // Windsurf (Codeium) - 多文件编辑
        agents.insert("windsurf".to_string(), AgentConfig {
            command: "windsurf".to_string(),
            env: HashMap::new(),
        });
        
        // Cline - 开源可定制
        agents.insert("cline".to_string(), AgentConfig {
            command: "cline".to_string(),
            env: HashMap::new(),
        });
        
        Self {
            enabled: true,
            default_agent: "claude".to_string(), // Claude 作为默认（长上下文 + 强推理）
            allowed_agents: vec![
                "codex".to_string(),
                "claude".to_string(),
                "cursor".to_string(),
                "windsurf".to_string(),
                "cline".to_string(),
            ],
            agents,
        }
    }
}
