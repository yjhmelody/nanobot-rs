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
    pub args: Vec<String>,
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
                args: vec![],
                env: HashMap::new(),
            },
        );

        // Codex ACP - Zed Industries' Codex implementation
        // https://github.com/zed-industries/codex-acp
        agents.insert(
            "codex".to_string(),
            AgentConfig {
                command: "codex-acp".to_string(),
                args: vec![],
                env: HashMap::new(),
            },
        );

        // GitHub Copilot CLI
        // https://github.com/github/copilot-cli
        agents.insert(
            "copilot".to_string(),
            AgentConfig {
                command: "copilot".to_string(),
                args: vec!["--acp".to_string()],
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_correct_structure() {
        let config = ACPConfig::default();

        assert!(config.enabled);
        assert_eq!(config.default_agent, "claude");
        assert_eq!(config.allowed_agents.len(), 3);
        assert_eq!(config.agents.len(), 3);
    }

    #[test]
    fn default_config_includes_all_acp_agents() {
        let config = ACPConfig::default();

        assert!(config.agents.contains_key("claude"));
        assert!(config.agents.contains_key("codex"));
        assert!(config.agents.contains_key("copilot"));
    }

    #[test]
    fn default_config_has_correct_commands() {
        let config = ACPConfig::default();

        assert_eq!(
            config.agents.get("claude").unwrap().command,
            "claude-agent-acp"
        );
        assert_eq!(config.agents.get("codex").unwrap().command, "codex-acp");
        assert_eq!(config.agents.get("copilot").unwrap().command, "copilot");
    }

    #[test]
    fn copilot_has_acp_argument() {
        let config = ACPConfig::default();
        let copilot = config.agents.get("copilot").unwrap();

        assert_eq!(copilot.args, vec!["--acp"]);
    }

    #[test]
    fn claude_and_codex_have_no_arguments() {
        let config = ACPConfig::default();

        assert!(config.agents.get("claude").unwrap().args.is_empty());
        assert!(config.agents.get("codex").unwrap().args.is_empty());
    }

    #[test]
    fn default_agent_is_in_allowed_list() {
        let config = ACPConfig::default();

        assert!(config.allowed_agents.contains(&config.default_agent));
    }

    #[test]
    fn all_allowed_agents_have_configs() {
        let config = ACPConfig::default();

        for agent in &config.allowed_agents {
            assert!(
                config.agents.contains_key(agent),
                "allowed agent '{}' should have a config",
                agent
            );
        }
    }

    #[test]
    fn agent_config_serialization() {
        let mut env = HashMap::new();
        env.insert("API_KEY".to_string(), "test".to_string());

        let agent = AgentConfig {
            command: "test-command".to_string(),
            args: vec!["--flag".to_string(), "value".to_string()],
            env,
        };

        let json = serde_json::to_string(&agent).unwrap();
        let deserialized: AgentConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.command, "test-command");
        assert_eq!(deserialized.args, vec!["--flag", "value"]);
        assert_eq!(deserialized.env.get("API_KEY").unwrap(), "test");
    }

    #[test]
    fn acp_config_serialization() {
        let config = ACPConfig::default();

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: ACPConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.enabled, config.enabled);
        assert_eq!(deserialized.default_agent, config.default_agent);
        assert_eq!(deserialized.allowed_agents, config.allowed_agents);
        assert_eq!(deserialized.agents.len(), config.agents.len());
    }

    // Optional: Test if ACP binaries are available (non-blocking)
    #[test]
    #[ignore = "requires local ACP binaries installed"]
    fn check_claude_agent_acp_available() {
        let output = std::process::Command::new("claude-agent-acp")
            .arg("--help")
            .output();
        output.expect("claude-agent-acp not available");
    }

    #[test]
    #[ignore = "requires local ACP binaries installed"]
    fn check_codex_acp_available() {
        let output = std::process::Command::new("codex-acp")
            .arg("--help")
            .output();
        output.expect("codex-acp not available");
    }

    #[test]
    #[ignore = "requires local ACP binaries installed"]
    fn check_copilot_cli_available() {
        let output = std::process::Command::new("copilot").arg("--help").output();
        output.expect("copilot not available");
    }

    // Integration test that can be run when binaries are available
    #[tokio::test]
    #[ignore = "requires local ACP binaries and valid auth"]
    async fn smoke_test_claude_agent_acp() {
        use crate::acp::{ACPClient, build_acp_command};

        let cwd = std::env::current_dir().expect("current dir");
        let (command, session_cwd) = build_acp_command(
            "claude-agent-acp",
            &[],
            Some(cwd),
            &HashMap::new(),
        )
        .expect("build command");

        let mut client = ACPClient::spawn(
            "claude".to_string(),
            command,
            session_cwd,
        )
        .await
        .expect("spawn claude-agent-acp");

        let output = client
            .execute("Reply with 'ACP OK' if you can read this.")
            .await
            .expect("execute prompt");

        assert!(!output.trim().is_empty());
        client.close().await.expect("close client");
    }

    #[tokio::test]
    #[ignore = "requires local ACP binaries and valid auth"]
    async fn smoke_test_codex_acp() {
        use crate::acp::{ACPClient, build_acp_command};

        let cwd = std::env::current_dir().expect("current dir");
        let (command, session_cwd) = build_acp_command(
            "codex-acp",
            &[],
            Some(cwd),
            &HashMap::new(),
        )
        .expect("build command");

        let mut client = ACPClient::spawn(
            "codex".to_string(),
            command,
            session_cwd,
        )
        .await
        .expect("spawn codex-acp");

        let output = client
            .execute("Reply with 'ACP OK' if you can read this.")
            .await
            .expect("execute prompt");

        assert!(!output.trim().is_empty());
        client.close().await.expect("close client");
    }
}
