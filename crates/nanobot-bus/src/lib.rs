//! # nanobot-bus
//!
//! The message bus crate for the nanobot agent framework. Provides a publish-subscribe
//! communication channel for routing messages between channel adapters and the agent
//! loop.
//!
//! ## Architecture
//!
//! [`MessageBus`] is the central hub, wrapping two independent
//! [`tokio::sync::broadcast`] channels — one for inbound messages (from channels to
//! the agent) and one for outbound messages (from the agent to channels).
//!
//! ```text
//! Channel Adapters           MessageBus               Agent Loop
//! ┌─────────────┐    ┌───────────────────┐    ┌──────────────────┐
//! │ Slack        │───▶│  inbound_tx       │───▶│  subscribe_inbound│
//! │ Discord      │    │  (broadcast)      │    │                   │
//! │ CLI          │    │                   │    │  Agent processes  │
//! │ Feishu       │    │  outbound_tx      │◀───│  and responds     │
//! └─────────────┘◀───│  (broadcast)      │    └──────────────────┘
//!                     └───────────────────┘
//! ```
//!
//! ## Key design decisions
//!
//! - **Broadcast semantics**: Every message is delivered to every subscriber. This
//!   enables multiple concurrent consumers (e.g., logging, persistence, agent) without
//!   extra wiring.
//! - **No message persistence**: The bus is an in-memory channel. Messages are lost if
//!   all receivers are dropped. Persistence is handled upstream by
//!   [`SessionManager`](https://docs.rs/nanobot-session).
//! - **Error type is minimal**: A `NoSubscribers` error signals when a message is
//!   published to a channel with zero active receivers.
//!
//! ## Re-exports
//!
//! For caller convenience this crate re-exports:
//!
//! - [`MessageBus`] and its builder
//! - [`BusError`] / [`BusResult`]
//! - All bus message types from `nanobot_types::bus` (`InboundMessage`,
//!   `OutboundMessage`, `MessageMetadata`, etc.)

pub mod error;
pub mod queue;

pub use self::queue::*;
pub use error::{BusError, BusResult};
pub use nanobot_types::bus::*;
