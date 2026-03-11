use std::sync::OnceLock;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

pub const COMPONENT_RUNTIME: &str = "runtime";
pub const COMPONENT_AGENT: &str = "agent";
pub const COMPONENT_REACT: &str = "react";
pub const COMPONENT_SUBAGENT: &str = "subagent";
pub const COMPONENT_BUS: &str = "bus";
pub const COMPONENT_CHANNELS: &str = "channels";
pub const COMPONENT_CRON: &str = "cron";
pub const COMPONENT_HEARTBEAT: &str = "heartbeat";
pub const COMPONENT_PROVIDER: &str = "provider";
pub const COMPONENT_TOOLS: &str = "tools";
pub const COMPONENT_SESSION: &str = "session";

pub const TARGET_RUNTIME: &str = "nanobot.runtime";
pub const TARGET_AGENT: &str = "nanobot.agent";
pub const TARGET_REACT: &str = "nanobot.agent.react";
pub const TARGET_SUBAGENT: &str = "nanobot.subagent";
pub const TARGET_BUS: &str = "nanobot.bus";
pub const TARGET_CHANNELS: &str = "nanobot.channels";
pub const TARGET_CRON: &str = "nanobot.cron";
pub const TARGET_HEARTBEAT: &str = "nanobot.heartbeat";
pub const TARGET_PROVIDER: &str = "nanobot.provider";
pub const TARGET_TOOLS: &str = "nanobot.tools";
pub const TARGET_SESSION: &str = "nanobot.session";

static INIT: OnceLock<()> = OnceLock::new();

pub fn init() {
    INIT.get_or_init(|| {
        let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

        let _ = tracing_subscriber::registry()
            .with(filter)
            .with(tracing_subscriber::fmt::layer().with_target(true))
            .try_init();
    });
}
