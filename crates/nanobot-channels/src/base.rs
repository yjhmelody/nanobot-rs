//! Core trait and types that every channel adapter must implement.
//!
//! The [`ChannelAdapter`] trait defines the lifecycle contract (start / stop / send / update),
//! plus optional streaming support. All platform-specific adapters (CLI, Telegram, Feishu)
//! implement this trait and are registered in the [`ChannelManager`](crate::manager::ChannelManager).
//!
//! This module also provides [`is_sender_allowed`], a helper for evaluating
//! allow-from access-control lists in a way compatible with the Python configuration
//! convention.

use async_trait::async_trait;

use crate::error::ChannelResult;
use nanobot_bus::OutboundMessage;

/// Result returned by a channel adapter after sending a message.
///
/// Wraps optional platform-level metadata such as the server-assigned message ID,
/// which can be used later for updates (editing a streamed response).
#[derive(Debug, Clone, Default)]
pub struct SendOutcome {
    /// Platform-assigned message ID, if the channel supports it.
    ///
    /// For example, Feishu returns a `message_id` that can be passed to
    /// [`ChannelAdapter::update`] for streaming edits. CLI and Telegram
    /// typically yield `None`.
    pub message_id: Option<String>,
}

/// Common runtime contract for external channel adapters.
///
/// Every messaging platform exposed through nanobot needs a struct that
/// implements this trait.  The trait covers:
///
/// - **Lifecycle** – `start` / `stop` for setting up and tearing down
///   background listeners (HTTP callbacks, WebSocket connections, polling loops).
/// - **Outbound delivery** – `send` to push a response to the platform,
///   with optional `begin_stream` / `update` / `supports_stream_updates`
///   for progressive message editing.
/// - **Status** – `is_running` for health checks.
///
/// # Implementing
///
/// 1. Define a struct with the fields the platform needs
///    (token, API base, HTTP client, etc.).
/// 2. Wrap shared mutable state in `Arc` (e.g. `Arc<AtomicBool>` for `running`).
/// 3. Implement `name` to return a stable identifier matching the config key.
#[async_trait]
pub trait ChannelAdapter: Send + Sync {
    /// Stable adapter name, e.g. `telegram`, `cli`, `my_feishu_bot`.
    ///
    /// This is used by [`ChannelManager`](crate::manager::ChannelManager) for
    /// routing outbound messages and for log messages.
    fn name(&self) -> &str;

    /// Start inbound listening / connection lifecycle.
    ///
    /// This should be non-blocking; spawn background tasks as needed.
    /// # Errors
    /// Returns `ChannelError::Adapter` if the platform rejects credentials
    /// or the listener cannot bind.
    async fn start(&self) -> ChannelResult<()>;

    /// Stop background tasks and clean up resources.
    ///
    /// Sets internal flags so that polling loops / WebSocket clients exit,
    /// and aborts any spawned tasks.
    async fn stop(&self) -> ChannelResult<()>;

    /// Deliver an outbound message to the external platform.
    ///
    /// # Arguments
    /// * `msg` – The outbound message, containing channel routing info,
    ///   content text, optional media attachments, and metadata.
    ///
    /// # Returns
    /// A [`SendOutcome`] that may contain a platform-assigned message ID.
    ///
    /// # Errors
    /// Returns `ChannelError::Adapter` if the HTTP request fails or the
    /// platform rejects the message.
    async fn send(&self, msg: OutboundMessage) -> ChannelResult<SendOutcome>;

    /// Optionally create an initial placeholder for a streaming response.
    ///
    /// The default implementation returns `None`, meaning no placeholder is
    /// created and the first chunk is simply sent via [`send`](ChannelAdapter::send).
    ///
    /// Override this when the platform supports editing messages after sending,
    /// and you want to send a placeholder (e.g. "thinking...") immediately.
    ///
    /// # Returns
    /// `Some(outcome)` if a placeholder was created, `None` otherwise.
    async fn begin_stream(&self, msg: &OutboundMessage) -> ChannelResult<Option<SendOutcome>> {
        let _ = msg;
        Ok(None)
    }

    /// Update an existing message on platforms that support edits.
    ///
    /// The default implementation falls back to sending a new message (calls `send`).
    ///
    /// # Arguments
    /// * `message_id` – The platform-assigned ID of the message to update.
    /// * `msg` – New content for the message.
    async fn update(&self, message_id: &str, msg: OutboundMessage) -> ChannelResult<()> {
        let _ = message_id;
        let _ = self.send(msg).await?;
        Ok(())
    }

    /// Whether the adapter supports message updates for streaming.
    ///
    /// When `true`, the dispatcher will use `update` for subsequent chunks
    /// and `begin_stream` for the initial placeholder.  When `false`, each
    /// chunk is sent as a new message.
    fn supports_stream_updates(&self) -> bool {
        false
    }

    /// Best-effort runtime status.
    ///
    /// Returns `true` if the adapter has been started and has not been stopped.
    fn is_running(&self) -> bool;
}

/// Evaluates an allow-from access-control list for inbound messages.
///
/// Semantics (compatible with the Python configuration convention):
/// - Empty list: deny all senders.
/// - Contains `"*"`: allow all senders.
/// - Exact match against `sender_id` or any `|`-separated segment: allow.
///
/// # Arguments
/// * `allow_from` – The list of allowed sender identifiers from config.
/// * `sender_id`  – The sender identifier from the inbound message
///   (e.g. the chat ID or user ID as a string).
///
/// # Examples
/// ```
/// use nanobot_channels::base::is_sender_allowed;
/// assert!(is_sender_allowed(&["*".to_string()], "any_user"));
/// assert!(is_sender_allowed(&["alice".to_string()], "alice"));
/// assert!(is_sender_allowed(&["alice".to_string()], "bob|alice"));
/// assert!(!is_sender_allowed(&["alice".to_string()], "bob"));
/// assert!(!is_sender_allowed(&Vec::<String>::new(), "bob"));
/// ```
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
