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

        // Claude Agent ACP - Zed Industries' Claude implementation
        // https://github.com/zed-industries/claude-agent-acp
        agents.insert(
            "claude".to_string(),
            AgentConfig {
                command: "claude-agent-acp".to_string(),
                env: HashMap::new(),
            },
        );

        // Codex ACP - Zed Industries' Codex implementation
        // https://github.com/zed-industries/codex-acp
        agents.insert(
            "codex".to_string(),
            AgentConfig {
                command: "codex-acp".to_string(),
                env: HashMap::new(),
            },
        );

        // GitHub Copilot CLI
        // https://github.com/github/copilot-cli
        agents.insert(
            "copilot".to_string(),
            AgentConfig {
                command: "github-copilot-cli".to_string(),
                env: HashMap::new(),
            },
        );

        Self {
            enabled: true,
            default_agent: "claude".to_string(),
            allowed_agents: vec![
                "claude".to_string(),
                "codex".to_string(),
                "copilot".to_string(),
            ],
            agents,
        }
    }
}
