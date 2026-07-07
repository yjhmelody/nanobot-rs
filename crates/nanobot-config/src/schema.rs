use std::collections::HashMap;
use std::path::PathBuf;

use crate::error::{ConfigError, ConfigResult};
use nanobot_types::provider::ReasoningConfig;
use serde::{Deserialize, Deserializer, Serialize};

/// Top-level configuration loaded from config files and defaults.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    /// Agent configuration (defaults, memory, model selection).
    pub agents: AgentsConfig,
    /// Channel adapter configuration and flags.
    pub channels: ChannelsConfig,
    /// Provider configuration map.
    pub providers: ProvidersConfig,
    /// Gateway server configuration.
    pub gateway: GatewayConfig,
    /// Tool subsystem configuration.
    pub tools: ToolsConfig,
    /// Retrieval context layer configuration.
    pub retrieval: RetrievalConfig,

    #[serde(skip_serializing_if = "Option::is_none")]
    /// Optional ACP configuration for external agent tools.
    pub acp: Option<crate::acp::ACPConfig>,
}

impl Config {
    /// Returns the workspace path, expanding tilde if present.
    ///
    /// The workspace is the base directory for all agent operations including
    /// file tools, session storage, and memory persistence.
    ///
    /// # Example
    ///
    /// ```
    /// use nanobot_config::schema::Config;
    ///
    /// let config = Config::default();
    /// let workspace = config.workspace_path();
    /// assert!(workspace.is_absolute() || workspace.starts_with("~"));
    /// ```
    pub fn workspace_path(&self) -> PathBuf {
        expand_tilde(&self.agents.defaults.workspace)
            .unwrap_or_else(|_| PathBuf::from("~/.nanobot/workspace"))
    }
}

fn expand_tilde(raw: &str) -> Result<PathBuf, String> {
    if let Some(rest) = raw.strip_prefix("~/") {
        let home =
            dirs::home_dir().ok_or_else(|| "failed to resolve home directory".to_string())?;
        Ok(home.join(rest))
    } else {
        Ok(PathBuf::from(raw))
    }
}

impl Config {
    /// Validates the configuration for correctness.
    ///
    /// This method checks all configuration parameters and returns an error
    /// if any invalid values are found.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - `max_tokens` is not positive
    /// - `temperature` is not in range [0.0, 2.0]
    /// - `max_tool_iterations` is zero
    /// - `memory_window` is zero
    /// - `consolidation_keep_recent` is zero or greater than `memory_window`
    /// - `exec.timeout` is zero
    /// - `heartbeat.interval_s` is zero when enabled
    pub fn validate(&self) -> ConfigResult<()> {
        // Validate agent defaults
        self.agents.defaults.validate()?;

        // Validate tools config
        self.tools.validate()?;

        // Validate gateway config
        self.gateway.validate()?;

        // Validate retrieval config
        self.retrieval.validate()?;

        for (name, instance) in &self.channels.instances {
            if let Some(overrides) = instance.agent_overrides() {
                self.agents.defaults.validate_overrides(
                    overrides,
                    &format!("channels.instances.{name}.agentOverrides"),
                )?;
            }
        }

        Ok(())
    }

