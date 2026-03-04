use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use async_trait::async_trait;
use tracing::warn;

use crate::bus::OutboundMessage;
use crate::channels::base::ChannelAdapter;

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

    async fn start(&self) -> Result<()> {
        self.running.store(true, Ordering::SeqCst);
        warn!(
            "channel '{}' is enabled but not implemented in nanobot-rs yet",
            self.name
        );
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        self.running.store(false, Ordering::SeqCst);
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        warn!(
            "dropping outbound for unimplemented channel '{}': {}",
            self.name, msg.chat_id
        );
        Ok(())
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}
