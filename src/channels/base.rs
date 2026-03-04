use anyhow::Result;
use async_trait::async_trait;

use crate::bus::OutboundMessage;

/// Common runtime contract for external channel adapters.
#[async_trait]
pub trait ChannelAdapter: Send + Sync {
    /// Stable adapter name, e.g. `telegram`, `cli`.
    fn name(&self) -> &str;

    /// Start inbound listening / connection lifecycle.
    async fn start(&self) -> Result<()>;

    /// Stop background tasks and clean up resources.
    async fn stop(&self) -> Result<()>;

    /// Deliver an outbound message to the external platform.
    async fn send(&self, msg: OutboundMessage) -> Result<()>;

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
