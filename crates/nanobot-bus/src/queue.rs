//! # Message Queue / Bus Implementation
//!
//! Implements the core [`MessageBus`] type — a multi-subscriber publish-subscribe
//! channel for routing messages between channel adapters and the agent loop.
//!
//! ## Channel architecture
//!
//! The bus wraps two independent [`tokio::sync::broadcast`] channels:
//!
//! - **Inbound channel** (`inbound_tx`): Carries [`InboundMessage`]s from external
//!   channels (CLI, Slack, Feishu, etc.) into the agent loop.
//! - **Outbound channel** (`outbound_tx`): Carries [`OutboundMessage`]s from the agent
//!   loop back out to channel adapters for delivery.
//!
//! ## Why broadcast?
//!
//! `tokio::sync::broadcast` was chosen over `mpsc` (multi-producer, single-consumer)
//! because the bus needs fan-out delivery: multiple consumers (agent, logger,
//! persistence hook) must each receive every message independently. With `mpsc` the
//! crate would need to manually implement forwarding to multiple receivers, which
//! broadcast provides natively.
//!
//! ### Trade-offs
//!
//! - **Lagging receivers**: If a receiver falls behind it will miss messages (the
//!   channel's ring buffer wraps). This is acceptable because the bus is an
//!   in-memory transit layer; durable persistence is handled by session storage.
//! - **No back-pressure**: A slow receiver can cause all receivers to miss messages.
//!   In practice the agent loop and channel adapters are the only consumers and
//!   they process at roughly the same rate.

use nanobot_types::text::truncate_utf8_prefix;
use tokio::sync::broadcast;
use tracing::info;

use crate::{BusError, BusResult, InboundMessage, OutboundMessage};

/// Maximum length of content preview in inbound message logs.
///
/// Content longer than this value is truncated using UTF-8-safe prefix
/// truncation before being included in trace log lines. This prevents
/// excessively long log entries when users send large messages.
///
/// # Value
///
/// 120 characters — chosen to fit comfortably within a single terminal
/// line at typical widths.
const CONTENT_PREVIEW_MAX: usize = 120;

/// Multi-subscriber message bus using broadcast channels.
///
/// `MessageBus` is the central communication hub of the nanobot framework.
/// It provides two independent publish-subscribe channels:
///
/// - **Inbound**: External messages arriving from channel adapters (CLI, Slack, etc.)
/// - **Outbound**: Responses produced by the agent loop to be sent back through channels
///
/// Each channel supports any number of publishers and subscribers. Messages sent
/// through the bus are delivered to all active subscribers for that channel.
///
/// # Clone semantics
///
/// `MessageBus` derives `Clone` — cloning produces a new handle to the **same**
/// underlying broadcast senders. This is safe because the senders are `Arc`-like
/// (via `tokio::sync::broadcast::Sender`): all clones share the same ring buffer
/// state.
///
/// # Examples
///
/// ```rust
/// use nanobot_bus::MessageBus;
/// use nanobot_types::bus::InboundMessage;
///
/// let bus = MessageBus::new();
/// let mut rx = bus.subscribe_inbound();
///
/// // Publish a message (in a real scenario this comes from a channel adapter)
/// // bus.publish_inbound(msg)...;
/// ```
#[derive(Debug, Clone)]
pub struct MessageBus {
    /// Sender half of the inbound broadcast channel.
    /// Messages published here are received by all inbound subscribers (e.g., the
    /// agent loop, the logging subsystem).
    inbound_tx: broadcast::Sender<InboundMessage>,

    /// Sender half of the outbound broadcast channel.
    /// Messages published here are received by all outbound subscribers (e.g.,
    /// channel adapters that deliver the message to end users).
    outbound_tx: broadcast::Sender<OutboundMessage>,
}

impl MessageBus {
    /// Creates a new message bus with a default buffer capacity of 100 messages.
    ///
    /// This is equivalent to calling `MessageBus::with_capacity(100)`.
    ///
    /// The capacity applies independently to both the inbound and outbound
    /// channels. If all receivers fall behind by more than `capacity` messages,
    /// the oldest messages are dropped from the ring buffer.
    ///
    /// # Returns
    ///
    /// A new `MessageBus` instance ready for publishing and subscribing.
    pub fn new() -> Self {
        Self::with_capacity(100)
    }

    /// Creates a new message bus with the specified buffer capacity.
    ///
    /// # Parameters
    ///
    /// * `capacity` — The number of messages each channel's ring buffer can hold
    ///   before old messages are overwritten. A larger capacity reduces the chance
    ///   of lagging receivers missing messages, at the cost of increased memory
    ///   usage.
    ///
    /// # Returns
    ///
    /// A new `MessageBus` instance with the given capacity.
    ///
    /// # Panics
    ///
    /// Panics if `capacity` is 0 (this is a limitation of
    /// [`tokio::sync::broadcast::channel`]).
    pub fn with_capacity(capacity: usize) -> Self {
        // Create separate broadcast channels for inbound and outbound traffic.
        // The `_` discard on the receiver halves is intentional: the first
        // receiver is immediately dropped because subscribers are created lazily
        // via `subscribe_*()`. If we kept it alive it would double-count
        // subscribers.
        let (inbound_tx, _) = broadcast::channel(capacity);
        let (outbound_tx, _) = broadcast::channel(capacity);
        Self {
            inbound_tx,
            outbound_tx,
        }
    }

