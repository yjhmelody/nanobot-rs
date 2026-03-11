use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Result, bail};
use serde::{Deserialize, Deserializer, Serialize};

use crate::utils::helpers::expand_tilde;

/// Top-level configuration loaded from config files and defaults.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    pub agents: AgentsConfig,
    pub channels: ChannelsConfig,
    pub providers: ProvidersConfig,
    pub gateway: GatewayConfig,
    pub tools: ToolsConfig,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub acp: Option<crate::acp::config::ACPConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            agents: AgentsConfig::default(),
            channels: ChannelsConfig::default(),
            providers: ProvidersConfig::default(),
            gateway: GatewayConfig::default(),
            tools: ToolsConfig::default(),
            acp: None,
        }
    }
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
    /// use nanobot_rs::config::schema::Config;
    ///
    /// let config = Config::default();
    /// let workspace = config.workspace_path();
    /// assert!(workspace.is_absolute() || workspace.starts_with("~"));
    /// ```
    pub fn workspace_path(&self) -> PathBuf {
        expand_tilde(&self.agents.defaults.workspace)
            .unwrap_or_else(|_| PathBuf::from("~/.nanobot/workspace"))
    }

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
    /// - `exec.timeout` is zero
    /// - `gateway.port` is zero
    /// - `heartbeat.interval_s` is zero when enabled
    pub fn validate(&self) -> Result<()> {
        // Validate agent defaults
        self.agents.defaults.validate()?;

        // Validate tools config
        self.tools.validate()?;

        // Validate gateway config
        self.gateway.validate()?;

        Ok(())
    }

    /// Determines the provider name based on model and configuration.
    ///
    /// This method implements the provider selection logic:
    /// 1. If `agents.defaults.provider` is set and not "auto", use it
    /// 2. If model has a provider prefix (e.g., "openai/gpt-4"), use that provider
    /// 3. If model contains provider keywords (e.g., "claude" → "anthropic"), use that provider
    /// 4. Fall back to the first configured provider with an API key
    ///
    /// # Arguments
    ///
    /// * `model` - Optional model name. If None, uses `agents.defaults.model`
    ///
    /// # Returns
    ///
    /// Returns the provider name (e.g., "anthropic", "openai"), or None if no provider is configured.
    ///
    /// # Example
    ///
    /// ```
    /// use nanobot_rs::config::schema::{Config, ProviderConfig};
    ///
    /// let mut config = Config::default();
    /// config.providers.anthropic.api_key = "sk-xxx".to_string();
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
        let normalized = target_model.replace('-', "_");
        if let Some((prefix, _)) = target_model.split_once('/')
            && let Some(spec) = provider_spec(prefix)
        {
            return Some(spec.name.to_string());
        }

        for spec in PROVIDER_SPECS {
            let hit = spec
                .keywords
                .iter()
                .any(|kw| target_model.contains(kw) || normalized.contains(&kw.replace('-', "_")));
            if hit
                && (self
                    .provider_config(spec.name)
                    .map(|p| p.has_auth())
                    .unwrap_or(false)
                    || spec.oauth)
            {
                return Some(spec.name.to_string());
            }
        }

        PROVIDER_SPECS
            .iter()
            .filter(|s| !s.oauth)
            .find(|s| {
                self.provider_config(s.name)
                    .map(|p| p.has_auth())
                    .unwrap_or(false)
            })
            .map(|s| s.name.to_string())
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

    /// Returns the API base URL for the specified model's provider.
    ///
    /// If the provider has a custom `api_base` configured, returns that.
    /// Otherwise, returns the provider's default API base when one is defined.
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
        if let Some(base) = &provider.api_base {
            if !base.trim().is_empty() {
                return Some(base.clone());
            }
        }

        provider_spec(&name)
            .and_then(|spec| spec.default_api_base)
            .map(str::to_string)
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
    /// - custom, anthropic, openai, github_copilot
    pub fn provider_config(&self, name: &str) -> Option<&ProviderConfig> {
        self.providers.get(name)
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

    let collapsed = collapse_provider_name(trimmed);
    if let Some(spec) = PROVIDER_SPECS
        .iter()
        .find(|spec| spec.matches_collapsed(&collapsed))
    {
        return spec.name.to_string();
    }

    trimmed.to_lowercase().replace('-', "_")
}

fn collapse_provider_name(raw: &str) -> String {
    raw.chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}

#[derive(Debug, Clone, Copy)]
struct ProviderSpec {
    name: &'static str,
    aliases: &'static [&'static str],
    keywords: &'static [&'static str],
    oauth: bool,
    default_api_base: Option<&'static str>,
}

