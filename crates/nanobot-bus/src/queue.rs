use tokio::sync::broadcast;

use crate::{BusError, BusResult, InboundMessage, OutboundMessage};

/// Multi-subscriber message bus using broadcast channels.
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
    pub fn with_capacity(capacity: usize) -> Self {
        let (inbound_tx, _) = broadcast::channel(capacity);
        let (outbound_tx, _) = broadcast::channel(capacity);
        Self {
            inbound_tx,
            outbound_tx,
        }
    }

    /// Publishes an inbound message to all subscribers.
    pub fn publish_inbound(&self, msg: InboundMessage) -> BusResult<()> {
        self.inbound_tx
            .send(msg)
            .map(|_| ())
            .map_err(|_| BusError::no_subscribers("inbound"))
    }

    /// Subscribes to inbound messages.
    pub fn subscribe_inbound(&self) -> broadcast::Receiver<InboundMessage> {
        self.inbound_tx.subscribe()
    }

    /// Publishes an outbound message to all subscribers.
    pub fn publish_outbound(&self, msg: OutboundMessage) -> BusResult<()> {
        self.outbound_tx
            .send(msg)
            .map(|_| ())
            .map_err(|_| BusError::no_subscribers("outbound"))
    }

    /// Subscribes to outbound messages.
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

        let r2_msg = rx2.recv().await.expect("rx2 receive");
        assert_eq!(r2_msg.content_text(), "second");
    }
}
