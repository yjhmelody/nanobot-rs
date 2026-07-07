//! CLI argument parsing and subcommand dispatch.
//!
//! This module defines the command-line interface using `clap` and implements
//! the handlers for each subcommand:
//!
//! | Subcommand | Purpose |
//! |------------|---------|
//! | `onboard` | Initialise or refresh config and workspace templates |
//! | `agent` | Run the agent in interactive (REPL) or one-shot (`--message`) mode |
//! | `gateway` | Start the gateway service with channels, cron, and heartbeat |
//! | `status` | Show paths, workspace, and provider configuration |
//! | `provider` | Manage provider credentials (login, status) |
//! | `retrieval` | Validate retrieval golden-case JSONL datasets |
//!
//! ## Agent Mode
//!
//! In interactive mode, the CLI spawns three concurrent tasks:
//! - Agent loop (processes inbound messages)
//! - Output listener (prints outbound messages matching the current session)
//! - Input reader (reads stdin and publishes `InboundMessage` to the bus)
//!
//! ## Gateway Mode
//!
//! In gateway mode, the CLI additionally starts:
//! - `ChannelManager` for external messaging channels (Feishu, Slack, etc.)
//! - `CronService` for scheduled jobs
//! - `HeartbeatService` for periodic task review
//!
//! Wiring between these services is done via handler callbacks
//! (`GatewayCronJobHandler`, `GatewayHeartbeatExecuteHandler`,
//! `GatewayHeartbeatNotifyHandler`).

use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;

use async_trait::async_trait;
use clap::{Args, Parser, Subcommand};
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command as TokioCommand;

use crate::acp::ACPConfig;
use crate::error::{NanobotError, NanobotResult};
use crate::heartbeat::{
    HeartbeatError, HeartbeatExecuteHandler, HeartbeatNotifyHandler, HeartbeatResult,
};
use crate::runtime::app::build_runtime;
use crate::runtime::error::RuntimeError;
use crate::utils::helpers::{get_workspace_path, sync_workspace_templates};
use nanobot_agent::AgentLoop;
use nanobot_bus::MessageBus;
use nanobot_bus::{InboundMessage, MessageMetadata, OutboundMessage};
use nanobot_channels::ChannelManager;
use nanobot_config::{
    Config, ConfigResult, get_config_path, load_config, normalize_provider_name, save_config,
};
use nanobot_cron::{CronJob, CronJobHandler, CronResult};
use nanobot_types::SessionKey;

/// Top-level CLI argument parser.
///
/// # Fields
///
/// * `config` — Optional path to a non-default config file.
/// * `command` — The subcommand to execute.
#[derive(Debug, Parser)]
#[command(
    name = "nanobot",
    about = "nanobot command-line interface",
    long_about = "nanobot command-line interface for onboarding, running the agent, and managing providers."
)]
pub struct Cli {
    /// Path to config file (default: `~/.nanobot/config.json`).
    #[arg(long, global = true, help = "Path to config file.")]
    pub config: Option<PathBuf>,
    #[command(subcommand)]
    pub command: Commands,
}

/// Top-level CLI subcommands.
#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Initialize or refresh config and workspace templates.
    #[command(
        long_about = "Create or refresh ~/.nanobot/config.json and ensure workspace templates are present. Use --overwrite to reset to defaults."
    )]
    Onboard(OnboardArgs),
    /// Run the agent in interactive or one-shot mode.
    #[command(
        long_about = "Start an interactive session by default, or run a single prompt with --message."
    )]
    Agent(AgentArgs),
    /// Run the gateway service.
    #[command(
        long_about = "Start the gateway service with the configured channels and heartbeat loop."
    )]
    Gateway(GatewayArgs),
    /// Show status of config, workspace, and providers.
    #[command(long_about = "Print paths and availability checks for config and workspace.")]
    Status,
    /// Manage provider configuration and connectivity checks.
    #[command(long_about = "Login to a provider or show provider auth status.")]
    Provider(ProviderArgs),
    /// Evaluate retrieval golden-case fixtures.
    #[command(
        long_about = "Validate retrieval golden-case JSONL fixtures and print a compact summary."
    )]
    Retrieval(RetrievalArgs),
}

