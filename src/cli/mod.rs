use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;

use async_trait::async_trait;
use clap::{Args, Parser, Subcommand};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command as TokioCommand;

use crate::acp::ACPConfig;
use crate::agent::AgentLoop;
use crate::bus::MessageBus;
use crate::bus::{InboundMessage, MessageMetadata, OutboundMessage};
use crate::channels::ChannelManager;
use crate::config::{Config, get_config_path, load_config, normalize_provider_name, save_config};
use crate::error::{NanobotError, NanobotResult};
use crate::cron::{CronJob, CronJobHandler, CronResult};
use crate::heartbeat::{HeartbeatError, HeartbeatExecuteHandler, HeartbeatNotifyHandler, HeartbeatResult};
use crate::runtime::RuntimeError;
use crate::runtime::build_runtime;
use crate::types::SessionKey;
use crate::utils::helpers::{get_workspace_path, sync_workspace_templates};

#[derive(Debug, Parser)]
#[command(
    name = "nanobot-rs",
    about = "nanobot command-line interface",
    long_about = "nanobot-rs command-line interface for onboarding, running the agent, and managing providers."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    #[command(about = "Initialize or refresh config and workspace templates.")]
    #[command(
        long_about = "Create or refresh ~/.nanobot/config.json and ensure workspace templates are present. Use --overwrite to reset to defaults."
    )]
    Onboard(OnboardArgs),
    #[command(about = "Run the agent in interactive or one-shot mode.")]
    #[command(
        long_about = "Start an interactive session by default, or run a single prompt with --message."
    )]
    Agent(AgentArgs),
    #[command(about = "Run the gateway service.")]
    #[command(
        long_about = "Start the gateway service with the configured channels and heartbeat loop."
    )]
    Gateway(GatewayArgs),
    #[command(about = "Show status of config, workspace, and providers.")]
    #[command(long_about = "Print paths and availability checks for config and workspace.")]
    Status,
    #[command(about = "Manage provider configuration and connectivity checks.")]
    #[command(long_about = "Login to a provider or show provider auth status.")]
    Provider(ProviderArgs),
}

#[derive(Debug, Args)]
pub struct OnboardArgs {
    #[arg(long, help = "Overwrite existing config with defaults.")]
    pub overwrite: bool,
}

#[derive(Debug, Args)]
pub struct AgentArgs {
    #[arg(long, short, help = "Send a single message and exit.")]
    pub message: Option<String>,
    #[arg(
        long,
        short,
        default_value = "cli:direct",
        help = "Session key in channel:chat_id format."
    )]
    pub session: String,
}

#[derive(Debug, Args)]
pub struct GatewayArgs {
    #[arg(
        long,
        short,
        default_value_t = 18790,
        help = "Port to bind the gateway service."
    )]
    pub port: u16,
}

#[derive(Debug, Args)]
pub struct ProviderArgs {
    #[command(subcommand)]
    pub command: ProviderCommands,
}

#[derive(Debug, Subcommand)]
pub enum ProviderCommands {
    #[command(about = "Store provider credentials.")]
    #[command(
        long_about = "Write provider API key into the config file for the selected provider."
    )]
    Login(ProviderLoginArgs),
    #[command(about = "Show provider auth status.")]
    #[command(long_about = "Check whether the selected provider has credentials configured.")]
    Status(ProviderStatusArgs),
}

