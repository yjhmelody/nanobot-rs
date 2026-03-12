pub mod anthropic;
mod anthropic_types;
pub mod base;
pub mod openai_compat;
mod openai_types;
pub mod proxy;
pub mod registry;
pub mod streaming;
pub mod tool_name;
pub mod traits;

use std::sync::Arc;

use anyhow::{Result, anyhow};

use crate::config::schema::Config;

pub use crate::provider::base::*;
pub use crate::types::provider::*;

use anthropic::AnthropicProvider;
use openai_compat::OpenAICompatProvider;
pub use tool_name::ToolName;

pub fn make_provider(config: &Config) -> Result<Arc<dyn LLMProvider>> {
    let model = config.agents.defaults.model.clone();
    let provider_name = config
        .get_provider_name(Some(&model))
        .ok_or_else(|| anyhow!("no provider matched for model {}", model))?;

    if provider_name == "openai_codex" || provider_name == "github_copilot" {
        return Err(anyhow!(
            "OAuth provider '{}' is not supported as LLM provider. Use ACP as a tool instead.",
            provider_name
        ));
    }

    let provider_cfg = config
        .provider_config(&provider_name)
        .cloned()
        .unwrap_or_default();

    if provider_name != "custom"
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

    let extra_headers = provider_cfg.extra_headers.unwrap_or_default();

    match provider_name.as_str() {
        "anthropic" => Ok(Arc::new(AnthropicProvider::new(
            provider_cfg.api_key,
            api_base,
            model,
            extra_headers,
        ))),
        _ => Ok(Arc::new(OpenAICompatProvider::new(
            provider_cfg.api_key,
            api_base,
            model,
            provider_name,
            extra_headers,
        ))),
    }
}