/// Arguments for the `onboard` subcommand.
#[derive(Debug, Args)]
pub struct OnboardArgs {
    /// Overwrite existing config with defaults (otherwise preserves existing values).
    #[arg(long, help = "Overwrite existing config with defaults.")]
    pub overwrite: bool,
}

/// Arguments for the `agent` subcommand.
#[derive(Debug, Args)]
pub struct AgentArgs {
    /// Optional single message to process (one-shot mode). If absent, enters interactive mode.
    #[arg(long, short, help = "Send a single message and exit.")]
    pub message: Option<String>,
    /// Session key in `channel:chat_id` format. Defaults to `"cli:direct"`.
    #[arg(
        long,
        short,
        default_value = "cli:direct",
        help = "Session key in channel:chat_id format."
    )]
    pub session: String,
}

/// Arguments for the `gateway` subcommand.
#[derive(Debug, Args)]
pub struct GatewayArgs {
    // TODO: gateway endpoint is not enabled yet; keep for future compatibility.
    /// Reserved gateway port argument (currently unused).
    #[arg(
        long = "port",
        short,
        default_value_t = 18790,
        hide = true,
        help = "Reserved gateway port argument (currently unused)."
    )]
    pub _port: u16,
}

/// Arguments for the `provider` subcommand (contains sub-subcommands).
#[derive(Debug, Args)]
pub struct ProviderArgs {
    #[command(subcommand)]
    pub command: ProviderCommands,
}

/// Arguments for the `retrieval` subcommand (contains sub-subcommands).
#[derive(Debug, Args)]
pub struct RetrievalArgs {
    #[command(subcommand)]
    pub command: RetrievalCommands,
}

/// Sub-subcommands for retrieval evaluation.
#[derive(Debug, Subcommand)]
pub enum RetrievalCommands {
    /// Validate a retrieval golden-case JSONL dataset.
    Eval(RetrievalEvalArgs),
}

/// Arguments for `retrieval eval`.
#[derive(Debug, Args)]
pub struct RetrievalEvalArgs {
    /// Path to `golden_cases.jsonl`.
    #[arg(long, help = "Path to golden_cases.jsonl.")]
    pub dataset: PathBuf,
}

/// A single golden case from the retrieval evaluation dataset.
///
/// Fields use `camelCase` serialisation to match the JSONL file convention.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RetrievalGoldenCase {
    /// The query text (must be non-empty).
    query: String,
    /// Expected source documents that should be retrieved.
    #[serde(default)]
    expected_sources: Vec<String>,
    /// Expected terms that should appear in the response.
    #[serde(default)]
    expected_terms: Vec<String>,
    /// Expected citations that should be included.
    #[serde(default)]
    expected_citations: Vec<String>,
}

/// Sub-subcommands for provider management.
#[derive(Debug, Subcommand)]
pub enum ProviderCommands {
    /// Store provider credentials.
    #[command(
        long_about = "Write provider API key into the config file for the selected provider."
    )]
    Login(ProviderLoginArgs),
    /// Show provider auth status.
    #[command(long_about = "Check whether the selected provider has credentials configured.")]
    Status(ProviderStatusArgs),
}

/// Arguments for `provider login`.
#[derive(Debug, Args)]
pub struct ProviderLoginArgs {
    /// Provider name (e.g., `anthropic`, `openai`, `custom`).
    #[arg(help = "Provider name (e.g., anthropic, openai, custom).")]
    pub provider: String,
    /// Optional API host override.
    #[arg(long, help = "Optional API host override.")]
    pub host: Option<String>,
    /// Config directory override.
    #[arg(long = "config-dir", help = "Config directory override.")]
    pub config_dir: Option<PathBuf>,
}

/// Arguments for `provider status`.
#[derive(Debug, Args)]
pub struct ProviderStatusArgs {
    /// Provider name (e.g., `anthropic`, `openai`, `custom`).
    #[arg(help = "Provider name (e.g., anthropic, openai, custom).")]
    pub provider: String,
    /// Config directory override.
    #[arg(long = "config-dir", help = "Config directory override.")]
    pub config_dir: Option<PathBuf>,
}