    /// Publishes an inbound message to all subscribers.
    ///
    /// Before sending, the method logs a structured trace event containing the
    /// channel name, sender ID, chat ID, media count, and a truncated content
    /// preview.
    ///
    /// # Parameters
    ///
    /// * `msg` — The [`InboundMessage`] to publish. This is consumed (moved) into
    ///   the broadcast channel.
    ///
    /// # Returns
    ///
    /// * `Ok(())` if at least one subscriber received the message.
    /// * `Err(BusError::NoSubscribers { kind: "inbound" })` if there are zero
    ///   active inbound subscribers.
    ///
    /// # Logging
    ///
    /// Uses the `nanobot::bus` tracing target. The content preview is truncated to
    /// [`CONTENT_PREVIEW_MAX`] characters to avoid bloating logs.
    pub fn publish_inbound(&self, msg: InboundMessage) -> BusResult<()> {
        let preview = msg.content.as_text();
        let preview = if preview.len() > CONTENT_PREVIEW_MAX {
            // Use UTF-8-safe truncation so we don't split in the middle of a
            // multi-byte character, which would produce garbled log output.
            truncate_utf8_prefix(preview.trim(), CONTENT_PREVIEW_MAX)
        } else {
            preview
        };
        info!(
            target: "nanobot::bus",
            channel = %msg.channel,
            sender = %msg.sender_id,
            chat_id = %msg.chat_id,
            media = msg.media.len(),
            content_preview = %preview,
            "inbound message received"
        );
        // `send` returns the number of receivers that received the message on Ok,
        // but we map to () because callers only care about success/failure.
        // On error the only current failure mode is "no receivers" (SendError
        // contains the message back, but we discard it since NoSubscribers conveys
        // enough information).
        self.inbound_tx
            .send(msg)
            .map(|_| ())
            .map_err(|_| BusError::no_subscribers("inbound"))
    }

    /// Subscribes to inbound messages.
    ///
    /// Returns a new receiver that will see all inbound messages published *after*
    /// the subscription is created. Messages published before subscription are not
    /// replayed (broadcast channels are not persistent buffers).
    ///
    /// # Returns
    ///
    /// A `broadcast::Receiver<InboundMessage>` that yields inbound messages via
    /// `recv()`.
    ///
    /// # Multiple subscriptions
    ///
    /// Each call produces an independent receiver. Callers can spawn multiple
    /// consumers that each receive the full stream of inbound messages.
    pub fn subscribe_inbound(&self) -> broadcast::Receiver<InboundMessage> {
        self.inbound_tx.subscribe()
    }

    /// Publishes an outbound message to all subscribers.
    ///
    /// Before sending, the method classifies the message type based on its
    /// [`MessageId`](crate::MessageId) — "progress", "tool_hint", or "reply" —
    /// and logs accordingly. Progress and tool hint messages log only a content
    /// length (to reduce noise), while replies include a truncated content preview.
    ///
    /// # Parameters
    ///
    /// * `msg` — The [`OutboundMessage`] to publish. This is consumed (moved) into
    ///   the broadcast channel.
    ///
    /// # Returns
    ///
    /// * `Ok(())` if at least one subscriber received the message.
    /// * `Err(BusError::NoSubscribers { kind: "outbound" })` if there are zero
    ///   active outbound subscribers.
    ///
    /// # Logging
    ///
    /// Uses the `nanobot::bus` tracing target with the `msg_type` field set to
    /// one of `"progress"`, `"tool_hint"`, `"reply"`, or `"send"` (fallback).
    pub fn publish_outbound(&self, msg: OutboundMessage) -> BusResult<()> {
        let preview = truncate_utf8_prefix(msg.content.trim(), CONTENT_PREVIEW_MAX);
        // Classify the message based on its MessageId tag. This determines
        // which log fields are included: progress/tool_hint messages omit the
        // full content preview to keep logs more readable during streaming.
        let msg_type = msg
            .metadata
            .message_id
            .as_ref()
            .map(|id| {
                if id.is_progress() {
                    "progress"
                } else if id.is_tool_hint() {
                    "tool_hint"
                } else {
                    "reply"
                }
            })
            .unwrap_or("send");
        if msg_type == "progress" || msg_type == "tool_hint" {
            info!(
                target: "nanobot::bus",
                channel = %msg.channel,
                chat_id = %msg.chat_id,
                msg_type,
                content_len = msg.content.len(),
                "outbound message sent"
            );
        } else {
            info!(
                target: "nanobot::bus",
                channel = %msg.channel,
                chat_id = %msg.chat_id,
                media = msg.media.len(),
                msg_type,
                content_preview = %preview,
                "outbound message sent"
            );
        }
        self.outbound_tx
            .send(msg)
            .map(|_| ())
            .map_err(|_| BusError::no_subscribers("outbound"))
    }

