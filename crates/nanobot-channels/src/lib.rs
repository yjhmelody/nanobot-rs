//! Channel adapters for nanobot: CLI, Telegram, and placeholder adapters,
//! plus the `ChannelManager` that wires them to the `MessageBus`.

pub mod base;
pub mod cli;
pub mod error;
pub mod manager;
pub mod placeholder;
pub mod telegram;

pub use error::{ChannelError, ChannelResult};
pub use manager::ChannelManager;

/// Tracing target for channel-related log events.
pub(crate) const LOG_TARGET: &str = "nanobot.channels";
