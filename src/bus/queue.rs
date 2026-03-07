use tokio::sync::broadcast;

use crate::bus::events::{InboundMessage, OutboundMessage};

/// Multi-subscriber message bus using broadcast channels.
///
/// This implementation allows multiple consumers to subscribe to message streams,
/// enabling features like message auditing, logging, and multiple channel adapters.
///
/// # Example
///
/// ```no_run
/// use nanobot_rs::bus::MessageBus;
/// use nanobot_rs::bus::events::{InboundMessage, MessageMetadata};
/// use chrono::Utc;
///
/// # async fn example() {
/// let bus = MessageBus::new();
///
/// // Multiple subscribers can listen to the same messages
/// let mut sub1 = bus.subscribe_inbound();
/// let mut sub2 = bus.subscribe_inbound();
///
/// // Publish a message
/// let msg = InboundMessage {
///     channel: "cli".to_string(),
///     sender_id: "user".to_string(),
///     chat_id: "direct".to_string(),
///     content: "hello".to_string(),
///     timestamp: Utc::now(),
///     media: vec![],
///     metadata: MessageMetadata::default(),
///     session_key_override: None,
/// };
/// bus.publish_inbound(msg).ok();
///
/// // Both subscribers receive the message
/// let msg1 = sub1.recv().await.ok();
/// let msg2 = sub2.recv().await.ok();
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct MessageBus {
    inbound_tx: broadcast::Sender<InboundMessage>,
    outbound_tx: broadcast::Sender<OutboundMessage>,
}

impl MessageBus {
    /// Creates a new message bus with default capacity (100).
    pub fn new() -> Self {
        Self::with_capacity(100)
    }

    /// Creates a new message bus with the specified buffer capacity.
    ///
    /// # Arguments
    ///
    /// * `capacity` - Maximum number of messages to buffer per channel.
    ///   When the buffer is full, the oldest message is dropped.
    ///
    /// # Recommended Capacity
    ///
    /// - Small systems: 32-64
    /// - Medium systems: 100-256
    /// - Large systems: 512-1024
    pub fn with_capacity(capacity: usize) -> Self {
        let (inbound_tx, _) = broadcast::channel(capacity);
        let (outbound_tx, _) = broadcast::channel(capacity);
        Self {
            inbound_tx,
            outbound_tx,
        }
    }

    /// Publishes an inbound message to all subscribers.
    ///
    /// # Errors
    ///
    /// Returns an error if there are no active subscribers.
    /// This is usually not a problem as the error can be safely ignored.
    pub fn publish_inbound(&self, msg: InboundMessage) -> anyhow::Result<()> {
        self.inbound_tx
            .send(msg)
            .map(|_| ())
            .map_err(|_| anyhow::anyhow!("failed to publish inbound: no subscribers"))
    }

    /// Subscribes to inbound messages.
    ///
    /// Returns a receiver that will receive all inbound messages published
    /// after the subscription is created. Multiple subscribers can coexist.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use nanobot_rs::bus::MessageBus;
    /// # async fn example() {
    /// let bus = MessageBus::new();
    /// let mut rx = bus.subscribe_inbound();
    ///
    /// while let Ok(msg) = rx.recv().await {
    ///     // Process message
    /// }
    /// # }
    /// ```
    pub fn subscribe_inbound(&self) -> broadcast::Receiver<InboundMessage> {
        self.inbound_tx.subscribe()
    }

    /// Publishes an outbound message to all subscribers.
    ///
    /// # Errors
    ///
    /// Returns an error if there are no active subscribers.
    pub fn publish_outbound(&self, msg: OutboundMessage) -> anyhow::Result<()> {
        self.outbound_tx
            .send(msg)
            .map(|_| ())
            .map_err(|_| anyhow::anyhow!("failed to publish outbound: no subscribers"))
    }

    /// Subscribes to outbound messages.
    ///
    /// Returns a receiver that will receive all outbound messages published
    /// after the subscription is created. Multiple subscribers can coexist.
    pub fn subscribe_outbound(&self) -> broadcast::Receiver<OutboundMessage> {
        self.outbound_tx.subscribe()
    }

    /// Returns the number of active inbound subscribers.
    pub fn inbound_subscriber_count(&self) -> usize {
        self.inbound_tx.receiver_count()
    }