    /// Subscribes to outbound messages.
    ///
    /// Returns a new receiver that will see all outbound messages published *after*
    /// the subscription is created. Messages published before subscription are not
    /// replayed.
    ///
    /// # Returns
    ///
    /// A `broadcast::Receiver<OutboundMessage>` that yields outbound messages via
    /// `recv()`.
    ///
    /// # Multiple subscriptions
    ///
    /// Each call produces an independent receiver. Channel adapters typically
    /// create one subscription each and filter by their channel name.
    pub fn subscribe_outbound(&self) -> broadcast::Receiver<OutboundMessage> {
        self.outbound_tx.subscribe()
    }

    /// Returns the number of active inbound subscribers.
    ///
    /// This is useful for diagnostics and testing — it can confirm that expected
    /// consumers have subscribed before publishing.
    ///
    /// # Returns
    ///
    /// The count of live `broadcast::Receiver` handles for the inbound channel.
    pub fn inbound_subscriber_count(&self) -> usize {
        self.inbound_tx.receiver_count()
    }

    /// Returns the number of active outbound subscribers.
    ///
    /// This is useful for diagnostics and testing — it can confirm that expected
    /// consumers have subscribed before publishing.
    ///
    /// # Returns
    ///
    /// The count of live `broadcast::Receiver` handles for the outbound channel.
    pub fn outbound_subscriber_count(&self) -> usize {
        self.outbound_tx.receiver_count()
    }
}

impl Default for MessageBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use nanobot_types::bus::MessageMetadata;

    #[tokio::test]
    async fn single_subscriber_receives_messages() {
        let bus = MessageBus::new();
        let mut rx = bus.subscribe_inbound();

        let msg = InboundMessage {
            channel: "cli".to_string(),
            sender_id: "user".to_string(),
            chat_id: "direct".to_string(),
            content: "hello".into(),
            timestamp: Utc::now(),
            media: vec![],
            metadata: MessageMetadata::default(),
            session_key_override: None,
        };

        bus.publish_inbound(msg.clone()).expect("publish");
        let received = rx.recv().await.expect("receive");

        assert_eq!(received.channel, "cli");
        assert_eq!(received.content_text(), "hello");
    }

    #[tokio::test]
    async fn multiple_subscribers_receive_same_message() {
        let bus = MessageBus::new();
        let mut rx1 = bus.subscribe_inbound();
        let mut rx2 = bus.subscribe_inbound();
        let mut rx3 = bus.subscribe_inbound();

        assert_eq!(bus.inbound_subscriber_count(), 3);

        let msg = InboundMessage {
            channel: "telegram".to_string(),
            sender_id: "user123".to_string(),
            chat_id: "chat456".to_string(),
            content: "broadcast test".into(),
            timestamp: Utc::now(),
            media: vec![],
            metadata: MessageMetadata::default(),
            session_key_override: None,
        };

        bus.publish_inbound(msg.clone()).expect("publish");

        let r1 = rx1.recv().await.expect("rx1 receive");
        let r2 = rx2.recv().await.expect("rx2 receive");
        let r3 = rx3.recv().await.expect("rx3 receive");

        assert_eq!(r1.content_text(), "broadcast test");
        assert_eq!(r2.content_text(), "broadcast test");
        assert_eq!(r3.content_text(), "broadcast test");
    }

    #[tokio::test]
    async fn late_subscriber_misses_early_messages() {
        let bus = MessageBus::new();

        let msg1 = InboundMessage {
            channel: "cli".to_string(),
            sender_id: "user".to_string(),
            chat_id: "direct".to_string(),
            content: "first".into(),
            timestamp: Utc::now(),
            media: vec![],
            metadata: MessageMetadata::default(),
            session_key_override: None,
        };

        let mut rx1 = bus.subscribe_inbound();
        bus.publish_inbound(msg1.clone()).ok();

        // rx2 subscribes after the first message was published — it should only
        // see the second message, demonstrating broadcast's no-replay semantics.
        let mut rx2 = bus.subscribe_inbound();

        let msg2 = InboundMessage {
            content: "second".into(),
            ..msg1
        };
        bus.publish_inbound(msg2.clone()).ok();

        let r1_msg1 = rx1.recv().await.expect("rx1 first");
        let r1_msg2 = rx1.recv().await.expect("rx1 second");
        assert_eq!(r1_msg1.content_text(), "first");
        assert_eq!(r1_msg2.content_text(), "second");

        // rx2 should have missed the first message
        let r2_msg = rx2.recv().await.expect("rx2 receive");
        assert_eq!(r2_msg.content_text(), "second");
    }
}