#[derive(Debug, Args)]
pub struct ProviderLoginArgs {
    #[arg(help = "Provider name (e.g., anthropic, openai, custom).")]
    pub provider: String,
    #[arg(long, help = "Optional API host override.")]
    pub host: Option<String>,
    #[arg(long = "config-dir", help = "Config directory override.")]
    pub config_dir: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct ProviderStatusArgs {
    #[arg(help = "Provider name (e.g., anthropic, openai, custom).")]
    pub provider: String,
    #[arg(long = "config-dir", help = "Config directory override.")]
    pub config_dir: Option<PathBuf>,
}

pub async fn run(cli: Cli) -> NanobotResult<()> {
    match cli.command {
        Commands::Onboard(args) => onboard(args).await,
        Commands::Agent(args) => agent(args).await,
        Commands::Gateway(args) => gateway(args).await,
        Commands::Status => status().await,
        Commands::Provider(args) => provider(args).await,
    }
}

async fn onboard(args: OnboardArgs) -> NanobotResult<()> {
    let config_path = get_config_path()?;

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

    println!("\n🐈 nanobot-rs is ready!");
    println!("\nNext steps:");
    println!("  1. Add your API key to ~/.nanobot/config.json");
    println!("  2. Chat: nanobot-rs agent -m \"Hello!\"");

    Ok(())
}

async fn agent(args: AgentArgs) -> NanobotResult<()> {
    let config = load_config(None)?;
    tracing::trace!("load config: {:#?}", config);
    let workspace = get_workspace_path(Some(config.agents.defaults.workspace.as_str())).await?;
    sync_workspace_templates(&workspace, true).await?;

    let runtime = build_runtime(config).await?;

    if let Some(message) = args.message {
        let (channel, chat_id) = split_session(&args.session);
        let session_key = SessionKey::from(args.session.as_str());
        let response = runtime
            .agent
            .process_direct(&message, &session_key, &channel, &chat_id)
            .await;
        runtime.agent.close_mcp().await;
        runtime.agent.close_provider().await;
        let response = response?;
        println!("\n🐈 nanobot response:\n\n{}\n", response);
        return Ok(());
    }

    println!("🐈 Interactive mode (type exit/quit to quit)\n");
    let session_key = args.session.clone();
    let (channel, chat_id) = split_session(&args.session);
    let agent_task = tokio::spawn(runtime.agent.clone().run());

    let mut outbound_rx = runtime.bus.subscribe_outbound();
    let output_channel = channel.clone();
    let output_chat_id = chat_id.clone();
    let output_task = tokio::spawn(async move {
        loop {
            match outbound_rx.recv().await {
                Ok(msg) => {
                    if matches_outbound_session(&msg, &output_channel, &output_chat_id)
                        && !msg.content.trim().is_empty()
                    {
                        println!("\n🐈 nanobot\n\n{}\n", msg.content);
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

async fn gateway(args: GatewayArgs) -> NanobotResult<()> {
    let config = load_config(None)?;
    let workspace = get_workspace_path(Some(config.agents.defaults.workspace.as_str())).await?;
    sync_workspace_templates(&workspace, true).await?;

    let runtime = build_runtime(config).await?;
    let channels = Arc::new(ChannelManager::new(
        runtime.config.channels.clone(),
        runtime.bus.clone(),
    )?);
    println!("🐈 Starting nanobot-rs gateway on port {}...", args.port);

    let agent = runtime.agent.clone();
    let bus = runtime.bus.clone();
    let cron = runtime.cron.clone();
    let heartbeat = runtime.heartbeat.clone();
    let enabled = Arc::new(
        channels
            .enabled_channels()
            .into_iter()
            .collect::<std::collections::HashSet<_>>(),
    );
    let picker = Arc::new(SessionTargetPicker {
        agent: agent.clone(),
        enabled_channels: enabled,
    });

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

    let agent_task = tokio::spawn(agent.clone().run());

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

async fn status() -> NanobotResult<()> {
    let config_path = get_config_path()?;
    let config = load_config(Some(&config_path))?;
    let workspace = PathBuf::from(config.workspace_path());

    println!("🐈 nanobot-rs Status\n");
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
    println!("Model: {}", config.agents.defaults.model);

    if let Some(name) = config.get_provider_name(None) {
        println!("Provider: {}", name);
    }

    Ok(())
}

async fn provider(args: ProviderArgs) -> NanobotResult<()> {
    match args.command {
        ProviderCommands::Login(args) => provider_login(args).await,
        ProviderCommands::Status(args) => provider_status(args).await,
    }
}

async fn provider_login(args: ProviderLoginArgs) -> NanobotResult<()> {
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

    let config = load_config(None)?;
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

    let status = command
        .status()
        .await
        .map_err(|err| {
            NanobotError::Runtime(RuntimeError::message(format!(
                "failed to launch '{}': {}",
                command_name, err
            )))
        })?;

    if !status.success() {
        return Err(NanobotError::Runtime(RuntimeError::message(format!(
            "GitHub Copilot login exited with status {}",
            status.code().unwrap_or(-1)
        ))));
    }

    Ok(())
}

async fn provider_status(args: ProviderStatusArgs) -> NanobotResult<()> {
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

    let config = load_config(None)?;
    let command_name = copilot_command_name(&config);
    let binary_path = which::which(&command_name).ok();
    let version = TokioCommand::new(&command_name)
        .arg("--version")
        .output()
        .await
        .ok();
    let env_token = ["COPILOT_GITHUB_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"]
        .iter()
        .find_map(|name| {
            std::env::var(name)
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .is_some();
    let config_dir = resolve_copilot_config_dir(args.config_dir);

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

struct SessionTargetPicker {
    agent: Arc<AgentLoop>,
    enabled_channels: Arc<HashSet<String>>,
}

impl SessionTargetPicker {
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
            .unwrap_or_else(|e| format!("Error: {}", e));

        if job.payload.deliver {
            if let Some(chat_id) = job.payload.to.as_deref() {
                if !response.trim().is_empty() {
                    let _ = self.bus.publish_outbound(OutboundMessage {
                        channel: job.payload.channel.unwrap_or_else(|| "cli".to_string()),
                        chat_id: chat_id.to_string(),
                        content: response.clone(),
                        reply_to: None,
                        media: Vec::new(),
                        metadata: MessageMetadata::default(),
                    });
                }
            }
        }

        Ok(Some(response))
    }
}

#[derive(Clone)]
struct GatewayHeartbeatExecuteHandler {
    agent: Arc<AgentLoop>,
    picker: Arc<SessionTargetPicker>,
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

#[derive(Clone)]
struct GatewayHeartbeatNotifyHandler {
    bus: MessageBus,
    picker: Arc<SessionTargetPicker>,
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

fn split_session(session: &str) -> (String, String) {
    if let Some((channel, chat_id)) = session.split_once(':') {
        (channel.to_string(), chat_id.to_string())
    } else {
        ("cli".to_string(), session.to_string())
    }
}

fn matches_outbound_session(msg: &OutboundMessage, channel: &str, chat_id: &str) -> bool {
    msg.channel == channel && msg.chat_id == chat_id
}

fn is_exit_cmd(input: &str) -> bool {
    matches!(
        input.to_lowercase().as_str(),
        "exit" | "quit" | "/exit" | "/quit" | ":q"
    )
}