/// Dispatch the CLI subcommand to its handler.
///
/// Parses the `Cli` struct and calls the appropriate handler function based
/// on the selected subcommand.
///
/// # Errors
///
/// Returns a `NanobotError` if the subcommand handler fails.
pub async fn run(cli: Cli) -> NanobotResult<()> {
    let config_path = cli.config.clone();
    match cli.command {
        Commands::Onboard(args) => onboard(args, config_path).await,
        Commands::Agent(args) => agent(args, config_path.clone()).await,
        Commands::Gateway(args) => gateway(args, config_path).await,
        Commands::Status => status(config_path).await,
        Commands::Provider(args) => provider(args, config_path.clone()).await,
        Commands::Retrieval(args) => retrieval(args).await,
    }
}

async fn retrieval(args: RetrievalArgs) -> NanobotResult<()> {
    match args.command {
        RetrievalCommands::Eval(args) => retrieval_eval(args).await,
    }
}

/// Validate a retrieval golden-case JSONL dataset, printing a summary report.
async fn retrieval_eval(args: RetrievalEvalArgs) -> NanobotResult<()> {
    let content = tokio::fs::read_to_string(&args.dataset)
        .await
        .map_err(|e| {
            NanobotError::Other(anyhow::anyhow!(
                "failed to read dataset {}: {}",
                args.dataset.display(),
                e
            ))
        })?;
    let mut cases = Vec::new();
    for (idx, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let case: RetrievalGoldenCase = serde_json::from_str(trimmed).map_err(|e| {
            NanobotError::Other(anyhow::anyhow!(
                "invalid JSONL at {}:{}: {}",
                args.dataset.display(),
                idx + 1,
                e
            ))
        })?;
        if case.query.trim().is_empty() {
            return Err(NanobotError::Other(anyhow::anyhow!(
                "invalid golden case at {}:{}: query cannot be empty",
                args.dataset.display(),
                idx + 1
            )));
        }
        cases.push(case);
    }

    let source_expectations = cases
        .iter()
        .filter(|case| !case.expected_sources.is_empty())
        .count();
    let term_expectations = cases
        .iter()
        .filter(|case| !case.expected_terms.is_empty())
        .count();
    let citation_expectations = cases
        .iter()
        .filter(|case| !case.expected_citations.is_empty())
        .count();

    println!("Retrieval golden cases: {}", cases.len());
    println!("  with expected sources: {source_expectations}");
    println!("  with expected terms: {term_expectations}");
    println!("  with expected citations: {citation_expectations}");
    Ok(())
}

/// Resolve the config file path: use the explicit one or get the default.
fn resolve_config_path(explicit: Option<PathBuf>) -> ConfigResult<PathBuf> {
    match explicit {
        Some(p) => Ok(p),
        None => get_config_path(),
    }
}

/// Handle `onboard` subcommand: create or refresh config and templates.
async fn onboard(args: OnboardArgs, config_path: Option<PathBuf>) -> NanobotResult<()> {
    let config_path = resolve_config_path(config_path)?;

    if config_path.exists() {
        if args.overwrite {
            let cfg = Config::default();
            save_config(&cfg, Some(&config_path))?;
            println!("✓ Config reset to defaults at {}", config_path.display());
        } else {
            let cfg = load_config(Some(&config_path))?;
            save_config(&cfg, Some(&config_path))?;
            println!(
                "✓ Config refreshed at {} (existing values preserved)",
                config_path.display()
            );
        }
    } else {
        save_config(&Config::default(), Some(&config_path))?;
        println!("✓ Created config at {}", config_path.display());
    }

    let cfg = load_config(Some(&config_path))?;
    let workspace = get_workspace_path(Some(cfg.agents.defaults.workspace.as_str())).await?;
    println!("✓ Workspace at {}", workspace.display());

    let _ = sync_workspace_templates(&workspace, false).await?;

    println!("\n nanobot is ready!");
    println!("\nNext steps:");
    println!("  1. Add your API key to ~/.nanobot/config.json");
    println!("  2. Chat: nanobot agent -m \"Hello!\"");

    Ok(())
}

