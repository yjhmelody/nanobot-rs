use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use async_trait::async_trait;

use crate::bus::OutboundMessage;
use crate::channels::base::ChannelAdapter;

pub struct CliChannel {
    running: AtomicBool,
}

impl CliChannel {
    pub fn new() -> Self {
        Self {
            running: AtomicBool::new(false),
        }
    }
}

#[async_trait]
impl ChannelAdapter for CliChannel {
    fn name(&self) -> &str {
        "cli"
    }

    async fn start(&self) -> Result<()> {
        self.running.store(true, Ordering::SeqCst);
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        self.running.store(false, Ordering::SeqCst);
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        if msg.content.trim().is_empty() {
            return Ok(());
        }
        println!("\n🐈 [{}:{}]\n{}\n", msg.channel, msg.chat_id, msg.content);
        Ok(())
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}