    /// Determines the provider name based on model and configuration.
    ///
    /// This method implements the provider selection logic:
    /// 1. If `agents.defaults.provider` is set and not "auto", use it
    /// 2. If model has a provider prefix (e.g., "openai/gpt-4"), and it exists in config, use it
    /// 3. Fall back to the first configured non-OAuth provider with an API key
    ///
    /// # Arguments
    ///
    /// * `model` - Optional model name. If None, uses `agents.defaults.model`
    ///
    /// # Returns
    ///
    /// Returns the provider name (e.g., "anthropic", "openai", "my_provider"), or None if no provider is configured.
    ///
    /// # Example
    ///
    /// ```
    /// use nanobot_config::schema::{Config, ProviderConfig};
    ///
    /// let mut config = Config::default();
    /// if let Some(provider) = config.providers.get_mut("anthropic") {
    ///     provider.api_key = "sk-xxx".to_string();
    /// }
    ///
    /// let provider = config.get_provider_name(Some("claude-3-opus"));
    /// assert_eq!(provider.as_deref(), Some("anthropic"));
    /// ```
    pub fn get_provider_name(&self, model: Option<&str>) -> Option<String> {
        let forced = normalize_provider_name(&self.agents.defaults.provider);
        if !forced.is_empty() && forced != "auto" {
            return Some(forced);
        }

        let target_model = model.unwrap_or(&self.agents.defaults.model).to_lowercase();
        if let Some((prefix, _)) = target_model.split_once('/') {
            let normalized_prefix = normalize_provider_name(prefix);
            if self.providers.get(&normalized_prefix).is_some() {
                return Some(normalized_prefix);
            }
        }

        let mut candidates = self
            .providers
            .iter()
            .filter_map(|(name, cfg)| {
                if cfg.has_auth() && self.provider_type(name) != ProviderType::OAuth {
                    Some(name.clone())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        candidates.sort();
        candidates.into_iter().next()
    }

    /// Returns the provider configuration for the specified model.
    ///
    /// This is a convenience method that combines `get_provider_name()` and `provider_config()`.
    ///
    /// # Arguments
    ///
    /// * `model` - Optional model name
    ///
    /// # Returns
    ///
    /// Returns a cloned provider configuration, or None if no provider is found.
    pub fn get_provider(&self, model: Option<&str>) -> Option<ProviderConfig> {
        let name = self.get_provider_name(model)?;
        self.provider_config(&name).cloned()
    }

    /// Resolves the model for a given provider.
    ///
    /// Prefers `providers.<name>.model` when configured, otherwise falls back to
    /// `agents.defaults.model`.
    pub fn model_for_provider(&self, name: &str) -> String {
        self.provider_config(name)
            .and_then(|cfg| cfg.model.as_ref())
            .map(|model| model.trim())
            .filter(|model| !model.is_empty())
            .map(ToString::to_string)
            .unwrap_or_else(|| self.agents.defaults.model.clone())
    }

    /// Resolves the runtime model using the currently selected provider.
    pub fn active_model(&self) -> Option<String> {
        let provider = self.get_provider_name(None)?;
        Some(self.model_for_provider(&provider))
    }

    /// Returns the API base URL for the specified model's provider.
    ///
    /// Returns configured `api_base` when it is set and non-empty.
    ///
    /// # Arguments
    ///
    /// * `model` - Optional model name
    ///
    /// # Returns
    ///
    /// Returns the API base URL, or None if the provider has no default URL.
    pub fn get_api_base(&self, model: Option<&str>) -> Option<String> {
        let name = self.get_provider_name(model)?;
        let provider = self.provider_config(&name)?;
        provider
            .api_base
            .as_ref()
            .filter(|base| !base.trim().is_empty())
            .cloned()
    }

    /// Returns the provider configuration for the specified provider name.
    ///
    /// # Arguments
    ///
    /// * `name` - Provider name (e.g., "anthropic", "openai", "custom")
    ///
    /// # Returns
    ///
    /// Returns a reference to the provider config, or None if the provider is unknown.
    ///
    /// # Supported Providers
    ///
    /// - Any configured provider key in `providers`
    pub fn provider_config(&self, name: &str) -> Option<&ProviderConfig> {
        self.providers.get(name)
    }

    /// Resolves provider protocol type from explicit config.
    ///
    /// Defaults to `OpenAiCompatible` when `provider_type` is not configured.
    pub fn provider_type(&self, name: &str) -> ProviderType {
        let normalized = normalize_provider_name(name);
        if let Some(cfg) = self.providers.get(&normalized)
            && let Some(kind) = cfg.provider_type
        {
            return kind;
        }
        ProviderType::OpenAiCompatible
    }

    /// Resolves wire API type for OpenAI-compatible providers.
    ///
    /// Defaults to `Responses` when `wire_api` is not configured.
    pub fn wire_api(&self, name: &str) -> ProviderWireApi {
        let normalized = normalize_provider_name(name);
        if let Some(cfg) = self.providers.get(&normalized)
            && let Some(kind) = cfg.wire_api
        {
            return kind;
        }
        ProviderWireApi::Responses
    }
}

pub fn normalize_provider_name(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    if trimmed.eq_ignore_ascii_case("auto") {
        return "auto".to_string();
    }

    let mut out = String::with_capacity(trimmed.len());
    let mut prev_is_sep = false;
    for ch in trimmed.chars() {
        match ch {
            '-' | ' ' => {
                if !out.is_empty() && !prev_is_sep {
                    out.push('_');
                }
                prev_is_sep = true;
            }
            c if c.is_ascii_uppercase() => {
                if !out.is_empty() && !prev_is_sep {
                    out.push('_');
                }
                out.push(c.to_ascii_lowercase());
                prev_is_sep = false;
            }
            c if c.is_ascii_alphanumeric() || c == '_' => {
                out.push(c.to_ascii_lowercase());
                prev_is_sep = c == '_';
            }
            _ => {
                if !out.is_empty() && !prev_is_sep {
                    out.push('_');
                }
                prev_is_sep = true;
            }
        }
    }
    out.trim_matches('_').to_string()
}

/// Agent-related configuration (defaults and model settings).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct AgentsConfig {
    /// Default agent settings applied to all sessions.
    pub defaults: AgentDefaults,
}

/// Default settings applied to all agents unless overridden.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AgentDefaults {
    /// Base workspace directory for agent operations.
    pub workspace: String,
    /// Default model identifier (provider/model).
    pub model: String,
    /// Provider override ("auto" to infer from model).
    pub provider: String,
    /// Fallback providers to try in order when primary provider fails.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback_providers: Option<Vec<String>>,
    /// Max tokens for model responses.
    pub max_tokens: i32,
    /// Sampling temperature for model responses.
    pub temperature: f32,
    /// Maximum tool iterations per turn.
    pub max_tool_iterations: usize,
    /// Maximum iterations for subagent tasks.
    pub max_subagent_iterations: usize,
    /// Number of recent messages to include in context.
    pub memory_window: usize,
    /// Number of recent messages kept as raw turns during consolidation.
    pub consolidation_keep_recent: usize,
    /// Optional reasoning/thinking configuration.
    /// Provider-agnostic: Anthropic reads `type`/`budget_tokens`, OpenAI reads `effort`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<ReasoningConfig>,
    /// Enables automatic session consolidation after saving turns.
    pub consolidation_enabled: bool,
    /// Minimum number of unconsolidated messages before consolidation runs.
    pub consolidation_min_messages: usize,
    /// Maximum tokens used for the consolidation summary request.
    pub consolidation_summary_max_tokens: i32,
    /// Custom prompt configuration for this agent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<PromptConfig>,
}

/// Optional agent runtime overrides that can be applied per channel.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct AgentRuntimeOverrides {
    /// Override for the number of recent messages included in context.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_window: Option<usize>,
    /// Override for enabling automatic session consolidation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consolidation_enabled: Option<bool>,
    /// Override for the number of recent raw messages preserved during consolidation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consolidation_keep_recent: Option<usize>,
    /// Override for the unconsolidated-message threshold before consolidation runs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consolidation_min_messages: Option<usize>,
    /// Override for the summarization token budget used by consolidation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consolidation_summary_max_tokens: Option<i32>,
    /// Override for retrieval enablement.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retrieval_enabled: Option<bool>,
    /// Override for automatic retrieval injection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retrieval_auto_inject: Option<bool>,
    /// Override for retrieval hit budget.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retrieval_max_hits: Option<usize>,
    /// Override for retrieval context token budget.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retrieval_max_context_tokens: Option<usize>,
    /// Override for retrieval source allowlist.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retrieval_source_allowlist: Option<Vec<String>>,
}