impl ProviderSpec {
    fn matches_collapsed(&self, candidate: &str) -> bool {
        collapse_provider_name(self.name) == candidate
            || self
                .aliases
                .iter()
                .any(|alias| collapse_provider_name(alias) == candidate)
    }
}

/// Agent-related configuration (defaults and model settings).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AgentsConfig {
    pub defaults: AgentDefaults,
}

impl Default for AgentsConfig {
    fn default() -> Self {
        Self {
            defaults: AgentDefaults::default(),
        }
    }
}

/// Default settings applied to all agents unless overridden.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AgentDefaults {
    pub workspace: String,
    pub model: String,
    pub provider: String,
    pub max_tokens: i32,
    pub temperature: f32,
    pub max_tool_iterations: usize,
    pub memory_window: usize,
    pub reasoning_effort: Option<String>,
}

impl Default for AgentDefaults {
    fn default() -> Self {
        Self {
            workspace: "~/.nanobot/workspace".to_string(),
            model: "anthropic/claude-sonnet-4-5".to_string(),
            provider: "auto".to_string(),
            max_tokens: 8192,
            temperature: 0.1,
            max_tool_iterations: 40,
            memory_window: 100,
            reasoning_effort: None,
        }
    }
}

impl AgentDefaults {
    /// Validates agent default configuration.
    pub fn validate(&self) -> Result<()> {
        if self.max_tokens <= 0 {
            bail!("max_tokens must be positive, got {}", self.max_tokens);
        }

        if !(0.0..=2.0).contains(&self.temperature) {
            bail!(
                "temperature must be in range [0.0, 2.0], got {}",
                self.temperature
            );
        }

        if self.max_tool_iterations == 0 {
            bail!("max_tool_iterations must be positive");
        }

        if self.memory_window == 0 {
            bail!("memory_window must be positive");
        }

        if self.workspace.trim().is_empty() {
            bail!("workspace path cannot be empty");
        }

        if self.model.trim().is_empty() {
            bail!("model name cannot be empty");
        }

        Ok(())
    }
}

/// Configuration for outbound channel adapters.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ChannelsConfig {
    pub send_progress: bool,
    pub send_tool_hints: bool,
    pub telegram: GenericChannelConfig,
    pub discord: GenericChannelConfig,
    pub feishu: GenericChannelConfig,
}

impl Default for ChannelsConfig {
    fn default() -> Self {
        Self {
            send_progress: true,
            send_tool_hints: false,
            telegram: GenericChannelConfig::default(),
            discord: GenericChannelConfig::default(),
            feishu: GenericChannelConfig::default(),
        }
    }
}

/// Per-channel settings shared across adapters.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GenericChannelConfig {
    pub enabled: bool,
    pub allow_from: Vec<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

impl Default for GenericChannelConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            allow_from: Vec::new(),
            extra: HashMap::new(),
        }
    }
}

/// Provider settings for a single LLM backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ProviderConfig {
    pub api_key: String,
    pub api_base: Option<String>,
    pub extra_headers: Option<HashMap<String, String>>,
    pub github_instruction: Option<String>,
}

impl ProviderConfig {
    pub fn has_auth(&self) -> bool {
        !self.api_key.trim().is_empty()
    }
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            api_base: None,
            extra_headers: None,
            github_instruction: None,
        }
    }
}

/// Collection of all configured providers.
#[derive(Debug, Clone, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ProvidersConfig {
    pub custom: ProviderConfig,
    pub anthropic: ProviderConfig,
    pub openai: ProviderConfig,
    pub github_copilot: ProviderConfig,
}

impl Default for ProvidersConfig {
    fn default() -> Self {
        Self {
            custom: ProviderConfig::default(),
            anthropic: ProviderConfig::default(),
            openai: ProviderConfig::default(),
            github_copilot: ProviderConfig::default(),
        }
    }
}

impl ProvidersConfig {
    fn get(&self, name: &str) -> Option<&ProviderConfig> {
        match normalize_provider_name(name).as_str() {
            "custom" => Some(&self.custom),
            "anthropic" => Some(&self.anthropic),
            "openai" => Some(&self.openai),
            "github_copilot" => Some(&self.github_copilot),
            _ => None,
        }
    }

    fn get_mut(&mut self, name: &str) -> Option<&mut ProviderConfig> {
        match normalize_provider_name(name).as_str() {
            "custom" => Some(&mut self.custom),
            "anthropic" => Some(&mut self.anthropic),
            "openai" => Some(&mut self.openai),
            "github_copilot" => Some(&mut self.github_copilot),
            _ => None,
        }
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
            if let Some(slot) = providers.get_mut(&key) {
                *slot = value;
            }
        }