/// Handle `agent` subcommand: run in one-shot (`--message`) or interactive (REPL) mode.
///
/// In one-shot mode, calls `process_direct` and prints the response.
/// In interactive mode, spawns three concurrent tasks (agent loop, output
/// listener, input reader) and shuts down on Ctrl+C or `exit`/`quit`.
async fn agent(args: AgentArgs, config_path: Option<PathBuf>) -> NanobotResult<()> {
    let config = load_config(config_path.as_deref())?;
    tracing::trace!("load config: {:#?}", config);
    let workspace = get_workspace_path(Some(config.agents.defaults.workspace.as_str())).await?;
    sync_workspace_templates(&workspace, true).await?;

    let runtime = build_runtime(config).await?;

    if let Some(message) = args.message {
        let (channel, chat_id) = split_session(&args.session);
        let session_key = SessionKey::from(args.session.as_str());
        // 单轮模式：直接走 process_direct，不启动完整 inbound/outbound 循环。
        let response = runtime
            .agent
            .process_direct(&message, &session_key, &channel, &chat_id)
            .await;
        runtime.agent.close_mcp().await;
        runtime.agent.close_provider().await;
        let response = response?;
        println!("nanobot response:\n{}\n", response);
        return Ok(());
    }

    println!("Interactive mode (type exit/quit to quit)\n");
    let session_key = args.session.clone();
    let (channel, chat_id) = split_session(&args.session);
    let agent_arc = runtime.agent.clone();
    // 交互模式下，AgentLoop 在后台消费 inbound 消息；CLI 只负责输入输出桥接。
    let agent_task = tokio::spawn(async move { agent_arc.run().await });

    let mut outbound_rx = runtime.bus.subscribe_outbound();
    let output_channel = channel.clone();
    let output_chat_id = chat_id.clone();
    let output_task = tokio::spawn(async move {
        // 输出协程：只打印当前 session 的消息，避免多会话串屏。
        loop {
            match outbound_rx.recv().await {
                Ok(msg) => {
                    if matches_outbound_session(&msg, &output_channel, &output_chat_id)
                        && !msg.content.trim().is_empty()
                    {
                        println!("\n nanobot\n\n{}\n", msg.content);
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            }
        }
    });

    let bus = runtime.bus.clone();
    let input_channel = channel.clone();
    let input_chat_id = chat_id.clone();
    let input_task = tokio::spawn(async move {
        // 输入协程：把终端输入封装成 InboundMessage 后投递到总线。
        let stdin = tokio::io::stdin();
        let mut reader = BufReader::new(stdin).lines();

        loop {
            print!("You: ");
            std::io::stdout().flush().ok();

            let line = match reader.next_line().await {
                Ok(Some(line)) => line,
                Ok(None) => break,
                Err(err) => {
                    eprintln!("stdin read error: {}", err);
                    break;
                }
            };
            let input = line.trim().to_string();
            if input.is_empty() {
                continue;
            }
            if is_exit_cmd(&input) {
                break;
            }

            let msg = InboundMessage {
                channel: input_channel.clone(),
                sender_id: "user".to_string(),
                chat_id: input_chat_id.clone(),
                content: input.into(),
                timestamp: chrono::Utc::now(),
                media: Vec::new(),
                metadata: MessageMetadata::default(),
                session_key_override: Some(SessionKey::from(session_key.clone())),
            };
            let _ = bus.publish_inbound(msg);
        }
    });

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            println!("\nShutting down...");
        }
        _ = input_task => {
            println!("Goodbye!");
        }
    }

    runtime.agent.stop().await;
    let _ = agent_task.await;
    output_task.abort();
    let _ = output_task.await;
    runtime.agent.close_mcp().await;
    runtime.agent.close_provider().await;
    Ok(())
}

/// Handle `status` subcommand: print config path, workspace path, and provider info.
async fn status(config_path: Option<PathBuf>) -> NanobotResult<()> {
    let config_path = resolve_config_path(config_path)?;
    let config = load_config(Some(&config_path))?;
    let workspace = config.workspace_path();

    println!("nanobot Status\n");
    println!(
        "Config: {} {}",
        config_path.display(),
        if config_path.exists() { "✓" } else { "✗" }
    );
    println!(
        "Workspace: {} {}",
        workspace.display(),
        if workspace.exists() { "✓" } else { "✗" }
    );
    let model = config
        .active_model()
        .unwrap_or_else(|| config.agents.defaults.model.clone());
    println!("Model: {}", model);

    if let Some(name) = config.get_provider_name(None) {
        println!("Provider: {}", name);
    }

    Ok(())
}

/// Handle `provider` subcommand: dispatch to login or status.
async fn provider(args: ProviderArgs, config_path: Option<PathBuf>) -> NanobotResult<()> {
    match args.command {
        ProviderCommands::Login(args) => provider_login(args, config_path).await,
        ProviderCommands::Status(args) => provider_status(args, config_path).await,
    }
}

