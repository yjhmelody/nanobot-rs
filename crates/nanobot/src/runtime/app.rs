//! Runtime services bundle and bootstrap constructor.
//!
//! `RuntimeBundle` holds references to all core services after they have been
//! wired together. `build_runtime` is the single entry point for assembling
//! these services from a `Config`.

use std::sync::Arc;

use crate::acp::ACPTool;
use crate::error::NanobotResult;

use crate::heartbeat::HeartbeatService;
use crate::utils::helpers::get_data_path;
use nanobot_agent::{AgentConfig, AgentLoop, AgentLoopBuilder};
use nanobot_bus::MessageBus;
use nanobot_config::schema::Config;
use nanobot_cron::CronService;
use nanobot_provider::make_provider;
use nanobot_session::ConsolidationConfig;

/// All runtime services for a running nanobot instance.
///
/// Created by [`build_runtime`]. The bundle is `Clone` because individual
/// fields are `Arc`-wrapped and can be shared across concurrent tasks.
#[derive(Clone)]
pub struct RuntimeBundle {
    /// Parsed configuration used to build this runtime.
    pub config: Config,
    /// Central pub/sub message bus for inbound/outbound message routing.
    pub bus: MessageBus,
    /// The main agent reasoning loop (processes inbound messages).
    pub agent: Arc<AgentLoop>,
    /// Background cron scheduler for timed job execution.
    pub cron: Arc<CronService>,
    /// Heartbeat service for periodic task review and execution.
    pub heartbeat: Arc<HeartbeatService>,
}

/// Construct a fully wired `RuntimeBundle` from the given configuration.
///
/// Initialises the services in dependency order:
/// 1. `MessageBus` — message routing backbone.
/// 2. LLM provider — via `make_provider`.
/// 3. `CronService` — backed by a JSONL store in the data directory.
/// 4. `HeartbeatService` — reads `HEARTBEAT.md` from the workspace.
/// 5. `AgentLoop` — the core reasoning loop with tool registry.
///
/// If ACP is configured, an `ACPTool` is injected as a custom tool.
///
/// # Errors
///
/// Returns a `NanobotError` if the provider cannot be initialised, the
/// agent loop cannot be built, or data directories cannot be resolved.
pub async fn build_runtime(config: Config) -> NanobotResult<RuntimeBundle> {
    let bus = MessageBus::new();
    let provider = make_provider(&config)?;
    let workspace = config.workspace_path();

    let cron_store_path = get_data_path().await?.join("cron").join("jobs.json");
    let cron = Arc::new(CronService::new(cron_store_path));

    let defaults = &config.agents.defaults;
    let active_model = config
        .active_model()
        .unwrap_or_else(|| defaults.model.clone());
    let heartbeat = Arc::new(HeartbeatService::new(
        workspace.clone(),
        provider.clone(),
        active_model.clone(),
        config.gateway.heartbeat.interval_s,
        config.gateway.heartbeat.enabled,
    ));

    let agent_config = AgentConfig {
        model: active_model,
        max_iterations: defaults.max_tool_iterations,
        temperature: defaults.temperature,
        max_tokens: defaults.max_tokens,
        memory_window: defaults.memory_window,
        reasoning_effort: defaults.reasoning_effort.clone(),
        max_subagent_iterations: defaults.max_subagent_iterations,
    };

    let custom_tools = config
        .acp
        .clone()
        // ACP 不是主 provider，而是一个按需注入的外部编码代理工具。
        .map(|acp_config| Arc::new(ACPTool::new(acp_config)) as Arc<dyn nanobot_tools::Tool>)
        .into_iter()
        .collect();

    let agent = Arc::new(
        AgentLoopBuilder::new(bus.clone(), provider, workspace)
            .config(agent_config)
            .consolidation_config(ConsolidationConfig {
                min_messages: defaults.consolidation_min_messages,
                keep_recent: defaults.consolidation_keep_recent,
                max_tokens: defaults.consolidation_summary_max_tokens,
            })
            .web_config(config.tools.web.clone())
            .exec_config(config.tools.exec.clone())
            .mcp_servers(config.tools.mcp_servers.clone())
            .restrict_to_workspace(config.tools.restrict_to_workspace)
            .retrieval_config(config.retrieval.clone())
            .cron_service(cron.clone())
            .channel_configs(config.channels.clone())
            .send_usage_summary(config.channels.defaults.send_usage_summary)
            .auto_consolidation(config.agents.defaults.consolidation_enabled)
            .custom_tools(custom_tools)
            .build()
            .await?,
    );

    Ok(RuntimeBundle {
        config,
        bus,
        agent,
        cron,
        heartbeat,
    })
}