        Ok(providers)
    }
}

const PROVIDER_SPECS: &[ProviderSpec] = &[
    ProviderSpec {
        name: "custom",
        aliases: &[],
        keywords: &[],
        oauth: false,
        default_api_base: None,
    },
    ProviderSpec {
        name: "anthropic",
        aliases: &[],
        keywords: &["anthropic", "claude"],
        oauth: false,
        default_api_base: Some("https://api.anthropic.com/v1"),
    },
    ProviderSpec {
        name: "openai",
        aliases: &[],
        keywords: &["openai", "gpt"],
        oauth: false,
        default_api_base: None,
    },
    ProviderSpec {
        name: "github_copilot",
        aliases: &["github-copilot", "githubCopilot", "copilot"],
        keywords: &["github_copilot", "copilot"],
        oauth: true,
        default_api_base: None,
    },
];

fn provider_spec(name: &str) -> Option<&'static ProviderSpec> {
    let collapsed = collapse_provider_name(name);
    if collapsed.is_empty() {
        return None;
    }

    PROVIDER_SPECS
        .iter()
        .find(|spec| spec.matches_collapsed(&collapsed))
}

/// Gateway server configuration (host/port/heartbeat).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GatewayConfig {
    pub host: String,
    pub port: u16,
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
    pub fn validate(&self) -> Result<()> {
        if self.port == 0 {
            bail!("gateway port cannot be zero");
        }

        if self.host.trim().is_empty() {
            bail!("gateway host cannot be empty");
        }

        self.heartbeat.validate()?;

        Ok(())
    }
}

/// Heartbeat polling configuration for periodic tasks.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct HeartbeatConfig {
    pub enabled: bool,
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
    pub fn validate(&self) -> Result<()> {
        if self.enabled && self.interval_s == 0 {
            bail!("heartbeat interval_s cannot be zero when enabled");
        }

        Ok(())
    }
}

/// Configuration for tool subsystems (web, exec, MCP).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ToolsConfig {
    pub web: WebToolsConfig,
    pub exec: ExecToolConfig,
    pub restrict_to_workspace: bool,
    pub mcp_servers: HashMap<String, MCPServerConfig>,
}

impl Default for ToolsConfig {
    fn default() -> Self {
        Self {
            web: WebToolsConfig::default(),
            exec: ExecToolConfig::default(),
            restrict_to_workspace: false,
            mcp_servers: HashMap::new(),
        }
    }
}

impl ToolsConfig {
    /// Validates tools configuration.
    pub fn validate(&self) -> Result<()> {
        self.web.validate()?;
        self.exec.validate()?;

        // Validate MCP servers
        for (name, server) in &self.mcp_servers {
            if name.trim().is_empty() {
                bail!("MCP server name cannot be empty");
            }
            server.validate()?;
        }

        Ok(())
    }
}

/// Web tool configuration including proxy and search settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WebToolsConfig {
    pub proxy: Option<String>,
    pub search: WebSearchConfig,
}

impl Default for WebToolsConfig {
    fn default() -> Self {
        Self {
            proxy: None,
            search: WebSearchConfig::default(),
        }
    }
}

impl WebToolsConfig {
    /// Validates web tools configuration.
    pub fn validate(&self) -> Result<()> {
        self.search.validate()?;
        Ok(())
    }
}

/// Web search configuration (API key and result limits).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WebSearchConfig {
    pub api_key: String,
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
    pub fn validate(&self) -> Result<()> {
        if self.max_results == 0 {
            bail!("web search max_results must be positive");
        }
        Ok(())
    }
}

/// Exec tool configuration (timeouts and PATH adjustments).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ExecToolConfig {
    pub timeout: u64,
    pub path_append: String,
}

impl Default for ExecToolConfig {
    fn default() -> Self {
        Self {
            timeout: 60,
            path_append: String::new(),
        }
    }
}

impl ExecToolConfig {
    /// Validates exec tool configuration.
    pub fn validate(&self) -> Result<()> {
        if self.timeout == 0 {
            bail!("exec timeout must be positive");
        }
        Ok(())
    }
}