/// Configuration for agent prompts
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct PromptConfig {
    /// Name of the prompt template to use
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,

    /// Variables to substitute in the template
    #[serde(default)]
    pub variables: HashMap<String, String>,

    /// Inline system prompt (overrides template)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,

    /// Additional custom instructions
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom: Option<String>,
}

impl Default for AgentDefaults {
    fn default() -> Self {
        Self {
            workspace: "~/.nanobot/workspace".to_string(),
            model: "anthropic/claude-sonnet-4-5".to_string(),
            provider: "auto".to_string(),
            fallback_providers: None,
            max_tokens: 8192,
            temperature: 0.1,
            max_tool_iterations: 40,
            max_subagent_iterations: 15,
            memory_window: 100,
            consolidation_keep_recent: 10,
            reasoning_effort: None,
            consolidation_enabled: true,
            consolidation_min_messages: 20,
            consolidation_summary_max_tokens: 1000,
            prompt: None,
        }
    }
}

impl AgentDefaults {
    /// Validates agent default configuration.
    pub fn validate(&self) -> ConfigResult<()> {
        if self.max_tokens <= 0 {
            return Err(ConfigError::invalid(format!(
                "max_tokens must be positive, got {}",
                self.max_tokens
            )));
        }

        if !(0.0..=2.0).contains(&self.temperature) {
            return Err(ConfigError::invalid(format!(
                "temperature must be in range [0.0, 2.0], got {}",
                self.temperature
            )));
        }

        if self.max_tool_iterations == 0 {
            return Err(ConfigError::invalid("max_tool_iterations must be positive"));
        }

        if self.memory_window == 0 {
            return Err(ConfigError::invalid("memory_window must be positive"));
        }

        if self.consolidation_keep_recent == 0 {
            return Err(ConfigError::invalid(
                "consolidation_keep_recent must be positive",
            ));
        }

        if self.consolidation_keep_recent > self.memory_window {
            return Err(ConfigError::invalid(format!(
                "consolidation_keep_recent ({}) cannot be greater than memory_window ({})",
                self.consolidation_keep_recent, self.memory_window
            )));
        }

        if self.consolidation_min_messages == 0 {
            return Err(ConfigError::invalid(
                "consolidation_min_messages must be positive",
            ));
        }

        if self.consolidation_summary_max_tokens <= 0 {
            return Err(ConfigError::invalid(format!(
                "consolidation_summary_max_tokens must be positive, got {}",
                self.consolidation_summary_max_tokens
            )));
        }

        if self.workspace.trim().is_empty() {
            return Err(ConfigError::invalid("workspace path cannot be empty"));
        }

        if self.model.trim().is_empty() {
            return Err(ConfigError::invalid("model name cannot be empty"));
        }

        Ok(())
    }

    /// Validates a channel-specific override set against these defaults.
    pub fn validate_overrides(
        &self,
        overrides: &AgentRuntimeOverrides,
        scope: &str,
    ) -> ConfigResult<()> {
        let memory_window = overrides.memory_window.unwrap_or(self.memory_window);
        let consolidation_keep_recent = overrides
            .consolidation_keep_recent
            .unwrap_or(self.consolidation_keep_recent);
        let consolidation_min_messages = overrides
            .consolidation_min_messages
            .unwrap_or(self.consolidation_min_messages);
        let consolidation_summary_max_tokens = overrides
            .consolidation_summary_max_tokens
            .unwrap_or(self.consolidation_summary_max_tokens);

        if memory_window == 0 {
            return Err(ConfigError::invalid(format!(
                "{scope}: memory_window must be positive"
            )));
        }

        if consolidation_keep_recent == 0 {
            return Err(ConfigError::invalid(format!(
                "{scope}: consolidation_keep_recent must be positive"
            )));
        }

        if consolidation_keep_recent > memory_window {
            return Err(ConfigError::invalid(format!(
                "{scope}: consolidation_keep_recent ({consolidation_keep_recent}) cannot be greater than memory_window ({memory_window})"
            )));
        }

        if consolidation_min_messages == 0 {
            return Err(ConfigError::invalid(format!(
                "{scope}: consolidation_min_messages must be positive"
            )));
        }

        if consolidation_summary_max_tokens <= 0 {
            return Err(ConfigError::invalid(format!(
                "{scope}: consolidation_summary_max_tokens must be positive, got {consolidation_summary_max_tokens}"
            )));
        }
        if overrides.retrieval_max_hits == Some(0) {
            return Err(ConfigError::invalid(format!(
                "{scope}: retrieval_max_hits must be positive"
            )));
        }
        if overrides.retrieval_max_context_tokens == Some(0) {
            return Err(ConfigError::invalid(format!(
                "{scope}: retrieval_max_context_tokens must be positive"
            )));
        }

        Ok(())
    }
}

/// Configuration for outbound channel adapters.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum StreamMode {
    #[default]
    /// Edit the same message for both progress and final output.
    UpdateAll,
    /// Edit only progress messages; send final output as a new message.
    UpdateProgress,
    /// Always send new messages; never edit.
    Append,
}

/// Default runtime settings shared across all channel instances.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ChannelDefaults {
    /// Emit progress events during long-running tasks.
    pub send_progress: bool,
    /// Emit tool hints when tools are invoked.
    pub send_tool_hints: bool,
    /// Send a usage summary message after the final response.
    pub send_usage_summary: bool,
    /// Streaming message behavior for adapters that support edits.
    pub stream_mode: StreamMode,
}

impl Default for ChannelDefaults {
    fn default() -> Self {
        Self {
            send_progress: true,
            send_tool_hints: false,
            send_usage_summary: false,
            stream_mode: StreamMode::UpdateAll,
        }
    }
}

/// Configuration for outbound channel adapters.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ChannelsConfig {
    /// Default settings inherited by all instances.
    pub defaults: ChannelDefaults,
    /// Named channel instances, keyed by user-defined instance name.
    pub instances: HashMap<String, ChannelInstanceConfig>,
}

