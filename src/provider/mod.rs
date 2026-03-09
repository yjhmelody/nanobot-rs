pub mod base;
pub mod copilot_acp;
pub mod openai_compat;
pub mod registry;
pub mod tool_name;

use std::sync::Arc;

use anyhow::{Result, anyhow};

use crate::acp::{ACPConfig, AgentConfig as ACPAgentConfig};
use crate::config::schema::Config;

pub use crate::provider::base::*;
pub use crate::types::provider::*;

use copilot_acp::CopilotAcpProvider;
use openai_compat::OpenAICompatProvider;
pub use tool_name::ToolName;

pub fn make_provider(config: &Config) -> Result<Arc<dyn LLMProvider>> {
    let model = config.agents.defaults.model.clone();
    let provider_name = config
        .get_provider_name(Some(&model))
        .ok_or_else(|| anyhow!("no provider matched for model {}", model))?;

    if provider_name == "openai_codex" {
        return Err(anyhow!(
            "openai_codex OAuth provider is not implemented yet in nanobot-rs MVP"
        ));
    }

    let provider_cfg = config
        .provider_config(&provider_name)
        .cloned()
        .unwrap_or_default();

    if provider_name == "github_copilot" {
        let acp_config = config.acp.clone().unwrap_or_else(ACPConfig::default);
        let agent_config = resolve_copilot_agent_config(&acp_config)?;
        return Ok(Arc::new(CopilotAcpProvider::new(
            model,
            config.workspace_path(),
            agent_config,
        )));
    }

    if provider_name != "github_copilot"
        && provider_name != "openai_codex"
        && provider_name != "custom"
        && provider_cfg.api_key.trim().is_empty()
        && !model.starts_with("bedrock/")
    {
        return Err(anyhow!(
            "no API key configured for provider '{}' (model: {})",
            provider_name,
            model
        ));
    }

    let api_base = if provider_name == "custom" {
        Some(
            provider_cfg
                .api_base
                .clone()
                .unwrap_or_else(|| "http://localhost:8000/v1".to_string()),
        )
    } else {
        config.get_api_base(Some(&model))
    };

    Ok(Arc::new(OpenAICompatProvider::new(
        provider_cfg.api_key,
        api_base,
        model,
        provider_name,
        provider_cfg.extra_headers.unwrap_or_default(),
    )))
}

fn resolve_copilot_agent_config(acp_config: &ACPConfig) -> Result<ACPAgentConfig> {
    acp_config
        .agents
        .get("copilot")
        .cloned()
        .or_else(|| ACPConfig::default().agents.get("copilot").cloned())
        .ok_or_else(|| anyhow!("no ACP agent configuration found for 'copilot'"))
}