/// MCP server configuration for dynamic tool hosting.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MCPServerConfig {
    pub command: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub url: String,
    pub headers: HashMap<String, String>,
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
    pub fn validate(&self) -> Result<()> {
        // Either command or url must be specified
        let has_command = !self.command.trim().is_empty();
        let has_url = !self.url.trim().is_empty();

        if !has_command && !has_url {
            bail!("MCP server must specify either 'command' or 'url'");
        }

        if self.tool_timeout == 0 {
            bail!("MCP server tool_timeout must be positive");
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

        assert_eq!(cfg.providers.github_copilot.api_key, "token-1");
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

        assert_eq!(cfg.providers.github_copilot.api_key, "token-1");
    }

    #[test]
    fn auto_provider_selects_configured_key_provider() {
        let mut cfg = Config::default();
        cfg.agents.defaults.provider = "auto".to_string();
        cfg.providers.openai.api_key = "key_xxx".to_string();

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
        cfg.providers.openai.api_key = "key_xxx".to_string();

        let base = cfg.get_api_base(Some("gpt-4"));
        assert_eq!(base, None);
    }

    #[test]
    fn get_api_base_returns_default_for_anthropic() {
        let mut cfg = Config::default();
        cfg.providers.anthropic.api_key = "sk-ant-xxx".to_string();

        let base = cfg.get_api_base(Some("anthropic/claude-opus-4-5"));
        assert_eq!(base.as_deref(), Some("https://api.anthropic.com/v1"));
    }

    #[test]
    fn config_validation_succeeds_with_defaults() {
        let cfg = Config::default();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn agent_defaults_validation_rejects_invalid_max_tokens() {
        let mut defaults = AgentDefaults::default();
        defaults.max_tokens = 0;
        assert!(defaults.validate().is_err());

        defaults.max_tokens = -100;
        assert!(defaults.validate().is_err());
    }

    #[test]
    fn agent_defaults_validation_rejects_invalid_temperature() {
        let mut defaults = AgentDefaults::default();
        defaults.temperature = -0.1;
        assert!(defaults.validate().is_err());

        defaults.temperature = 2.1;
        assert!(defaults.validate().is_err());
    }

    #[test]
    fn agent_defaults_validation_rejects_zero_iterations() {
        let mut defaults = AgentDefaults::default();
        defaults.max_tool_iterations = 0;
        assert!(defaults.validate().is_err());
    }

    #[test]
    fn agent_defaults_validation_rejects_zero_memory_window() {
        let mut defaults = AgentDefaults::default();
        defaults.memory_window = 0;
        assert!(defaults.validate().is_err());
    }

    #[test]
    fn agent_defaults_validation_rejects_empty_workspace() {
        let mut defaults = AgentDefaults::default();
        defaults.workspace = "".to_string();
        assert!(defaults.validate().is_err());

        defaults.workspace = "   ".to_string();
        assert!(defaults.validate().is_err());
    }

    #[test]
    fn agent_defaults_validation_rejects_empty_model() {
        let mut defaults = AgentDefaults::default();
        defaults.model = "".to_string();
        assert!(defaults.validate().is_err());
    }

    #[test]
    fn gateway_validation_rejects_zero_port() {
        let mut gateway = GatewayConfig::default();
        gateway.port = 0;
        assert!(gateway.validate().is_err());
    }

    #[test]
    fn gateway_validation_rejects_empty_host() {
        let mut gateway = GatewayConfig::default();
        gateway.host = "".to_string();
        assert!(gateway.validate().is_err());
    }

    #[test]
    fn heartbeat_validation_rejects_zero_interval_when_enabled() {
        let mut heartbeat = HeartbeatConfig::default();
        heartbeat.enabled = true;
        heartbeat.interval_s = 0;
        assert!(heartbeat.validate().is_err());
    }

    #[test]
    fn heartbeat_validation_allows_zero_interval_when_disabled() {
        let mut heartbeat = HeartbeatConfig::default();
        heartbeat.enabled = false;
        heartbeat.interval_s = 0;
        assert!(heartbeat.validate().is_ok());
    }

    #[test]
    fn web_search_validation_rejects_zero_max_results() {
        let mut search = WebSearchConfig::default();
        search.max_results = 0;
        assert!(search.validate().is_err());
    }

    #[test]
    fn exec_tool_validation_rejects_zero_timeout() {
        let mut exec = ExecToolConfig::default();
        exec.timeout = 0;
        assert!(exec.validate().is_err());
    }

    #[test]
    fn mcp_server_validation_rejects_empty_command_and_url() {
        let server = MCPServerConfig::default();
        assert!(server.validate().is_err());
    }

    #[test]
    fn mcp_server_validation_accepts_command_only() {
        let mut server = MCPServerConfig::default();
        server.command = "node".to_string();
        assert!(server.validate().is_ok());
    }

    #[test]
    fn mcp_server_validation_accepts_url_only() {
        let mut server = MCPServerConfig::default();
        server.url = "http://localhost:3000".to_string();
        assert!(server.validate().is_ok());
    }

    #[test]
    fn mcp_server_validation_rejects_zero_tool_timeout() {
        let mut server = MCPServerConfig::default();
        server.command = "node".to_string();
        server.tool_timeout = 0;
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
