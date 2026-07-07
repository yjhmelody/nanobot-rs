//! CLI channel adapter — the simplest possible channel.
//!
//! The CLI channel prints outbound messages to stdout and ignores inbound
//! messages entirely (it has no listener).  It is always compiled in and is
//! auto-registered by [`ChannelManager`](crate::manager::ChannelManager) under
//! the name `"cli"`.
//!
//! # Use Cases
//! - Local development and testing without an external messaging platform.
//! - Debugging the outbound message pipeline.
//!
//! # Limitations
//! - No inbound message support (messages are produced by other means).
//! - No streaming updates; every message is printed as a new block.

use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;

use crate::base::{ChannelAdapter, SendOutcome};
use crate::error::ChannelResult;
use nanobot_bus::OutboundMessage;

/// Channel adapter that prints outbound messages to stdout.
///
/// This is the simplest adapter in the system and serves as both a reference
/// implementation of [`ChannelAdapter`] and a debugging aid.
///
/// # State
/// Uses an [`AtomicBool`] for the running flag — no heap allocation needed
/// since there are no background tasks.
pub struct CliChannel {
    running: AtomicBool,
}

impl CliChannel {
    /// Creates a new `CliChannel` in the stopped state.
    pub fn new() -> Self {
        Self {
            running: AtomicBool::new(false),
        }
    }
}

impl Default for CliChannel {
    fn default() -> Self {
        Self::new()
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
        // Skip empty messages to avoid printing blank lines.
        if msg.content.trim().is_empty() {
            return Ok(SendOutcome::default());
        }
        println!("\n[{}:{}]\n{}\n", msg.channel, msg.chat_id, msg.content);
        Ok(SendOutcome::default())
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}
