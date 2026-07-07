//! Channel adapters for nanobot (e.g. CLI, Telegram, Feishu),
//! plus the `ChannelManager` that wires them to the `MessageBus`.
//!
//! # Architecture
//!
//! Each external messaging platform has a corresponding adapter module
//! that implements the [`ChannelAdapter`](base::ChannelAdapter) trait.
//! The [`ChannelManager`] acts as the orchestrator:
//!
//! - On construction, it builds adapter instances from configuration.
//! - On start, it launches the outbound dispatcher task (listening on
//!   the shared `MessageBus`) and starts each adapter's inbound listener.
//! - On stop, it tears everything down cleanly.
//!
//! # Channel Feature Gates
//!
//! | Feature | Module | Platform |
//! |---------|--------|----------|
//! | `channel-telegram` | `telegram` | Telegram Bot API |
//! | `channel-feishu` | `feishu` | Feishu / Lark |
//!
//! The CLI channel (`cli`) is always compiled in; it prints to stdout.
//!
//! # Error Handling
//!
//! All fallible operations return [`ChannelResult<T>`](error::ChannelResult),
//! which wraps the [`ChannelError`] enum. This provides
//! structured error variants for configuration issues vs. runtime failures.

pub mod base;
pub mod cli;
pub mod error;
#[cfg(feature = "channel-feishu")]
pub mod feishu;
pub mod manager;
#[cfg(feature = "channel-telegram")]
pub mod telegram;

pub use error::{ChannelError, ChannelResult};
pub use manager::ChannelManager;
