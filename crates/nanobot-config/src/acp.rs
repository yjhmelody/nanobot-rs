//! Configuration for the Agent Client Protocol (ACP) integration.
//!
//! ACP is a protocol that allows nanobot to delegate tasks to external
//! agent binaries (e.g., Claude Agent ACP, Codex ACP, GitHub Copilot CLI).
//! This module provides the [`ACPConfig`] and [`AgentConfig`] types that
//! control how these external agents are launched and managed.
//!
//! # Design
//!
//! - The default configuration pre-populates agents for three well-known ACP
//!   implementations: Claude Agent ACP (Zed Industries), Codex ACP, and
//!   GitHub Copilot CLI.
//! - Users can override the default agent and add custom agents via the
//!   config file. See [`ACPConfig::default`] for the built-in defaults.
//! - The `allowed_agents` field acts as an allowlist — agents not in this
//!   list cannot be invoked via ACP, even if they have a config entry.
//!
//! # Relationships
//!
//! - Referenced from [`crate::schema::Config`] as `Config.acp`, an optional
//!   field (`Option<ACPConfig>`).
//! - Used at runtime by `nanobot-agent`'s ACP client infrastructure to spawn
//!   subprocesses and communicate via the ACP protocol.
//!
//! # References
//!
//! - [Claude Agent ACP](https://github.com/zed-industries/claude-agent-acp)
//! - [Codex ACP](https://github.com/zed-industries/codex-acp)

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Configuration for the Agent Client Protocol (ACP) integration.
///
/// ACP allows nanobot to delegate tasks to external agent subprocesses.
/// This struct controls which agents are available, which one is used by
/// default, and how each agent is launched.
///
/// # Example
///
/// ```json
/// {
///   "enabled": true,
///   "defaultAgent": "claude",
///   "allowedAgents": ["claude", "codex"],
///   "agents": {
///     "claude": {
///       "command": "claude-agent-acp",
///       "args": [],
///       "env": {}
///     }
///   }
/// }
/// ```
///
/// # Invariants
///
/// - Every name in `allowed_agents` should have a corresponding entry in
///   `agents`. The default [`ACPConfig::default`] upholds this invariant.
/// - `default_agent` should be in `allowed_agents`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ACPConfig {
    /// Whether ACP integration is enabled.
    ///
    /// When `false`, external agent delegation via ACP is disallowed
    /// regardless of the configured agents.
    pub enabled: bool,
    /// Name of the default agent to use when none is specified.
    ///
    /// Must correspond to a key in `agents` and be present in `allowed_agents`.
    pub default_agent: String,
    /// Agents that are permitted to be invoked via ACP.
    ///
    /// Acts as an allowlist — only agent names in this vector can be
    /// delegated to, even if `agents` contains additional entries.
    pub allowed_agents: Vec<String>,
    /// Per-agent command and environment configuration, keyed by agent name.
    ///
    /// Each value describes how to launch a specific ACP agent subprocess.
    /// The key must match an entry in `allowed_agents` to be usable.
    pub agents: HashMap<String, AgentConfig>,
}

/// Command-line configuration for a single ACP agent.
///
/// Describes how to spawn an external agent subprocess: the executable
/// binary, its arguments, and environment variables.
///
/// # Example
///
/// ```json
/// {
///   "command": "claude-agent-acp",
///   "args": [],
///   "env": { "ANTHROPIC_API_KEY": "sk-ant-..." }
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Executable command used to launch the agent process.
    ///
    /// Should be resolvable via `PATH` or be an absolute path. This is the
    /// first argument passed to [`std::process::Command::new`].
    pub command: String,
    /// Additional arguments passed to the agent command.
    ///
    /// These follow the command in the subprocess invocation.
    /// Defaults to an empty vector.
    #[serde(default)]
    pub args: Vec<String>,
    /// Extra environment variables injected into the agent process.
    ///
    /// These are merged into the parent process's environment when spawning
    /// the ACP agent. Keys are variable names, values are their contents.
    /// Defaults to an empty map.
    #[serde(default)]
    pub env: HashMap<String, String>,
}

/// Provides sensible defaults for ACP integration.
///
/// The default configuration:
/// - Enables ACP (`enabled: true`).
/// - Pre-populates three well-known ACP agents (Claude, Codex, Copilot).
/// - Sets "claude" as the default agent.
///
/// # Pre-populated Agents
///
/// | Key      | Command            | Args      | Source                                  |
/// |----------|--------------------|-----------|-----------------------------------------|
/// | claude   | `claude-agent-acp` | (none)    | Zed Industries                          |
/// | codex    | `codex-acp`        | (none)    | Zed Industries                          |
/// | copilot  | `copilot`          | `--acp`   | GitHub Copilot CLI                      |
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

    // Integration tests that require binaries from the nanobot binary crate — skipped here.
    #[test]
    #[ignore = "requires local ACP binaries and valid auth"]
    fn smoke_test_claude_agent_acp() {
        // Full test lives in the nanobot binary crate where ACPClient is available.
    }

    #[test]
    #[ignore = "requires local ACP binaries and valid auth"]
    fn smoke_test_codex_acp() {
        // Full test lives in the nanobot binary crate where ACPClient is available.
    }
}
