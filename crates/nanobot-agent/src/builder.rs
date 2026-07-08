//! Builder for constructing a fully-wired [`AgentLoop`].
//!
//! This module provides [`AgentConfig`] (the configuration struct) and
//! [`AgentLoopBuilder`] (the builder API powered by `bon`). The builder
//! assembles all dependencies — context provider, session store, retrieval
//! service, tool registry, subagent manager, and MCP manager — into a
//! single [`AgentLoop`] ready to run.
//!
//! # Design Notes
//!
//! - Uses the `bon` crate for a derive-free builder pattern (`bon::bon`).
//! - Requires only three mandatory arguments (`bus`, `provider`, `workspace`);
//!   everything else has sensible defaults.
//! - Lazily initialises session persistence (JSONL), memory provider (file),
//!   and LLM consolidation strategy during construction.
//! - Wires up retrieval tools (`context_search`, `context_sources`,
//!   `context_explain`) as dynamic tools on the registry.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;

use crate::context::ContextBuilder;
use crate::error::AgentResult;
use crate::loop_core::AgentLoop;
use crate::retrieval::{
    ContextExplainTool, ContextSearchTool, ContextSourcesTool, RetrievalService,
};
use crate::subagent::SubagentManager;
use crate::traits::ContextProvider;
use nanobot_bus::MessageBus;
use nanobot_config::schema::{
    ChannelsConfig, ExecToolConfig, MCPServerConfig, RetrievalConfig, WebToolsConfig,
};
use nanobot_cron::CronService;
use nanobot_provider::LLMProvider;
use nanobot_provider::ReasoningConfig;
use nanobot_session::{
    ConsolidationConfig, FileMemoryProvider, JsonlSessionStore, LlmConsolidationStrategy,
    SessionManager,
};
use nanobot_tools::mcp::MCPManager;
use nanobot_tools::{Tool, ToolRegistry, ToolRegistryBuilder};

/// Configuration for [`AgentLoop`].
///
/// Controls the model identity, loop limits, token budgets, and subagent
/// cap.  All fields have sensible defaults via [`Default`]; use the builder
/// to override.
#[derive(Debug, Clone)]
pub struct AgentConfig {
    /// Model identifier (e.g. `"anthropic/claude-opus-4-6"`).
    pub model: String,
    /// Maximum ReAct iterations per turn (default: 40).
    pub max_iterations: usize,
    /// LLM temperature (default: 0.1).
    pub temperature: f32,
    /// Maximum output tokens per model call (default: 8192).
    pub max_tokens: i32,
    /// Number of historical messages to pass to the LLM (default: 100).
    pub memory_window: usize,
    /// Optional reasoning-effort configuration for extended thinking.
    pub reasoning_effort: Option<ReasoningConfig>,
    /// Maximum ReAct iterations for spawned subagents (default: 15).
    pub max_subagent_iterations: usize,
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
            max_subagent_iterations: 15,
        }
    }
}

/// Namespace for the `bon`-generated [`AgentLoop`] builder.
///
/// Use `AgentLoopBuilder::new(...)` to start building, then chain optional
/// setters, and call `.build().await` to produce the [`AgentLoop`].
///
/// # Required arguments
///
/// - `bus` — message bus for inbound/outbound communication
/// - `provider` — LLM provider implementation
/// - `workspace` — root workspace path for sessions, memory, and tools
pub struct AgentLoopBuilder;