/// A single named channel instance configuration.
///
/// Internally-tagged enum: the `channelType` field selects the variant,
/// and the remaining fields are deserialized into the variant's struct.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "channelType", rename_all = "camelCase")]
pub enum ChannelInstanceConfig {
    #[serde(rename = "telegram")]
    Telegram(TelegramChannelConfig),
    #[serde(rename = "feishu", alias = "lark")]
    Feishu(FeishuChannelConfig),
}

/// Telegram channel instance configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TelegramChannelConfig {
    /// Whether this instance is enabled.
    pub enabled: bool,
    /// Allowed sender IDs or chat IDs.
    #[serde(default)]
    pub allow_from: Vec<String>,
    /// Bot token.
    pub token: String,
    /// Optional API base URL override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_base: Option<String>,
    /// Override default: emit progress events.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub send_progress: Option<bool>,
    /// Override default: emit tool hints.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub send_tool_hints: Option<bool>,
    /// Override default: send usage summary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub send_usage_summary: Option<bool>,
    /// Override default: streaming message behavior.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_mode: Option<StreamMode>,
    /// Optional per-channel agent runtime overrides.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_overrides: Option<AgentRuntimeOverrides>,
}

impl Default for TelegramChannelConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            allow_from: Vec::new(),
            token: String::new(),
            api_base: None,
            send_progress: None,
            send_tool_hints: None,
            send_usage_summary: None,
            stream_mode: None,
            agent_overrides: None,
        }
    }
}

/// Feishu / Lark channel instance configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FeishuChannelConfig {
    /// Whether this instance is enabled.
    pub enabled: bool,
    /// Allowed sender IDs or chat IDs.
    #[serde(default)]
    pub allow_from: Vec<String>,
    /// Feishu app ID.
    pub app_id: String,
    /// Feishu app secret.
    pub app_secret: String,
    /// Webhook URL (simple delivery mode).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub webhook_url: Option<String>,
    /// Verify token for event callbacks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verify_token: Option<String>,
    /// Optional API base URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_base: Option<String>,
    /// Sign verification secret for webhook callbacks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
    /// Enable event callback server.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_enabled: Option<bool>,
    /// Enable WebSocket mode (default: true when appId+appSecret present).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ws_enabled: Option<bool>,
    /// Callback server listen address.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub callback_listen: Option<String>,
    /// Callback server path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub callback_path: Option<String>,
    /// Enable typing indicator placeholder during streaming.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_placeholder_enabled: Option<bool>,
    /// Text for the streaming placeholder.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_placeholder_text: Option<String>,
    /// Override default: emit progress events.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub send_progress: Option<bool>,
    /// Override default: emit tool hints.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub send_tool_hints: Option<bool>,
    /// Override default: send usage summary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub send_usage_summary: Option<bool>,
    /// Override default: streaming message behavior.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_mode: Option<StreamMode>,
    /// Message render mode: "raw" (text), "card" (interactive), "auto" (sniff).
    /// Default is "raw" for backward compatibility.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub render_mode: Option<String>,
    /// Optional per-channel agent runtime overrides.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_overrides: Option<AgentRuntimeOverrides>,
}

impl Default for FeishuChannelConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            allow_from: Vec::new(),
            app_id: String::new(),
            app_secret: String::new(),
            webhook_url: None,
            verify_token: None,
            api_base: None,
            secret: None,
            event_enabled: None,
            ws_enabled: None,
            callback_listen: None,
            callback_path: None,
            stream_placeholder_enabled: None,
            stream_placeholder_text: None,
            send_progress: None,
            send_tool_hints: None,
            send_usage_summary: None,
            stream_mode: None,
            render_mode: None,
            agent_overrides: None,
        }
    }
}

impl ChannelInstanceConfig {
    pub fn enabled(&self) -> bool {
        match self {
            Self::Telegram(c) => c.enabled,
            Self::Feishu(c) => c.enabled,
        }
    }

    pub fn allow_from(&self) -> &[String] {
        match self {
            Self::Telegram(c) => &c.allow_from,
            Self::Feishu(c) => &c.allow_from,
        }
    }

    pub fn agent_overrides(&self) -> Option<&AgentRuntimeOverrides> {
        match self {
            Self::Telegram(c) => c.agent_overrides.as_ref(),
            Self::Feishu(c) => c.agent_overrides.as_ref(),
        }
    }
}

/// Protocol type used by a provider endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ProviderType {
    /// OpenAI-compatible API (Responses API style).
    #[default]
    #[serde(
        rename = "open_ai_compatible",
        alias = "openai",
        alias = "open_ai",
        alias = "open_ai_compatible",
        alias = "openai_compatible"
    )]
    OpenAiCompatible,
    /// Anthropic Messages API.
    #[serde(rename = "anthropic")]
    Anthropic,
    /// OAuth-only provider (not used as main LLM provider).
    #[serde(rename = "oauth", alias = "o_auth")]
    OAuth,
}

/// Wire protocol variant for OpenAI-compatible providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ProviderWireApi {
    /// OpenAI Responses API (`/responses`).
    #[default]
    #[serde(rename = "responses")]
    Responses,
    /// Legacy OpenAI Chat Completions API (`/chat/completions`).
    #[serde(
        rename = "chat_completions",
        alias = "chat_completion",
        alias = "chat",
        alias = "completions"
    )]
    ChatCompletions,
}

/// Provider settings for a single LLM backend.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct ProviderConfig {
    /// Optional provider protocol override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_type: Option<ProviderType>,
    /// Optional wire API override for OpenAI-compatible providers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wire_api: Option<ProviderWireApi>,
    /// Optional default model for this provider.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// API key for the provider.
    pub api_key: String,
    /// Optional API base URL override.
    pub api_base: Option<String>,
    /// Optional extra headers for API requests.
    pub extra_headers: Option<HashMap<String, String>>,
}

impl ProviderConfig {
    pub fn has_auth(&self) -> bool {
        !self.api_key.trim().is_empty()
    }
}