/// Handle `provider login`: store credentials for a supported provider.
///
/// Currently only `github_copilot` is supported for this command.
async fn provider_login(
    args: ProviderLoginArgs,
    config_path: Option<PathBuf>,
) -> NanobotResult<()> {
    let provider = normalize_provider_name(&args.provider);
    if provider.is_empty() {
        return Err(NanobotError::Runtime(RuntimeError::message(
            "provider name cannot be empty",
        )));
    }
    if provider != "github_copilot" {
        return Err(NanobotError::Runtime(RuntimeError::message(format!(
            "provider '{}' is not supported by this command",
            provider
        ))));
    }

    let config = load_config(config_path.as_deref())?;
    let command_name = copilot_command_name(&config);
    let mut command = TokioCommand::new(&command_name);
    command.stdin(Stdio::inherit());
    command.stdout(Stdio::inherit());
    command.stderr(Stdio::inherit());
    command.arg("login");

    if let Some(host) = args.host.as_deref() {
        command.arg("--host").arg(host);
    }
    if let Some(config_dir) = args.config_dir.as_ref() {
        command.arg("--config-dir").arg(config_dir);
    }

    let status = command.status().await.map_err(|e| {
        NanobotError::Runtime(RuntimeError::message(format!(
            "failed to run {}: {}",
            command_name, e
        )))
    })?;

    if !status.success() {
        return Err(NanobotError::Runtime(RuntimeError::message(format!(
            "{} login failed with exit code: {}",
            command_name,
            status.code().unwrap_or(-1)
        ))));
    }

    println!("✓ {} login successful", command_name);
    Ok(())
}

/// Handle `provider status`: check auth status for a supported provider.
///
/// Currently only `github_copilot` is supported. Prints binary path,
/// version, env token presence, and config directory status.
async fn provider_status(
    args: ProviderStatusArgs,
    _config_path: Option<PathBuf>,
) -> NanobotResult<()> {
    let provider = normalize_provider_name(&args.provider);
    if provider != "github_copilot" {
        println!(
            "Provider '{}' status check not supported by this command.",
            provider
        );
        return Ok(());
    }

    let config = load_config(None)?;
    let command_name = copilot_command_name(&config);
    let config_dir = resolve_copilot_config_dir(args.config_dir);
    let binary_path = which::which(&command_name).ok();
    let version = std::process::Command::new(&command_name)
        .arg("--version")
        .output()
        .ok();
    let env_token = std::env::var("GITHUB_TOKEN")
        .map(|v| !v.is_empty())
        .unwrap_or(false)
        || std::env::var("COPILOT_TOKEN")
            .map(|v| !v.is_empty())
            .unwrap_or(false);

    println!("Provider: github_copilot");
    println!("Command: {}", command_name);
    println!(
        "Binary: {}",
        binary_path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "not found on PATH".to_string())
    );
    println!(
        "Version: {}",
        version
            .as_ref()
            .and_then(|output| String::from_utf8(output.stdout.clone()).ok())
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| "unknown".to_string())
    );
    println!(
        "Env token: {}",
        if env_token { "present" } else { "absent" }
    );
    println!(
        "Config dir: {} {}",
        config_dir.display(),
        if config_dir.exists() { "✓" } else { "✗" }
    );
    println!(
        "Credential store: not introspected (Copilot may still be logged in via system keychain)"
    );

    Ok(())
}

