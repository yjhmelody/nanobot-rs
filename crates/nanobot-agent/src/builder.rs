use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;

use crate::context::ContextBuilder;
use crate::error::AgentResult;
use crate::loop_core::AgentLoop;
use crate::subagent::SubagentManager;
use crate::traits::ContextProvider;
use nanobot_bus::MessageBus;
use nanobot_config::schema::{ChannelsConfig, ExecToolConfig, MCPServerConfig, WebToolsConfig};
use nanobot_cron::CronService;
use nanobot_provider::LLMProvider;
use nanobot_session::{
    ConsolidationConfig, FileMemoryProvider, JsonlSessionStore, LlmConsolidationStrategy,
    SessionManager,
};
use nanobot_tools::mcp::MCPManager;
use nanobot_tools::{ToolRegistry, ToolRegistryBuilder};

/// Configuration for AgentLoop.
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
            model: "anthropic/claude-opus-4-6".to_string(),
            max_iterations: 40,
            temperature: 0.1,
            max_tokens: 8192,
            memory_window: 100,
            reasoning_effort: None,
        }
    }
}

/// Builder for constructing AgentLoop with a fluent API.
pub struct AgentLoopBuilder {
    bus: MessageBus,
    provider: Arc<dyn LLMProvider>,
    workspace: PathBuf,
    config: AgentConfig,
    consolidation_config: ConsolidationConfig,
    auto_consolidation: bool,
    web_config: WebToolsConfig,
    exec_config: ExecToolConfig,
    mcp_servers: HashMap<String, MCPServerConfig>,
    restrict_to_workspace: bool,
    channel_configs: ChannelsConfig,
    send_usage_summary: bool,
    cron_service: Option<Arc<CronService>>,
    custom_tools: Vec<Arc<dyn nanobot_tools::Tool>>,
}

impl AgentLoopBuilder {
    /// Creates a new builder with the required bus, provider, and workspace path.
    pub fn new(bus: MessageBus, provider: Arc<dyn LLMProvider>, workspace: PathBuf) -> Self {
        Self {
            bus,
            provider,
            workspace,
            config: AgentConfig::default(),
            consolidation_config: ConsolidationConfig::default(),
            auto_consolidation: true,
            web_config: WebToolsConfig::default(),
            exec_config: ExecToolConfig::default(),
            mcp_servers: HashMap::new(),
            restrict_to_workspace: false,
            channel_configs: ChannelsConfig::default(),
            send_usage_summary: false,
            cron_service: None,
            custom_tools: Vec::new(),
        }
    }

    /// Sets the agent model configuration (model name, iterations, temperature, etc.).
    pub fn with_config(mut self, config: AgentConfig) -> Self {
        self.config = config;
        self
    }

    /// Sets the session consolidation configuration.
    pub fn with_consolidation_config(mut self, config: ConsolidationConfig) -> Self {
        self.consolidation_config = config;
        self
    }

    /// Enables or disables automatic session consolidation on each save.
    pub fn with_auto_consolidation(mut self, enabled: bool) -> Self {
        self.auto_consolidation = enabled;
        self
    }

    /// Configures the web search/fetch tool settings.
    pub fn with_web_config(mut self, config: WebToolsConfig) -> Self {
        self.web_config = config;
        self
    }

    /// Configures the shell execution tool settings.
    pub fn with_exec_config(mut self, config: ExecToolConfig) -> Self {
        self.exec_config = config;
        self
    }

    /// Registers MCP server configurations to be connected at startup.
    pub fn with_mcp_servers(mut self, servers: HashMap<String, MCPServerConfig>) -> Self {
        self.mcp_servers = servers;
        self
    }

    /// When `true`, filesystem tools are restricted to the workspace directory.
    pub fn with_restrict_to_workspace(mut self, restrict: bool) -> Self {
        self.restrict_to_workspace = restrict;
        self
    }

    /// Provides channel configuration so per-channel agent overrides can be resolved at runtime.
    pub fn with_channel_configs(mut self, config: ChannelsConfig) -> Self {
        self.channel_configs = config;
        self
    }

    /// Provides a `CronService` to enable cron tool support.
    pub fn with_cron_service(mut self, service: Arc<CronService>) -> Self {
        self.cron_service = Some(service);
        self
    }

    /// When `true`, appends a token-usage summary to each outbound message.
    pub fn with_send_usage_summary(mut self, enabled: bool) -> Self {
        self.send_usage_summary = enabled;
        self
    }

    /// Registers an additional custom tool into the agent's tool registry.
    pub fn with_custom_tool(mut self, tool: Arc<dyn nanobot_tools::Tool>) -> Self {
        self.custom_tools.push(tool);
        self
    }

    /// Builds and returns a fully configured `AgentLoop`.
    pub async fn build(self) -> AgentResult<AgentLoop> {
        let context: Arc<dyn ContextProvider> =
            Arc::new(ContextBuilder::new(self.workspace.clone())?);
        let store = JsonlSessionStore::new(&self.workspace).await?;

        let mut session_manager =
            SessionManager::new(Box::new(store)).with_auto_consolidation(self.auto_consolidation);

        let consolidation_strategy = LlmConsolidationStrategy::new(
            self.provider.clone(),
            self.config.model.clone(),
            self.consolidation_config.clone(),
        );
        session_manager = session_manager.with_consolidation(Box::new(consolidation_strategy));

        let memory_provider = FileMemoryProvider::new(&self.workspace)?;
        session_manager = session_manager.add_memory_provider(Box::new(memory_provider));

        let sessions = Arc::new(session_manager);
        let tools = self.build_tool_registry()?;

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
            consolidation_enabled: self.auto_consolidation,
            channel_configs: self.channel_configs,
            send_usage_summary: self.send_usage_summary,
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

    fn build_tool_registry(&self) -> AgentResult<Arc<ToolRegistry>> {
        let mut builder = ToolRegistryBuilder::new(self.workspace.clone())
            .with_restrict_to_workspace(self.restrict_to_workspace)
            .with_exec_config(self.exec_config.clone())
            .with_web_config(self.web_config.clone())
            .with_bus(self.bus.clone());

        if let Some(cron_service) = self.cron_service.clone() {
            builder = builder.with_cron_service(cron_service);
        }

        for tool in &self.custom_tools {
            builder = builder.with_custom_tool(tool.clone());
        }

        Ok(Arc::new(
            builder.build().context("Failed to build tool registry")?,
        ))
    }
}
