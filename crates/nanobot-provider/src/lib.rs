//! Provider selection and construction for nanobot's LLM backends.
//!
//! This crate defines and implements the [`LLMProvider`] trait, which is the abstraction
//! layer between nanobot's agent loop and various LLM APIs. It supports:
//!
//! - **Anthropic** (Claude): Native support via their Messages API
//! - **OpenAI-compatible**: Any provider implementing the OpenAI chat completions or
//!   responses wire format (OpenAI, OpenRouter, DeepSeek, etc.)
//! - **Fallback chains**: Automatically retry with alternative providers on failure
//! - **Streaming**: Unified [`StreamEvent`] stream regardless of the upstream format
//!   (Anthropic SSE, OpenAI SSE, etc.)
//!
//! # Architecture
//!
//! The crate's entry point is [`make_provider`], which reads the application [`Config`]
//! and constructs the appropriate provider chain. Each concrete provider
//! (`AnthropicProvider`, `OpenAICompatProvider`) implements the [`LLMProvider`] trait.
//! If `fallback_providers` are configured, a [`FallbackProvider`] wraps them with
//! automatic retry logic.
//!
//! Streaming is handled by provider-specific adapters (`SseAdapter`, `OpenAiAdapter`)
//! that convert raw HTTP byte streams into a unified [`StreamEvent`] stream.
//! The [`StreamAccumulator`] can then collect these events into a complete
//! [`LLMResponse`].
//!
//! # Key Design Decisions
//!
//! - **Trait-first abstraction**: [`LLMProvider`] is the core trait; providers are
//!   injected as `Arc<dyn LLMProvider>`, enabling easy testing and new provider additions.
//! - **Proxy fallback**: [`ProxyFallbackHelper`] implements a "try with proxy, retry
//!   without proxy" pattern for environments where a system proxy may be broken.
//! - **Model resolution**: [`OpenAICompatProvider`] applies provider-specific model name
//!   canonicalization rules from a static registry (`ProviderSpec`).
//!
//! # Dependencies
//!
//! - `nanobot-config` / `nanobot-types` / `nanobot-types-derive`: Shared types and
//!   configuration schema
//! - `reqwest`: HTTP client for API calls
//! - `rmcp`: Model Context Protocol (used externally, not directly in this crate)

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
pub mod tool_name;
pub mod traits;

use std::sync::Arc;

use nanobot_config::Config;
use nanobot_config::schema::ProviderType;

pub use crate::base::*;
pub use error::{ProviderError, ProviderResult};
pub use nanobot_types::provider::*;

use anthropic::AnthropicProvider;
use fallback::FallbackProvider;
pub use nanobot_types::tool_name::ToolName;
use openai_compat::OpenAICompatProvider;

/// Constructs an `LLMProvider` from the given configuration.
///
/// Selects the appropriate provider backend (Anthropic, OpenAI-compatible, etc.)
/// based on `config.agents.defaults.model` and `config.agents.defaults.provider`.
/// If `fallback_providers` are configured, wraps providers in a `FallbackProvider`.
pub fn make_provider(config: &Config) -> ProviderResult<Arc<dyn LLMProvider>> {
    let provider_name = config.get_provider_name(None).ok_or_else(|| {
        ProviderError::InvalidConfig("no provider matched for current configuration".to_string())
    })?;
    let model = config.model_for_provider(&provider_name);

    if config.provider_type(&provider_name) == ProviderType::OAuth {
        return Err(ProviderError::InvalidConfig(format!(
            "OAuth provider '{}' is not supported as LLM provider. Use ACP as a tool instead.",
            provider_name
        )));
    }

    // Check if fallback providers are configured
    if let Some(fallback_names) = &config.agents.defaults.fallback_providers
        && !fallback_names.is_empty()
    {
        let mut providers = Vec::new();

        // Add primary provider
        providers.push(create_single_provider(config, &provider_name)?);

        // Add fallback providers
        for fallback_name in fallback_names {
            let fallback_provider = create_single_provider(config, fallback_name)?;
            providers.push(fallback_provider);
        }

        return Ok(Arc::new(FallbackProvider::new(providers, model)));
    }

    // No fallback configured, return single provider
    create_single_provider(config, &provider_name)
}

/// Creates a single provider based on the given configuration and provider name.
///
/// This is an internal helper that reads the provider type from the config and
/// dispatches to the appropriate concrete implementation. It does NOT wrap the
/// result in a [`FallbackProvider`] — that is done by [`make_provider`] when
/// `fallback_providers` are configured.
///
/// # Errors
///
/// Returns [`ProviderError::InvalidConfig`] if:
/// - The provider type is `OAuth` (not supported as an LLM provider)
/// - The API key is missing or empty (unless the model starts with `bedrock/`)
fn create_single_provider(
    config: &Config,
    provider_name: &str,
) -> ProviderResult<Arc<dyn LLMProvider>> {
    let provider_cfg = config
        .provider_config(provider_name)
        .cloned()
        .unwrap_or_default();
    let provider_type = config.provider_type(provider_name);
    let wire_api = config.wire_api(provider_name);
    let model = config.model_for_provider(provider_name);

    tracing::debug!(
        "Creating provider '{}' for model '{}', api_key set: {}, api_base: {:?}",
        provider_name,
        model,
        !provider_cfg.api_key.trim().is_empty(),
        provider_cfg.api_base
    );

    if provider_type != ProviderType::OAuth
        && provider_name != "custom"
        && provider_cfg.api_key.trim().is_empty()
        && !model.starts_with("bedrock/")
    {
        return Err(ProviderError::InvalidConfig(format!(
            "no API key configured for provider '{}' (model: {})",
            provider_name, model
        )));
    }

    // For the "custom" provider name, always supply a default API base if none is
    // configured (localhost:8000). For all other providers, use the configured base
    // only if it is non-empty; otherwise let the provider fall back to its own default.
    let api_base = if provider_name == "custom" {
        Some(
            provider_cfg
                .api_base
                .clone()
                .unwrap_or_else(|| "http://localhost:8000/v1".to_string()),
        )
    } else {
        provider_cfg
            .api_base
            .clone()
            .filter(|base| !base.trim().is_empty())
    };

    let extra_headers = provider_cfg.extra_headers.unwrap_or_default();

    // TODO: this is a bit hacky,
    // we should have a more robust way to determine provider type from model name or config
    match provider_type {
        ProviderType::Anthropic => Ok(Arc::new(AnthropicProvider::new(
            provider_cfg.api_key,
            api_base,
            model,
            extra_headers,
        ))),
        ProviderType::OpenAiCompatible => Ok(Arc::new(OpenAICompatProvider::new(
            provider_cfg.api_key,
            api_base,
            model,
            provider_name.to_string(),
            wire_api,
            extra_headers,
        ))),
        ProviderType::OAuth => Err(ProviderError::InvalidConfig(format!(
            "OAuth provider '{}' is not supported as LLM provider. Use ACP as a tool instead.",
            provider_name
        ))),
    }
}
