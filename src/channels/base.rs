use async_trait::async_trait;

use crate::bus::OutboundMessage;
use crate::channels::ChannelResult;

#[derive(Debug, Clone, Default)]
pub struct SendOutcome {
    pub message_id: Option<String>,
}

/// Common runtime contract for external channel adapters.
#[async_trait]
pub trait ChannelAdapter: Send + Sync {
    /// Stable adapter name, e.g. `telegram`, `cli`.
    fn name(&self) -> &str;

    /// Start inbound listening / connection lifecycle.
    async fn start(&self) -> ChannelResult<()>;

    /// Stop background tasks and clean up resources.
    async fn stop(&self) -> ChannelResult<()>;

    /// Deliver an outbound message to the external platform.
    async fn send(&self, msg: OutboundMessage) -> ChannelResult<SendOutcome>;

    /// Update an existing message on platforms that support edits.
    async fn update(&self, message_id: &str, msg: OutboundMessage) -> ChannelResult<()> {
        let _ = message_id;
        let _ = self.send(msg).await?;
        Ok(())
    }

    /// Whether the adapter supports message updates for streaming.
    fn supports_stream_updates(&self) -> bool {
        false
    }

    /// Best-effort runtime status.
    fn is_running(&self) -> bool;
}

/// Python-compatible allow-from evaluation:
/// - empty list: deny all
/// - contains `*`: allow all
/// - exact sender id or any `|`-split segment matches.
pub fn is_sender_allowed(allow_from: &[String], sender_id: &str) -> bool {
    if allow_from.is_empty() {
        return false;
    }
    if allow_from.iter().any(|v| v == "*") {
        return true;
    }
    if allow_from.iter().any(|v| v == sender_id) {
        return true;
    }
    sender_id
        .split('|')
        .filter(|p| !p.is_empty())
        .any(|p| allow_from.iter().any(|v| v == p))
}