/// Collection of all configured providers.
#[derive(Debug, Clone, Serialize)]
#[serde(transparent)]
pub struct ProvidersConfig(pub HashMap<String, ProviderConfig>);

impl Default for ProvidersConfig {
    fn default() -> Self {
        let mut providers = HashMap::new();
        providers.insert(
            "anthropic".to_string(),
            ProviderConfig {
                provider_type: Some(ProviderType::Anthropic),
                ..ProviderConfig::default()
            },
        );
        providers.insert("openai".to_string(), ProviderConfig::default());
        providers.insert(
            "github_copilot".to_string(),
            ProviderConfig {
                provider_type: Some(ProviderType::OAuth),
                ..ProviderConfig::default()
            },
        );
        Self(providers)
    }
}

impl ProvidersConfig {
    pub fn get(&self, name: &str) -> Option<&ProviderConfig> {
        let normalized = normalize_provider_name(name);
        self.0.get(&normalized)
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut ProviderConfig> {
        let normalized = normalize_provider_name(name);
        self.0.get_mut(&normalized)
    }

    pub fn insert(&mut self, name: impl Into<String>, config: ProviderConfig) {
        let normalized = normalize_provider_name(&name.into());
        self.0.insert(normalized, config);
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &ProviderConfig)> {
        self.0.iter()
    }
}

impl<'de> Deserialize<'de> for ProvidersConfig {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = HashMap::<String, ProviderConfig>::deserialize(deserializer)?;
        let mut providers = Self::default();

        for (key, value) in raw {
            providers.insert(key, value);
        }

        Ok(providers)
    }
}

/// Gateway server configuration (host/port/heartbeat).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GatewayConfig {
    /// Host address to bind the gateway server.
    pub host: String,
    /// Port to bind the gateway server.
    pub port: u16,
    /// Heartbeat configuration for gateway runtime.
    pub heartbeat: HeartbeatConfig,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".to_string(),
            port: 18790,
            heartbeat: HeartbeatConfig::default(),
        }
    }
}

impl GatewayConfig {
    /// Validates gateway configuration.
    pub fn validate(&self) -> ConfigResult<()> {
        // Gateway host/port endpoint is reserved and currently unused.
        // Keep fields for config compatibility, but skip endpoint validation for now.

        self.heartbeat.validate()?;

        Ok(())
    }
}

/// Heartbeat polling configuration for periodic tasks.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct HeartbeatConfig {
    /// Whether heartbeat processing is enabled.
    pub enabled: bool,
    /// Polling interval in seconds.
    pub interval_s: u64,
}

impl Default for HeartbeatConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_s: 30 * 60,
        }
    }
}

impl HeartbeatConfig {
    /// Validates heartbeat configuration.
    pub fn validate(&self) -> ConfigResult<()> {
        if self.enabled && self.interval_s == 0 {
            return Err(ConfigError::invalid(
                "heartbeat interval_s cannot be zero when enabled",
            ));
        }

        Ok(())
    }
}

/// Configuration for tool subsystems (web, exec, MCP).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct ToolsConfig {
    /// Web tool configuration.
    pub web: WebToolsConfig,
    /// Exec tool configuration.
    pub exec: ExecToolConfig,
    /// Restrict file tools to workspace.
    pub restrict_to_workspace: bool,
    /// MCP server configurations.
    pub mcp_servers: HashMap<String, MCPServerConfig>,
}

impl ToolsConfig {
    /// Validates tools configuration.
    pub fn validate(&self) -> ConfigResult<()> {
        self.web.validate()?;
        self.exec.validate()?;

        // Validate MCP servers
        for (name, server) in &self.mcp_servers {
            if name.trim().is_empty() {
                return Err(ConfigError::invalid("MCP server name cannot be empty"));
            }
            server.validate()?;
        }

        Ok(())
    }
}

/// Retrieval context layer configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RetrievalConfig {
    /// Enables retrieval context features.
    pub enabled: bool,
    /// Inject retrieved context before the first model call of a turn.
    pub auto_inject: bool,
    /// Maximum retrieved snippets to keep after source collection.
    pub max_hits: usize,
    /// Approximate token budget for injected retrieved context.
    pub max_context_tokens: usize,
    /// Per-source timeout in milliseconds.
    pub source_timeout_ms: u64,
    /// Explicitly configured retrieval sources.
    pub sources: HashMap<String, RetrievalSourceConfig>,
}

impl Default for RetrievalConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            auto_inject: true,
            max_hits: 8,
            max_context_tokens: 3000,
            source_timeout_ms: 1500,
            sources: HashMap::new(),
        }
    }
}

impl RetrievalConfig {
    /// Validates retrieval configuration.
    pub fn validate(&self) -> ConfigResult<()> {
        if self.max_hits == 0 {
            return Err(ConfigError::invalid("retrieval max_hits must be positive"));
        }
        if self.max_context_tokens == 0 {
            return Err(ConfigError::invalid(
                "retrieval max_context_tokens must be positive",
            ));
        }
        if self.source_timeout_ms == 0 {
            return Err(ConfigError::invalid(
                "retrieval source_timeout_ms must be positive",
            ));
        }
        for (name, source) in &self.sources {
            if name.trim().is_empty() {
                return Err(ConfigError::invalid(
                    "retrieval source name cannot be empty",
                ));
            }
            source.validate(name)?;
        }
        Ok(())
    }
}

/// Retrieval source adapter kind.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RetrievalSourceKind {
    Memory,
    Workspace,
    McpTool,
    McpResource,
}

/// Configuration for a single retrieval source.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RetrievalSourceConfig {
    /// Adapter kind.
    pub kind: RetrievalSourceKind,
    /// Enables this source.
    pub enabled: bool,
    /// Include globs for workspace source.
    pub include: Vec<String>,
    /// Exclude globs for workspace source.
    pub exclude: Vec<String>,
    /// MCP server name for MCP sources.
    pub server: String,
    /// MCP tool name for mcpTool sources.
    pub tool: String,
    /// MCP resource template for mcpResource sources.
    pub template: String,
    /// Optional per-source hit limit.
    pub max_hits: Option<usize>,
    /// Optional per-source context token budget.
    pub max_context_tokens: Option<usize>,
    /// Allows source results without explicit citation.
    pub allow_anonymous_citation: bool,
}