/// Handle `gateway` subcommand: start the gateway service.
///
/// Initialises all services (bus, provider, agent, cron, heartbeat, channels),
/// wires them together via handler callbacks, and runs until Ctrl+C or stdin
/// EOF. The service hierarchy is:
///
/// ```text
/// Channels → MessageBus → AgentLoop
/// Cron → AgentLoop
/// Heartbeat → AgentLoop → MessageBus
/// ```
async fn gateway(_args: GatewayArgs, config_path: Option<PathBuf>) -> NanobotResult<()> {
    let config = load_config(config_path.as_deref())?;
    let workspace = get_workspace_path(Some(config.agents.defaults.workspace.as_str())).await?;
    sync_workspace_templates(&workspace, true).await?;

    let runtime = build_runtime(config).await?;
    let channels = Arc::new(ChannelManager::new(
        runtime.config.channels.clone(),
        runtime.bus.clone(),
    )?);
    println!("Starting nanobot gateway...");

    let agent = runtime.agent.clone();
    let bus = runtime.bus.clone();
    let cron = runtime.cron.clone();
    let heartbeat = runtime.heartbeat.clone();
    let enabled = Arc::new(
        channels
            .enabled_channels()
            .into_iter()
            .collect::<HashSet<_>>(),
    );
    let picker = SessionTargetPicker {
        agent: agent.clone(),
        enabled_channels: enabled,
    };

    cron.register_on_job_handler(Arc::new(GatewayCronJobHandler {
        agent: agent.clone(),
        bus: bus.clone(),
    }))
    .await;

    heartbeat
        .register_on_execute_handler(Arc::new(GatewayHeartbeatExecuteHandler {
            agent: agent.clone(),
            picker: picker.clone(),
        }))
        .await;

    heartbeat
        .register_on_notify_handler(Arc::new(GatewayHeartbeatNotifyHandler {
            bus: bus.clone(),
            picker,
        }))
        .await;

    channels.start_all().await?;
    cron.start().await?;
    heartbeat.start().await;

    let agent_arc = agent.clone();
    let agent_task = tokio::spawn(async move { agent_arc.run().await });

    let bus_for_input = bus.clone();
    let input_task = tokio::spawn(async move {
        let stdin = tokio::io::stdin();
        let mut lines = BufReader::new(stdin).lines();
        let session = "cli:gateway".to_string();
        loop {
            print!("gateway> ");
            std::io::stdout().flush().ok();
            let Some(line) = lines.next_line().await.unwrap_or(None) else {
                break;
            };
            let input = line.trim().to_string();
            if input.is_empty() {
                continue;
            }
            if is_exit_cmd(&input) {
                break;
            }
            let msg = InboundMessage {
                channel: "cli".to_string(),
                sender_id: "user".to_string(),
                chat_id: "gateway".to_string(),
                content: input.into(),
                timestamp: chrono::Utc::now(),
                media: Vec::new(),
                metadata: MessageMetadata::default(),
                session_key_override: Some(SessionKey::from(session.clone())),
            };
            let _ = bus_for_input.publish_inbound(msg);
        }
    });

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            println!("\nShutting down...");
        }
        _ = input_task => {
            println!("\nInput ended. Shutting down...");
        }
    }

    channels.stop_all().await;
    runtime.agent.stop().await;
    runtime.heartbeat.stop().await;
    runtime.cron.stop().await;

    let _ = agent_task.await;
    runtime.agent.close_mcp().await;
    runtime.agent.close_provider().await;

    Ok(())
}

/// Resolve the GitHub Copilot CLI command name from the ACP config or default to `"copilot"`.
fn copilot_command_name(config: &Config) -> String {
    config
        .acp
        .as_ref()
        .and_then(|acp| acp.agents.get("copilot"))
        .map(|agent| agent.command.clone())
        .filter(|command| !command.trim().is_empty())
        .or_else(|| {
            ACPConfig::default()
                .agents
                .get("copilot")
                .map(|agent| agent.command.clone())
        })
        .unwrap_or_else(|| "copilot".to_string())
}

/// Resolve the Copilot config directory from explicit override, `$COPILOT_HOME`,
/// or default to `~/.copilot`.
fn resolve_copilot_config_dir(explicit: Option<PathBuf>) -> PathBuf {
    if let Some(path) = explicit {
        return path;
    }
    if let Ok(home) = std::env::var("COPILOT_HOME")
        && !home.trim().is_empty()
    {
        return PathBuf::from(home);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("~"))
        .join(".copilot")
}

/// Picks an appropriate (channel, chat_id) target from active sessions for
/// delivering heartbeat or cron notifications.
///
/// Prefers the first session that belongs to an enabled channel (excluding
/// `cli` and `system` channels). Falls back to `("cli", "direct")`.
#[derive(Clone)]
struct SessionTargetPicker {
    agent: Arc<AgentLoop>,
    enabled_channels: Arc<HashSet<String>>,
}

