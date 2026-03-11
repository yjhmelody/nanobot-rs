use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};

use crate::agent::{AgentLoop, ContextBuilder, SubagentManager};
use crate::bus::MessageBus;
use crate::config::schema::{ExecToolConfig, MCPServerConfig, WebToolsConfig};
use crate::cron::CronService;
use crate::provider::LLMProvider;
use crate::session::{ConsolidationConfig, JsonlSessionStore};
use crate::tools::acp::ACPTool;
use crate::tools::mcp::MCPManager;
use crate::tools::{ToolRegistry, ToolRegistryBuilder};

/// Configuration for AgentLoop that groups related parameters.
#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub model: String,
    pub max_iterations: usize,
    pub temperature: f32,
    pub max_tokens: i32,
    pub memory_window: usize,
    pub reasoning_effort: Option<String>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            model: "anthropic/claude-opus-4-5".to_string(),
            max_iterations: 40,
            temperature: 0.1,
            max_tokens: 8192,
            memory_window: 100,
            reasoning_effort: None,
        }
    }
}

/// Builder for constructing AgentLoop with a fluent API.
///
/// This builder pattern solves the problem of AgentLoop::new() having too many parameters
/// by grouping related configuration and making optional dependencies explicit.
///
/// # Example
///
/// ```no_run
/// use std::sync::Arc;
/// use nanobot_rs::agent::{AgentLoopBuilder, AgentConfig};
/// use nanobot_rs::bus::MessageBus;
/// use nanobot_rs::provider::LLMProvider;
/// use std::path::PathBuf;
///
/// # async fn example(provider: Arc<dyn LLMProvider>) -> anyhow::Result<()> {
/// let bus = MessageBus::new();
/// let workspace = PathBuf::from("/workspace");
///
/// let agent = AgentLoopBuilder::new(bus, provider, workspace)
///     .with_config(AgentConfig {
///         model: "anthropic/claude-opus-4-5".to_string(),
///         max_iterations: 40,
///         ..Default::default()
///     })
///     .with_restrict_to_workspace(true)
///     .build()?;
/// # Ok(())
/// # }
/// ```
pub struct AgentLoopBuilder {
    // Required parameters
    bus: MessageBus,
    provider: Arc<dyn LLMProvider>,
    workspace: PathBuf,

    // Configuration
    config: AgentConfig,
    consolidation_config: ConsolidationConfig,
    web_config: WebToolsConfig,
    exec_config: ExecToolConfig,
    mcp_servers: HashMap<String, MCPServerConfig>,
    acp_config: Option<crate::acp::config::ACPConfig>,
    restrict_to_workspace: bool,

    // Optional dependencies
    cron_service: Option<Arc<CronService>>,
}

impl AgentLoopBuilder {
    /// Creates a new builder with required parameters.
    ///
    /// # Arguments
    ///
    /// * `bus` - Message bus for inter-component communication
    /// * `provider` - LLM provider for chat completions
    /// * `workspace` - Working directory for file operations
    pub fn new(bus: MessageBus, provider: Arc<dyn LLMProvider>, workspace: PathBuf) -> Self {
        Self {
            bus,
            provider,
            workspace,
            config: AgentConfig::default(),
            consolidation_config: ConsolidationConfig::default(),
            web_config: WebToolsConfig::default(),
            exec_config: ExecToolConfig::default(),
            mcp_servers: HashMap::new(),
            acp_config: None,
            restrict_to_workspace: false,
            cron_service: None,
        }
    }

    /// Sets the agent configuration (model, iterations, temperature, etc.).
    pub fn with_config(mut self, config: AgentConfig) -> Self {
        self.config = config;
        self
    }

    /// Sets the session consolidation configuration.
    pub fn with_consolidation_config(mut self, config: ConsolidationConfig) -> Self {
        self.consolidation_config = config;
        self
    }

    /// Sets the web tools configuration (proxy, search API key).
    pub fn with_web_config(mut self, config: WebToolsConfig) -> Self {
        self.web_config = config;
        self
    }

    /// Sets the exec tool configuration (timeout, PATH append).
    pub fn with_exec_config(mut self, config: ExecToolConfig) -> Self {
        self.exec_config = config;
        self
    }

    /// Sets the MCP servers configuration.
    pub fn with_mcp_servers(mut self, servers: HashMap<String, MCPServerConfig>) -> Self {
        self.mcp_servers = servers;
        self
    }

    /// Sets the ACP configuration.
    pub fn with_acp_config(mut self, config: Option<crate::acp::config::ACPConfig>) -> Self {
        self.acp_config = config;
        self
    }

    /// Restricts file operations to the workspace directory.
    pub fn with_restrict_to_workspace(mut self, restrict: bool) -> Self {
        self.restrict_to_workspace = restrict;
        self
    }

    /// Sets the cron service for scheduled tasks.
    pub fn with_cron_service(mut self, service: Arc<CronService>) -> Self {
        self.cron_service = Some(service);
        self
    }