impl Default for RetrievalSourceConfig {
    fn default() -> Self {
        Self {
            kind: RetrievalSourceKind::Memory,
            enabled: true,
            include: Vec::new(),
            exclude: Vec::new(),
            server: String::new(),
            tool: String::new(),
            template: String::new(),
            max_hits: None,
            max_context_tokens: None,
            allow_anonymous_citation: false,
        }
    }
}

impl RetrievalSourceConfig {
    fn validate(&self, name: &str) -> ConfigResult<()> {
        if self.max_hits == Some(0) {
            return Err(ConfigError::invalid(format!(
                "retrieval source {name}: max_hits must be positive"
            )));
        }
        if self.max_context_tokens == Some(0) {
            return Err(ConfigError::invalid(format!(
                "retrieval source {name}: max_context_tokens must be positive"
            )));
        }
        match self.kind {
            RetrievalSourceKind::Memory | RetrievalSourceKind::Workspace => {}
            RetrievalSourceKind::McpTool => {
                if self.server.trim().is_empty() {
                    return Err(ConfigError::invalid(format!(
                        "retrieval source {name}: mcpTool requires server"
                    )));
                }
                if self.tool.trim().is_empty() {
                    return Err(ConfigError::invalid(format!(
                        "retrieval source {name}: mcpTool requires tool"
                    )));
                }
            }
            RetrievalSourceKind::McpResource => {
                if self.server.trim().is_empty() {
                    return Err(ConfigError::invalid(format!(
                        "retrieval source {name}: mcpResource requires server"
                    )));
                }
                if self.template.trim().is_empty() {
                    return Err(ConfigError::invalid(format!(
                        "retrieval source {name}: mcpResource requires template"
                    )));
                }
            }
        }
        Ok(())
    }
}

/// Web tool configuration including proxy and search settings.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct WebToolsConfig {
    /// Optional HTTP proxy URL.
    pub proxy: Option<String>,
    /// Web search configuration.
    pub search: WebSearchConfig,
}

impl WebToolsConfig {
    /// Validates web tools configuration.
    pub fn validate(&self) -> ConfigResult<()> {
        self.search.validate()?;
        Ok(())
    }
}

/// Web search configuration (API key and result limits).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WebSearchConfig {
    /// API key for search provider.
    pub api_key: String,
    /// Maximum results to return.
    pub max_results: usize,
}

impl Default for WebSearchConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            max_results: 5,
        }
    }
}

impl WebSearchConfig {
    /// Validates web search configuration.
    pub fn validate(&self) -> ConfigResult<()> {
        if self.max_results == 0 {
            return Err(ConfigError::invalid(
                "web search max_results must be positive",
            ));
        }
        Ok(())
    }
}

/// Exec tool configuration (timeouts and PATH adjustments).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ExecToolConfig {
    /// Execution timeout in seconds.
    pub timeout: u64,
    /// PATH suffix to add for exec tool.
    pub path_append: String,
    /// Disable dangerous-command pattern checks for exec.
    pub disable_safety_guard: bool,
    /// Disable all exec guards, including workspace/path checks.
    pub disable_all_guards: bool,
}

impl Default for ExecToolConfig {
    fn default() -> Self {
        Self {
            timeout: 60,
            path_append: String::new(),
            disable_safety_guard: false,
            disable_all_guards: false,
        }
    }
}

impl ExecToolConfig {
    /// Validates exec tool configuration.
    pub fn validate(&self) -> ConfigResult<()> {
        if self.timeout == 0 {
            return Err(ConfigError::invalid("exec timeout must be positive"));
        }
        Ok(())
    }
}

/// MCP server configuration for dynamic tool hosting.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MCPServerConfig {
    /// Command to launch MCP server (stdio).
    pub command: String,
    /// Arguments for MCP server command.
    pub args: Vec<String>,
    /// Environment variables for MCP server process.
    pub env: HashMap<String, String>,
    /// HTTP URL for MCP server (streamable HTTP).
    pub url: String,
    /// Custom headers for MCP server HTTP requests.
    pub headers: HashMap<String, String>,
    /// Tool execution timeout in seconds.
    pub tool_timeout: u64,
}

impl Default for MCPServerConfig {
    fn default() -> Self {
        Self {
            command: String::new(),
            args: Vec::new(),
            env: HashMap::new(),
            url: String::new(),
            headers: HashMap::new(),
            tool_timeout: 30,
        }
    }
}

