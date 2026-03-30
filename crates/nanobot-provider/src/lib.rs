pub mod anthropic;
mod anthropic_types;
pub mod base;
pub mod error;
pub mod fallback;
pub mod openai_compat;
mod openai_types;
pub mod proxy;
pub mod registry;
pub mod streaming;
pub mod traits;

use std::sync::Arc;

use nanobot_config::Config;

pub use crate::base::*;
pub use nanobot_types::provider::*;
pub use error::{ProviderError, ProviderResult};

use anthropic::AnthropicProvider;
use fallback::FallbackProvider;
use openai_compat::OpenAICompatProvider;
pub use nanobot_types::tool_name::ToolName;

/// Constructs an `LLMProvider` from the given configuration.
///
/// Selects the appropriate provider backend (Anthropic, OpenAI-compatible, etc.)
/// based on `config.agents.defaults.model` and `config.agents.defaults.provider`.
/// If `fallback_providers` are configured, wraps providers in a `FallbackProvider`.
pub fn make_provider(config: &Config) -> ProviderResult<Arc<dyn LLMProvider>> {
    let model = config.agents.defaults.model.clone();
    let provider_name = config
        .get_provider_name(Some(&model))
        .ok_or_else(|| ProviderError::InvalidConfig(format!("no provider matched for model {}", model)))?;

    if provider_name == "openai_codex" || provider_name == "github_copilot" {
        return Err(ProviderError::InvalidConfig(format!(
            "OAuth provider '{}' is not supported as LLM provider. Use ACP as a tool instead.",
            provider_name
        )));
    }

    // Check if fallback providers are configured
    if let Some(fallback_names) = &config.agents.defaults.fallback_providers {
        if !fallback_names.is_empty() {
            let mut providers = Vec::new();

            // Add primary provider
            providers.push(create_single_provider(config, &provider_name, &model)?);

            // Add fallback providers
            for fallback_name in fallback_names {
                let fallback_provider = create_single_provider(config, fallback_name, &model)?;
                providers.push(fallback_provider);
            }

            return Ok(Arc::new(FallbackProvider::new(providers, model)));
        }
    }

    // No fallback configured, return single provider
    create_single_provider(config, &provider_name, &model)
}

fn create_single_provider(
    config: &Config,
    provider_name: &str,
    model: &str,
) -> ProviderResult<Arc<dyn LLMProvider>> {
    let provider_cfg = config
        .provider_config(provider_name)
        .cloned()
        .unwrap_or_default();

    if provider_name != "custom"
        && provider_cfg.api_key.trim().is_empty()
        // TODO: what's this model
        && !model.starts_with("bedrock/")
    {
        return Err(ProviderError::InvalidConfig(format!(
            "no API key configured for provider '{}' (model: {})",
            provider_name, model
        )));
    }

    let api_base = if provider_name == "custom" {
        Some(
            provider_cfg
                .api_base
                .clone()
                .unwrap_or_else(|| "http://localhost:8000/v1".to_string()),
        )
    } else {
        config.get_api_base(Some(model))
    };

    let extra_headers = provider_cfg.extra_headers.unwrap_or_default();

    // TODO: this is a bit hacky,
    // we should have a more robust way to determine provider type from model name or config
    match provider_name {
        "anthropic" => Ok(Arc::new(AnthropicProvider::new(
            provider_cfg.api_key,
            api_base,
            model.to_string(),
            extra_headers,
        ))),
        _ => Ok(Arc::new(OpenAICompatProvider::new(
            provider_cfg.api_key,
            api_base,
            model.to_string(),
            provider_name.to_string(),
            extra_headers,
        ))),
    }
}
