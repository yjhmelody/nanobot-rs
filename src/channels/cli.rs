use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;

use crate::bus::OutboundMessage;
use crate::channels::base::{ChannelAdapter, SendOutcome};
use crate::channels::ChannelResult;

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

    async fn start(&self) -> ChannelResult<()> {
        self.running.store(true, Ordering::SeqCst);
        Ok(())
    }

    async fn stop(&self) -> ChannelResult<()> {
        self.running.store(false, Ordering::SeqCst);
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> ChannelResult<SendOutcome> {
        if msg.content.trim().is_empty() {
            return Ok(SendOutcome::default());
        }
        println!("\n🐈 [{}:{}]\n{}\n", msg.channel, msg.chat_id, msg.content);
        Ok(SendOutcome::default())
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}