impl MCPServerConfig {
    /// Validates MCP server configuration.
    pub fn validate(&self) -> ConfigResult<()> {
        // Either command or url must be specified
        let has_command = !self.command.trim().is_empty();
        let has_url = !self.url.trim().is_empty();

        if !has_command && !has_url {
            return Err(ConfigError::invalid(
                "MCP server must specify either 'command' or 'url'",
            ));
        }

        if self.tool_timeout == 0 {
            return Err(ConfigError::invalid(
                "MCP server tool_timeout must be positive",
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forced_provider_wins_and_normalizes_name() {
        let mut cfg = Config::default();
        cfg.agents.defaults.provider = "github-copilot".to_string();
        let name = cfg.get_provider_name(Some("openai/gpt-4o"));
        assert_eq!(name.as_deref(), Some("github_copilot"));
    }

    #[test]
    fn forced_provider_accepts_camel_case_alias() {
        let mut cfg = Config::default();
        cfg.agents.defaults.provider = "githubCopilot".to_string();

        let name = cfg.get_provider_name(Some("gpt-5.4"));
        assert_eq!(name.as_deref(), Some("github_copilot"));
    }

    #[test]
    fn provider_config_accepts_camel_case_alias() {
        let cfg = Config::default();
        assert!(cfg.provider_config("githubCopilot").is_some());
    }

    #[test]
    fn providers_config_deserializes_kebab_case_aliases() {
        let cfg: Config = serde_json::from_str(
            r#"{
                "providers": {
                    "github-copilot": {
                        "apiKey": "token-1"
                    }
                }
            }"#,
        )
        .expect("deserialize config");

        assert_eq!(
            cfg.providers
                .get("github_copilot")
                .map(|p| p.api_key.as_str()),
            Some("token-1")
        );
    }

    #[test]
    fn providers_config_deserializes_camel_case_aliases() {
        let cfg: Config = serde_json::from_str(
            r#"{
                "providers": {
                    "githubCopilot": {
                        "apiKey": "token-1"
                    }
                }
            }"#,
        )
        .expect("deserialize config");

        assert_eq!(
            cfg.providers
                .get("github_copilot")
                .map(|p| p.api_key.as_str()),
            Some("token-1")
        );
    }

    #[test]
    fn auto_provider_selects_configured_key_provider() {
        let mut cfg = Config::default();
        cfg.agents.defaults.provider = "auto".to_string();
        cfg.providers.insert(
            "openai",
            ProviderConfig {
                api_key: "key_xxx".to_string(),
                ..ProviderConfig::default()
            },
        );

        let name = cfg.get_provider_name(Some("gpt-4"));
        assert_eq!(name.as_deref(), Some("openai"));
    }

    #[test]
    fn explicit_provider_prefix_wins_without_auth() {
        let cfg = Config::default();

        let name = cfg.get_provider_name(Some("anthropic/claude-opus-4-5"));
        assert_eq!(name.as_deref(), Some("anthropic"));
    }

    #[test]
    fn get_api_base_returns_none_for_builtin_providers() {
        let mut cfg = Config::default();
        cfg.providers.insert(
            "openai",
            ProviderConfig {
                api_key: "key_xxx".to_string(),
                ..ProviderConfig::default()
            },
        );

        let base = cfg.get_api_base(Some("gpt-4"));
        assert_eq!(base, None);
    }

    #[test]
    fn get_api_base_returns_configured_value_only() {
        let mut cfg = Config::default();
        cfg.providers.insert(
            "anthropic",
            ProviderConfig {
                api_key: "sk-ant-xxx".to_string(),
                api_base: Some("https://anthropic.example/v1".to_string()),
                ..ProviderConfig::default()
            },
        );

        let base = cfg.get_api_base(Some("anthropic/claude-opus-4-5"));
        assert_eq!(base.as_deref(), Some("https://anthropic.example/v1"));
    }

    #[test]
    fn providers_config_supports_arbitrary_provider_keys() {
        let cfg: Config = serde_json::from_str(
            r#"{
                "providers": {
                    "myVendor": {
                        "providerType": "openai",
                        "apiKey": "k"
                    }
                }
            }"#,
        )
        .expect("deserialize config");
        assert!(cfg.provider_config("myVendor").is_some());
        assert_eq!(
            cfg.provider_type("myVendor"),
            ProviderType::OpenAiCompatible
        );
    }

    #[test]
    fn provider_type_accepts_openai_alias() {
        let cfg: Config = serde_json::from_str(
            r#"{
                "providers": {
                    "myVendor": {
                        "providerType": "openai",
                        "apiKey": "k"
                    }
                }
            }"#,
        )
        .expect("deserialize config");

        assert_eq!(
            cfg.provider_type("myVendor"),
            ProviderType::OpenAiCompatible
        );
    }

    #[test]
    fn provider_type_accepts_oauth_alias() {
        let cfg: Config = serde_json::from_str(
            r#"{
                "providers": {
                    "myVendor": {
                        "providerType": "oauth",
                        "apiKey": "k"
                    }
                }
            }"#,
        )
        .expect("deserialize config");