impl SessionTargetPicker {
    /// Select the best target (channel, chat_id) for delivering a notification.
    ///
    /// Iterates active sessions and returns the first that belongs to an
    /// enabled external channel. Falls back to `("cli", "direct")`.
    async fn pick_target(&self) -> (String, String) {
        let Ok(sessions) = self.agent.sessions.list_sessions().await else {
            return ("cli".to_string(), "direct".to_string());
        };

        sessions
            .into_iter()
            .find_map(|item| {
                let (channel, chat_id) = item.key.split_once(':')?;
                if channel == "cli" || channel == "system" {
                    return None;
                }
                if self.enabled_channels.contains(channel) && !chat_id.is_empty() {
                    Some((channel.to_string(), chat_id.to_string()))
                } else {
                    None
                }
            })
            .unwrap_or_else(|| ("cli".to_string(), "direct".to_string()))
    }
}

/// Cron job handler for the gateway: forwards triggered jobs to the agent loop
/// and optionally publishes the response as an outbound message.
struct GatewayCronJobHandler {
    agent: Arc<AgentLoop>,
    bus: MessageBus,
}

#[async_trait]
impl CronJobHandler for GatewayCronJobHandler {
    async fn on_job(&self, job: CronJob) -> CronResult<Option<String>> {
        let reminder_note = format!(
            "[Scheduled Task] Timer finished.\n\nTask '{}' has been triggered.\nScheduled instruction: {}",
            job.name, job.payload.message
        );
        let session_key = SessionKey::from_string(format!("cron:{}", job.id));

        let response = self
            .agent
            .process_direct(
                &reminder_note,
                &session_key,
                job.payload.channel.as_deref().unwrap_or("cli"),
                job.payload.to.as_deref().unwrap_or("direct"),
            )
            .await
            .unwrap_or_else(|e| format!("⚠️ Error: {}", e));

        if job.payload.deliver
            && let Some(chat_id) = job.payload.to.as_deref()
            && !response.trim().is_empty()
        {
            let _ = self.bus.publish_outbound(OutboundMessage {
                channel: job.payload.channel.unwrap_or_else(|| "cli".to_string()),
                chat_id: chat_id.to_string(),
                content: response.clone(),
                reply_to: None,
                media: Vec::new(),
                metadata: MessageMetadata::default(),
            });
        }

        Ok(Some(response))
    }
}

/// Heartbeat execute handler for the gateway: runs tasks through the agent loop.
#[derive(Clone)]
struct GatewayHeartbeatExecuteHandler {
    agent: Arc<AgentLoop>,
    picker: SessionTargetPicker,
}

#[async_trait]
impl HeartbeatExecuteHandler for GatewayHeartbeatExecuteHandler {
    async fn on_execute(&self, tasks: String) -> HeartbeatResult<String> {
        let (channel, chat_id) = self.picker.pick_target().await;
        let session_key = SessionKey::from("heartbeat");
        self.agent
            .process_direct(&tasks, &session_key, &channel, &chat_id)
            .await
            .map_err(|e| HeartbeatError::execution(e.to_string()))
    }
}

/// Heartbeat notify handler for the gateway: publishes results as outbound messages.
#[derive(Clone)]
struct GatewayHeartbeatNotifyHandler {
    bus: MessageBus,
    picker: SessionTargetPicker,
}

#[async_trait]
impl HeartbeatNotifyHandler for GatewayHeartbeatNotifyHandler {
    async fn on_notify(&self, response: String) {
        let (channel, chat_id) = self.picker.pick_target().await;
        if channel == "cli" {
            return;
        }

        let _ = self.bus.publish_outbound(OutboundMessage {
            channel,
            chat_id,
            content: response,
            reply_to: None,
            media: Vec::new(),
            metadata: MessageMetadata::default(),
        });
    }
}

/// Split a `channel:chat_id` session string into its components.
///
/// If there is no `:`, defaults the channel to `"cli"`.
fn split_session(session: &str) -> (String, String) {
    if let Some((channel, chat_id)) = session.split_once(':') {
        (channel.to_string(), chat_id.to_string())
    } else {
        ("cli".to_string(), session.to_string())
    }
}

/// Returns `true` if the outbound message targets the given channel and chat ID.
fn matches_outbound_session(msg: &OutboundMessage, channel: &str, chat_id: &str) -> bool {
    msg.channel == channel && msg.chat_id == chat_id
}

/// Returns `true` if the input is an exit command (exit, quit, or variants).
fn is_exit_cmd(input: &str) -> bool {
    matches!(
        input.to_lowercase().as_str(),
        "exit" | "quit" | "/exit" | "/quit" | ":q"
    )
}