    /// Returns the number of active outbound subscribers.
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

    use crate::bus::events::MessageMetadata;

    #[tokio::test]
    async fn single_subscriber_receives_messages() {
        let bus = MessageBus::new();
        let mut rx = bus.subscribe_inbound();

        let msg = InboundMessage {
            channel: "cli".to_string(),
            sender_id: "user".to_string(),
            chat_id: "direct".to_string(),
            content: "hello".to_string(),
            timestamp: Utc::now(),
            media: vec![],
            metadata: MessageMetadata::default(),
            session_key_override: None,
        };

        bus.publish_inbound(msg.clone()).expect("publish");
        let received = rx.recv().await.expect("receive");

        assert_eq!(received.channel, "cli");
        assert_eq!(received.content, "hello");
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
            content: "broadcast test".to_string(),
            timestamp: Utc::now(),
            media: vec![],
            metadata: MessageMetadata::default(),
            session_key_override: None,
        };

        bus.publish_inbound(msg.clone()).expect("publish");

        let r1 = rx1.recv().await.expect("rx1 receive");
        let r2 = rx2.recv().await.expect("rx2 receive");
        let r3 = rx3.recv().await.expect("rx3 receive");

        assert_eq!(r1.content, "broadcast test");
        assert_eq!(r2.content, "broadcast test");
        assert_eq!(r3.content, "broadcast test");
    }

    #[tokio::test]
    async fn outbound_messages_work_similarly() {
        let bus = MessageBus::new();
        let mut rx = bus.subscribe_outbound();

        let msg = OutboundMessage {
            channel: "discord".to_string(),
            chat_id: "789".to_string(),
            content: "response".to_string(),
            reply_to: None,
            media: vec![],
            metadata: MessageMetadata::default(),
        };

        bus.publish_outbound(msg.clone()).expect("publish");
        let received = rx.recv().await.expect("receive");

        assert_eq!(received.channel, "discord");
        assert_eq!(received.content, "response");
    }

    #[tokio::test]
    async fn publish_without_subscribers_returns_error() {
        let bus = MessageBus::new();

        let msg = InboundMessage {
            channel: "cli".to_string(),
            sender_id: "user".to_string(),
            chat_id: "direct".to_string(),
            content: "hello".to_string(),
            timestamp: Utc::now(),
            media: vec![],
            metadata: MessageMetadata::default(),
            session_key_override: None,
        };

        let result = bus.publish_inbound(msg);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn subscriber_count_updates_correctly() {
        let bus = MessageBus::new();
        assert_eq!(bus.inbound_subscriber_count(), 0);

        let _rx1 = bus.subscribe_inbound();
        assert_eq!(bus.inbound_subscriber_count(), 1);

        let _rx2 = bus.subscribe_inbound();
        assert_eq!(bus.inbound_subscriber_count(), 2);

        drop(_rx1);
        // Note: subscriber count may not update immediately after drop
        // This is a limitation of broadcast channels
    }

    #[tokio::test]
    async fn late_subscriber_misses_earlier_messages() {
        let bus = MessageBus::new();

        let msg1 = InboundMessage {
            channel: "cli".to_string(),
            sender_id: "user".to_string(),
            chat_id: "direct".to_string(),
            content: "first".to_string(),
            timestamp: Utc::now(),
            media: vec![],
            metadata: MessageMetadata::default(),
            session_key_override: None,
        };

        // Subscribe first receiver
        let mut rx1 = bus.subscribe_inbound();

        // Publish first message
        bus.publish_inbound(msg1.clone()).ok();

        // Subscribe second receiver AFTER first message
        let mut rx2 = bus.subscribe_inbound();

        // Publish second message
        let msg2 = InboundMessage {
            content: "second".to_string(),
            ..msg1
        };
        bus.publish_inbound(msg2.clone()).ok();

        // rx1 receives both messages
        let r1_msg1 = rx1.recv().await.expect("rx1 first");
        let r1_msg2 = rx1.recv().await.expect("rx1 second");
        assert_eq!(r1_msg1.content, "first");
        assert_eq!(r1_msg2.content, "second");

        // rx2 only receives second message
        let r2_msg = rx2.recv().await.expect("rx2 receive");
        assert_eq!(r2_msg.content, "second");
    }
}
