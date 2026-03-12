use std::sync::Arc;

use anyhow::Result;

use crate::agent::{AgentConfig, AgentLoop, AgentLoopBuilder};
use crate::bus::MessageBus;
use crate::config::schema::Config;
use crate::cron::CronService;
use crate::heartbeat::HeartbeatService;
use crate::provider::make_provider;
use crate::utils::helpers::get_data_path;

#[derive(Clone)]
pub struct RuntimeBundle {
    pub config: Config,
    pub bus: MessageBus,
    pub agent: Arc<AgentLoop>,
    pub cron: Arc<CronService>,
    pub heartbeat: Arc<HeartbeatService>,
}

pub async fn build_runtime(config: Config) -> Result<RuntimeBundle> {
    let bus = MessageBus::new();
    let provider = make_provider(&config)?;
    let workspace = config.workspace_path();

    let cron_store_path = get_data_path().await?.join("cron").join("jobs.json");
    let cron = Arc::new(CronService::new(cron_store_path));

    let defaults = &config.agents.defaults;
    let heartbeat = Arc::new(HeartbeatService::new(
        workspace.clone(),
        provider.clone(),
        defaults.model.clone(),
        config.gateway.heartbeat.interval_s,
        config.gateway.heartbeat.enabled,
    ));

    // Use the new builder pattern for AgentLoop construction
    let agent_config = AgentConfig {
        model: defaults.model.clone(),
        max_iterations: defaults.max_tool_iterations,
        temperature: defaults.temperature,
        max_tokens: defaults.max_tokens,
        memory_window: defaults.memory_window,
        reasoning_effort: defaults.reasoning_effort.clone(),
    };

    let agent = Arc::new(
        AgentLoopBuilder::new(bus.clone(), provider, workspace)
            .with_config(agent_config)
            .with_web_config(config.tools.web.clone())
            .with_exec_config(config.tools.exec.clone())
            .with_mcp_servers(config.tools.mcp_servers.clone())
            .with_acp_config(config.acp.clone())
            .with_restrict_to_workspace(config.tools.restrict_to_workspace)
            .with_cron_service(cron.clone())
            .with_send_usage_summary(config.channels.send_usage_summary)
            .with_auto_consolidation(config.agents.defaults.auto_consolidate)
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