        assert_eq!(cfg.provider_type("myVendor"), ProviderType::OAuth);
    }

    #[test]
    fn provider_type_defaults_from_builtin_provider_specs() {
        let cfg = Config::default();
        assert_eq!(cfg.provider_type("anthropic"), ProviderType::Anthropic);
        assert_eq!(cfg.provider_type("openai"), ProviderType::OpenAiCompatible);
        assert_eq!(cfg.provider_type("github_copilot"), ProviderType::OAuth);
    }

    #[test]
    fn wire_api_accepts_chat_completions_alias() {
        let cfg: Config = serde_json::from_str(
            r#"{
                "providers": {
                    "myVendor": {
                        "wireApi": "chat",
                        "apiKey": "k"
                    }
                }
            }"#,
        )
        .expect("deserialize config");

        assert_eq!(cfg.wire_api("myVendor"), ProviderWireApi::ChatCompletions);
    }

    #[test]
    fn active_model_prefers_provider_model() {
        let cfg: Config = serde_json::from_str(
            r#"{
                "agents": {
                    "defaults": {
                        "provider": "xinx",
                        "model": "anthropic/claude-sonnet-4-5"
                    }
                },
                "providers": {
                    "xinx": {
                        "providerType": "openai",
                        "model": "gpt-5.4",
                        "apiKey": "k"
                    }
                }
            }"#,
        )
        .expect("deserialize config");

        assert_eq!(cfg.active_model().as_deref(), Some("gpt-5.4"));
    }

    #[test]
    fn config_validation_succeeds_with_defaults() {
        let cfg = Config::default();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn agent_defaults_validation_rejects_invalid_max_tokens() {
        let defaults = AgentDefaults {
            max_tokens: 0,
            ..AgentDefaults::default()
        };
        assert!(defaults.validate().is_err());

        let defaults = AgentDefaults {
            max_tokens: -100,
            ..AgentDefaults::default()
        };
        assert!(defaults.validate().is_err());
    }

    #[test]
    fn agent_defaults_validation_rejects_invalid_temperature() {
        let defaults = AgentDefaults {
            temperature: -0.1,
            ..AgentDefaults::default()
        };
        assert!(defaults.validate().is_err());

        let defaults = AgentDefaults {
            temperature: 2.1,
            ..AgentDefaults::default()
        };
        assert!(defaults.validate().is_err());
    }

    #[test]
    fn agent_defaults_validation_rejects_zero_iterations() {
        let defaults = AgentDefaults {
            max_tool_iterations: 0,
            ..AgentDefaults::default()
        };
        assert!(defaults.validate().is_err());
    }

    #[test]
    fn agent_defaults_validation_rejects_zero_memory_window() {
        let defaults = AgentDefaults {
            memory_window: 0,
            ..AgentDefaults::default()
        };
        assert!(defaults.validate().is_err());
    }

    #[test]
    fn agent_defaults_validation_rejects_zero_consolidation_keep_recent() {
        let defaults = AgentDefaults {
            consolidation_keep_recent: 0,
            ..AgentDefaults::default()
        };
        assert!(defaults.validate().is_err());
    }

    #[test]
    fn agent_defaults_validation_rejects_consolidation_keep_recent_greater_than_memory_window() {
        let defaults = AgentDefaults {
            memory_window: 5,
            consolidation_keep_recent: 6,
            ..AgentDefaults::default()
        };
        assert!(defaults.validate().is_err());
    }

    #[test]
    fn agent_defaults_validation_rejects_zero_consolidation_min_messages() {
        let defaults = AgentDefaults {
            consolidation_min_messages: 0,
            ..AgentDefaults::default()
        };
        assert!(defaults.validate().is_err());
    }

    #[test]
    fn agent_defaults_validation_rejects_non_positive_consolidation_summary_max_tokens() {
        let defaults = AgentDefaults {
            consolidation_summary_max_tokens: 0,
            ..AgentDefaults::default()
        };
        assert!(defaults.validate().is_err());
    }

    #[test]
    fn agent_overrides_validation_rejects_invalid_memory_window() {
        let defaults = AgentDefaults::default();
        let overrides = AgentRuntimeOverrides {
            memory_window: Some(0),
            ..AgentRuntimeOverrides::default()
        };
        assert!(
            defaults
                .validate_overrides(&overrides, "channels.instances.test.agentOverrides")
                .is_err()
        );
    }

    #[test]
    fn agent_overrides_validation_rejects_invalid_consolidation_window() {
        let defaults = AgentDefaults::default();
        let overrides = AgentRuntimeOverrides {
            memory_window: Some(10),
            consolidation_keep_recent: Some(11),
            ..AgentRuntimeOverrides::default()
        };
        assert!(
            defaults
                .validate_overrides(&overrides, "channels.instances.test.agentOverrides")
                .is_err()
        );
    }

    #[test]
    fn agent_defaults_validation_rejects_empty_workspace() {
        let defaults = AgentDefaults {
            workspace: "".to_string(),
            ..AgentDefaults::default()
        };
        assert!(defaults.validate().is_err());

        let defaults = AgentDefaults {
            workspace: "   ".to_string(),
            ..AgentDefaults::default()
        };
        assert!(defaults.validate().is_err());
    }

    #[test]
    fn agent_defaults_validation_rejects_empty_model() {
        let defaults = AgentDefaults {
            model: "".to_string(),
            ..AgentDefaults::default()
        };
        assert!(defaults.validate().is_err());
    }

    #[test]
    fn gateway_validation_allows_zero_port_when_endpoint_unused() {
        let gateway = GatewayConfig {
            port: 0,
            ..GatewayConfig::default()
        };
        assert!(gateway.validate().is_ok());
    }

    #[test]
    fn gateway_validation_allows_empty_host_when_endpoint_unused() {
        let gateway = GatewayConfig {
            host: "".to_string(),
            ..GatewayConfig::default()
        };
        assert!(gateway.validate().is_ok());
    }

    #[test]
    fn heartbeat_validation_rejects_zero_interval_when_enabled() {
        let heartbeat = HeartbeatConfig {
            enabled: true,
            interval_s: 0,
        };
        assert!(heartbeat.validate().is_err());
    }

    #[test]
    fn heartbeat_validation_allows_zero_interval_when_disabled() {
        let heartbeat = HeartbeatConfig {
            enabled: false,
            interval_s: 0,
        };
        assert!(heartbeat.validate().is_ok());
    }

    #[test]
    fn web_search_validation_rejects_zero_max_results() {
        let search = WebSearchConfig {
            max_results: 0,
            ..WebSearchConfig::default()
        };
        assert!(search.validate().is_err());
    }

    #[test]
    fn exec_tool_validation_rejects_zero_timeout() {
        let exec = ExecToolConfig {
            timeout: 0,
            ..ExecToolConfig::default()
        };
        assert!(exec.validate().is_err());
    }

    #[test]
    fn mcp_server_validation_rejects_empty_command_and_url() {
        let server = MCPServerConfig::default();
        assert!(server.validate().is_err());
    }

    #[test]
    fn mcp_server_validation_accepts_command_only() {
        let server = MCPServerConfig {
            command: "node".to_string(),
            ..MCPServerConfig::default()
        };
        assert!(server.validate().is_ok());
    }

    #[test]
    fn mcp_server_validation_accepts_url_only() {
        let server = MCPServerConfig {
            url: "http://localhost:3000".to_string(),
            ..MCPServerConfig::default()
        };
        assert!(server.validate().is_ok());
    }

    #[test]
    fn mcp_server_validation_rejects_zero_tool_timeout() {
        let server = MCPServerConfig {
            command: "node".to_string(),
            tool_timeout: 0,
            ..MCPServerConfig::default()
        };
        assert!(server.validate().is_err());
    }

    #[test]
    fn tools_config_validation_rejects_empty_mcp_server_name() {
        let mut tools = ToolsConfig::default();
        tools.mcp_servers.insert(
            "".to_string(),
            MCPServerConfig {
                command: "node".to_string(),
                ..Default::default()
            },
        );
        assert!(tools.validate().is_err());
    }
}