#[bon::bon]
impl AgentLoopBuilder {
    /// Builds and returns a fully configured [`AgentLoop`].
    ///
    /// This async constructor wires up all internal dependencies:
    ///
    /// 1. **Context provider** — [`ContextBuilder`] for prompt assembly.
    /// 2. **Session store** — JSONL-backed persistence with optional
    ///    LLM-powered consolidation.
    /// 3. **Memory** — File-based memory provider (`MEMORY.md`/`HISTORY.md`).
    /// 4. **Retrieval service** — multi-source context retrieval.
    /// 5. **Tool registry** — filesystem, shell, web, MCP, cron, and
    ///    retrieval tools, plus any custom tools.
    /// 6. **Subagent manager** — spawn service for parallel agent tasks.
    /// 7. **MCP manager** — Model Context Protocol server connections.
    ///
    /// # Errors
    ///
    /// Returns [`AgentError`] if the context builder or tool registry
    /// construction fails.
    #[allow(clippy::new_ret_no_self)]
    #[builder(start_fn = new, finish_fn = build)]
    pub async fn create(
        #[builder(start_fn)] bus: MessageBus,
        #[builder(start_fn)] provider: Arc<dyn LLMProvider>,
        #[builder(start_fn)] workspace: PathBuf,
        #[builder(default)] config: AgentConfig,
        #[builder(default)] consolidation_config: ConsolidationConfig,
        #[builder(default = true)] auto_consolidation: bool,
        #[builder(default)] web_config: WebToolsConfig,
        #[builder(default)] exec_config: ExecToolConfig,
        #[builder(default)] mcp_servers: HashMap<String, MCPServerConfig>,
        #[builder(default)] restrict_to_workspace: bool,
        #[builder(default)] retrieval_config: RetrievalConfig,
        #[builder(default)] channel_configs: ChannelsConfig,
        #[builder(default)] send_usage_summary: bool,
        cron_service: Option<Arc<CronService>>,
        #[builder(default)] custom_tools: Vec<Arc<dyn Tool>>,
    ) -> AgentResult<AgentLoop> {
        let context: Arc<dyn ContextProvider> = Arc::new(ContextBuilder::new(workspace.clone())?);
        let store = JsonlSessionStore::new(&workspace).await?;

        let mut session_manager =
            SessionManager::new(Box::new(store)).with_auto_consolidation(auto_consolidation);

        let consolidation_strategy = LlmConsolidationStrategy::new(
            provider.clone(),
            config.model.clone(),
            consolidation_config.clone(),
        );
        session_manager = session_manager.with_consolidation(Box::new(consolidation_strategy));

        let memory_provider = FileMemoryProvider::new(&workspace)?;
        session_manager = session_manager.add_memory_provider(Box::new(memory_provider));

        let sessions = Arc::new(session_manager);
        let retrieval = Arc::new(RetrievalService::new(
            retrieval_config.clone(),
            workspace.clone(),
            restrict_to_workspace,
        ));
        let tools = build_tool_registry(
            workspace.clone(),
            restrict_to_workspace,
            exec_config.clone(),
            web_config.clone(),
            cron_service.clone(),
            custom_tools,
            retrieval.clone(),
            sessions.clone(),
        )?;
        retrieval.set_tool_registry(&tools);

        let subagent_manager = Arc::new(SubagentManager::new(
            provider.clone(),
            workspace.clone(),
            bus.clone(),
            tools.clone(),
            config.model.clone(),
            config.temperature,
            config.max_tokens,
            config.reasoning_effort.clone(),
            config.max_subagent_iterations,
        ));

        tools.set_spawn_service(subagent_manager);

        let mcp = if mcp_servers.is_empty() {
            None
        } else {
            Some(Arc::new(MCPManager::new(mcp_servers)))
        };
        retrieval.set_mcp_manager(mcp.as_ref());

        Ok(AgentLoop {
            bus,
            provider,
            model: config.model,
            max_iterations: config.max_iterations,
            temperature: config.temperature,
            max_tokens: config.max_tokens,
            memory_window: config.memory_window,
            reasoning_effort: config.reasoning_effort,
            consolidation_config,
            consolidation_enabled: auto_consolidation,
            channel_configs,
            send_usage_summary,
            tools,
            mcp,
            context,
            retrieval,
            sessions,
            running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            session_locks: Arc::new(dashmap::DashMap::new()),
            active_tasks: Arc::new(dashmap::DashMap::new()),
            cancel_signals: Arc::new(dashmap::DashMap::new()),
            last_cleanup: Arc::new(parking_lot::Mutex::new(std::time::Instant::now())),
        })
    }
}

/// Construct the full [`ToolRegistry`] including built-in and retrieval tools.
///
/// Creates a [`ToolRegistryBuilder`] with the given configuration, then
/// registers the three retrieval dynamic tools:
///
/// * `context_search` — search all configured retrieval sources.
/// * `context_sources` — list available retrieval sources.
/// * `context_explain` — explain the last auto-retrieval for this session.
#[allow(clippy::too_many_arguments)]
fn build_tool_registry(
    workspace: PathBuf,
    restrict_to_workspace: bool,
    exec_config: ExecToolConfig,
    web_config: WebToolsConfig,
    cron_service: Option<Arc<CronService>>,
    custom_tools: Vec<Arc<dyn Tool>>,
    retrieval: Arc<RetrievalService>,
    sessions: Arc<SessionManager>,
) -> AgentResult<Arc<ToolRegistry>> {
    let registry = Arc::new(
        ToolRegistryBuilder::new(workspace)
            .restrict_to_workspace(restrict_to_workspace)
            .exec_config(exec_config)
            .web_config(web_config)
            .maybe_cron_service(cron_service)
            .custom_tools(custom_tools)
            .build()
            .context("Failed to build tool registry")?,
    );
    registry.register_dynamic_tool(Arc::new(ContextSearchTool::new(
        retrieval.clone(),
        sessions,
    )))?;
    registry.register_dynamic_tool(Arc::new(ContextSourcesTool::new(retrieval.clone())))?;
    registry.register_dynamic_tool(Arc::new(ContextExplainTool::new(retrieval)))?;

    Ok(registry)
}
