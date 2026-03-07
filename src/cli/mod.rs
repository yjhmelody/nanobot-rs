use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use clap::{Args, Parser, Subcommand};
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::agent::AgentLoop;
use crate::bus::MessageBus;
use crate::bus::{InboundMessage, MessageMetadata, OutboundMessage};
use crate::channels::ChannelManager;
use crate::config::{Config, get_config_path, load_config, save_config};
use crate::cron::{CronJob, CronJobHandler};
use crate::heartbeat::{HeartbeatExecuteHandler, HeartbeatNotifyHandler};
use crate::runtime::build_runtime;
use crate::utils::helpers::{get_workspace_path, sync_workspace_templates};

#[derive(Debug, Parser)]
#[command(name = "nanobot-rs")]
#[command(about = "nanobot")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    Onboard(OnboardArgs),
    Agent(AgentArgs),
    Gateway(GatewayArgs),
    Status,
}

#[derive(Debug, Args)]
pub struct OnboardArgs {
    #[arg(long)]
    pub overwrite: bool,
}

#[derive(Debug, Args)]
pub struct AgentArgs {
    #[arg(long, short)]
    pub message: Option<String>,
    #[arg(long, short, default_value = "cli:direct")]
    pub session: String,
}

#[derive(Debug, Args)]
pub struct GatewayArgs {
    #[arg(long, short, default_value_t = 18790)]
    pub port: u16,
}

pub async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Commands::Onboard(args) => onboard(args).await,
        Commands::Agent(args) => agent(args).await,
        Commands::Gateway(args) => gateway(args).await,
        Commands::Status => status().await,
    }
}

async fn onboard(args: OnboardArgs) -> Result<()> {
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

async fn agent(args: AgentArgs) -> Result<()> {
    let config = load_config(None)?;
    let workspace = get_workspace_path(Some(config.agents.defaults.workspace.as_str())).await?;
    sync_workspace_templates(&workspace, true).await?;

    let runtime = build_runtime(config).await?;

    if let Some(message) = args.message {
        let (channel, chat_id) = split_session(&args.session);
        let response = runtime
            .agent
            .process_direct(&message, &args.session, &channel, &chat_id)
            .await;
        runtime.agent.close_mcp().await;
        let response = response?;
        println!("\n🐈 nanobot\n\n{}\n", response);
        return Ok(());
    }

    println!("🐈 Interactive mode (type exit/quit to quit)\n");
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
        let input = line.trim();
        if input.is_empty() {
            continue;
        }
        if is_exit_cmd(input) {
            println!("Goodbye!");
            break;
        }

        let (channel, chat_id) = split_session(&args.session);
        let response = runtime
            .agent
            .process_direct(input, &args.session, &channel, &chat_id)
            .await
            .unwrap_or_else(|e| format!("Error: {}", e));

        println!("\n🐈 nanobot\n\n{}\n", response);
    }

    runtime.agent.close_mcp().await;
    Ok(())
}

async fn gateway(args: GatewayArgs) -> Result<()> {
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
                content: input,
                timestamp: chrono::Utc::now(),
                media: Vec::new(),
                metadata: MessageMetadata::default(),
                session_key_override: Some(session.clone()),
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

    Ok(())
}

async fn status() -> Result<()> {
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

struct SessionTargetPicker {
    agent: Arc<AgentLoop>,
    enabled_channels: Arc<std::collections::HashSet<String>>,
}

impl SessionTargetPicker {
    fn pick_target(&self) -> (String, String) {
        if let Ok(sessions) = self.agent.sessions.list_sessions() {
            for item in sessions {
                let key = item.key;
                if let Some((channel, chat_id)) = key.split_once(':') {
                    if channel == "cli" || channel == "system" {
                        continue;
                    }
                    if self.enabled_channels.contains(channel) && !chat_id.is_empty() {
                        return (channel.to_string(), chat_id.to_string());
                    }
                }
            }
        }
        ("cli".to_string(), "direct".to_string())
    }
}

struct GatewayCronJobHandler {
    agent: Arc<AgentLoop>,
    bus: MessageBus,
}

#[async_trait]
impl CronJobHandler for GatewayCronJobHandler {
    async fn on_job(&self, job: CronJob) -> Result<Option<String>> {
        let reminder_note = format!(
            "[Scheduled Task] Timer finished.\n\nTask '{}' has been triggered.\nScheduled instruction: {}",
            job.name, job.payload.message
        );

        let response = self
            .agent
            .process_direct(
                &reminder_note,
                &format!("cron:{}", job.id),
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
    async fn on_execute(&self, tasks: String) -> Result<String> {
        let (channel, chat_id) = self.picker.pick_target();
        self.agent
            .process_direct(&tasks, "heartbeat", &channel, &chat_id)
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))
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
        let (channel, chat_id) = self.picker.pick_target();
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

fn is_exit_cmd(input: &str) -> bool {
    matches!(
        input.to_lowercase().as_str(),
        "exit" | "quit" | "/exit" | "/quit" | ":q"
    )
}