    /// Builds the AgentLoop instance.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Context builder initialization fails
    /// - Session manager initialization fails
    pub async fn build(self) -> Result<AgentLoop> {
        let context = ContextBuilder::new(self.workspace.clone())?;
        let sessions = Arc::new(JsonlSessionStore::new(&self.workspace).await?);
        let tools = self.build_tool_registry()?;

        // Create SubagentManager with ToolRegistry
        let subagent_manager = Arc::new(SubagentManager::new(
            self.provider.clone(),
            self.workspace.clone(),
            self.bus.clone(),
            tools.clone(),
            self.config.model.clone(),
            self.config.temperature,
            self.config.max_tokens,
            self.config.reasoning_effort.clone(),
        ));

        // Set the spawn service in ToolRegistry (SubagentManager implements SpawnService)
        tools.set_spawn_service(subagent_manager);

        let mcp = if self.mcp_servers.is_empty() {
            None
        } else {
            Some(Arc::new(MCPManager::new(self.mcp_servers)))
        };

        Ok(AgentLoop {
            bus: self.bus,
            provider: self.provider,

            model: self.config.model,
            max_iterations: self.config.max_iterations,
            temperature: self.config.temperature,
            max_tokens: self.config.max_tokens,
            memory_window: self.config.memory_window,
            reasoning_effort: self.config.reasoning_effort,
            consolidation_config: self.consolidation_config,
            tools,
            mcp,
            context,
            sessions,
            running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            session_locks: Arc::new(dashmap::DashMap::new()),
            active_tasks: Arc::new(dashmap::DashMap::new()),
            last_cleanup: Arc::new(parking_lot::Mutex::new(std::time::Instant::now())),
        })
    }

    fn build_tool_registry(&self) -> Result<Arc<ToolRegistry>> {
        let mut builder = ToolRegistryBuilder::new(self.workspace.clone())
            .with_restrict_to_workspace(self.restrict_to_workspace)
            .with_exec_config(self.exec_config.clone())
            .with_web_config(self.web_config.clone())
            .with_bus(self.bus.clone());

        if let Some(cron_service) = self.cron_service.clone() {
            builder = builder.with_cron_service(cron_service);
        }

        if let Some(acp_config) = self.acp_config.clone().filter(|cfg| cfg.enabled) {
            builder = builder.with_custom_tool(Arc::new(ACPTool::new(acp_config)));
        }

        Ok(Arc::new(
            builder.build().context("Failed to build tool registry")?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{ChatRequest, LLMResponse, UsageStats};
    use async_trait::async_trait;

    #[derive(Debug)]
    struct DummyProvider;

    #[async_trait]
    impl LLMProvider for DummyProvider {
        async fn chat(&self, _req: ChatRequest) -> LLMResponse {
            LLMResponse {
                content: Some("test".to_string()),
                tool_calls: Vec::new(),
                finish_reason: "stop".to_string(),
                usage: UsageStats::default(),
                reasoning_content: None,
                thinking_blocks: None,
            }
        }

        fn default_model(&self) -> &str {
            "test/model"
        }
    }

    #[tokio::test]
    async fn builder_creates_agent_loop_with_defaults() {
        let bus = MessageBus::new();
        let provider: Arc<dyn LLMProvider> = Arc::new(DummyProvider);
        let workspace = std::env::temp_dir().join(format!("nanobot-test-{}", uuid::Uuid::new_v4()));

        let agent = AgentLoopBuilder::new(bus, provider, workspace)
            .build()
            .await
            .expect("build agent loop");

        assert_eq!(agent.max_iterations, 40);
        assert_eq!(agent.temperature, 0.1);
    }

    #[tokio::test]
    async fn builder_accepts_custom_config() {
        let bus = MessageBus::new();
        let provider: Arc<dyn LLMProvider> = Arc::new(DummyProvider);
        let workspace = std::env::temp_dir().join(format!("nanobot-test-{}", uuid::Uuid::new_v4()));

        let custom_config = AgentConfig {
            model: "custom/model".to_string(),
            max_iterations: 20,
            temperature: 0.5,
            max_tokens: 4096,
            memory_window: 50,
            reasoning_effort: Some("high".to_string()),
        };

        let agent = AgentLoopBuilder::new(bus, provider, workspace)
            .with_config(custom_config)
            .with_restrict_to_workspace(true)
            .build()
            .await
            .expect("build agent loop");

        assert_eq!(agent.model, "custom/model");
        assert_eq!(agent.max_iterations, 20);
        assert_eq!(agent.temperature, 0.5);
        assert_eq!(agent.max_tokens, 4096);
        assert_eq!(agent.memory_window, 50);
        assert_eq!(agent.reasoning_effort.as_deref(), Some("high"));
    }

    #[tokio::test]
    async fn builder_registers_spawn_tool_via_unified_tool_builder() {
        let bus = MessageBus::new();
        let provider: Arc<dyn LLMProvider> = Arc::new(DummyProvider);
        let workspace = std::env::temp_dir().join(format!("nanobot-test-{}", uuid::Uuid::new_v4()));

        let agent = AgentLoopBuilder::new(bus, provider, workspace)
            .build()
            .await
            .expect("build agent loop");

        let names: Vec<_> = agent
            .tools
            .definitions()
            .into_iter()
            .map(|d| d.function.name)
            .collect();
        assert!(names.contains(&"spawn".to_string()));
    }
}
