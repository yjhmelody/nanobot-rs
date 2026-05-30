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
#[derive(Clone)]
pub struct RuntimeBundle {
    /// Parsed configuration used to build this runtime.
    pub config: Config,
    /// Central pub/sub message bus.
    pub bus: MessageBus,
    /// The main agent reasoning loop.
    pub agent: Arc<AgentLoop>,
    /// Background cron scheduler.
    pub cron: Arc<CronService>,
    /// Heartbeat service for periodic health checks.
    pub heartbeat: Arc<HeartbeatService>,
}

/// Constructs a fully wired `RuntimeBundle` from the given configuration.
///
/// Initialises the bus, LLM provider, cron service, heartbeat, tool registry,
/// and agent loop in dependency order.
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
    };

    let mut builder = AgentLoopBuilder::new(bus.clone(), provider, workspace)
        .with_config(agent_config)
        .with_consolidation_config(ConsolidationConfig {
            min_messages: defaults.consolidation_min_messages,
            keep_recent: defaults.consolidation_keep_recent,
            max_tokens: defaults.consolidation_summary_max_tokens,
        })
        .with_web_config(config.tools.web.clone())
        .with_exec_config(config.tools.exec.clone())
        .with_mcp_servers(config.tools.mcp_servers.clone())
        .with_restrict_to_workspace(config.tools.restrict_to_workspace)
        .with_cron_service(cron.clone())
        .with_channel_configs(config.channels.clone())
        .with_send_usage_summary(config.channels.defaults.send_usage_summary)
        .with_auto_consolidation(config.agents.defaults.consolidation_enabled);

    // ACP 不是主 provider，而是一个按需注入的“外部编码代理工具”。
    // 这样可以保持主对话模型与外部 coding agent 的职责解耦。
    if let Some(acp_config) = config.acp.clone() {
        builder = builder.with_custom_tool(Arc::new(ACPTool::new(acp_config)));
    }

    let agent = Arc::new(builder.build().await?);

    Ok(RuntimeBundle {
        config,
        bus,
        agent,
        cron,
        heartbeat,
    })
}
