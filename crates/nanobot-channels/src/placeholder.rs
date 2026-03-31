use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use tracing::warn;

use crate::LOG_TARGET;
use crate::base::{ChannelAdapter, SendOutcome};
use crate::error::ChannelResult;
use nanobot_bus::OutboundMessage;

pub struct PlaceholderChannel {
    name: &'static str,
    running: AtomicBool,
}

impl PlaceholderChannel {
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            running: AtomicBool::new(false),
        }
    }
}

#[async_trait]
impl ChannelAdapter for PlaceholderChannel {
    fn name(&self) -> &str {
        self.name
    }

    async fn start(&self) -> ChannelResult<()> {
        self.running.store(true, Ordering::SeqCst);
        warn!(
            target: LOG_TARGET,
            "channel '{}' is enabled but not implemented in nanobot-rs yet",
            self.name
        );
        Ok(())
    }

    async fn stop(&self) -> ChannelResult<()> {
        self.running.store(false, Ordering::SeqCst);
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> ChannelResult<SendOutcome> {
        warn!(
            target: LOG_TARGET,
            "dropping outbound for unimplemented channel '{}': {}",
            self.name, msg.chat_id
        );
        Ok(SendOutcome::default())
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}
