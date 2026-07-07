//! Internal types for the Feishu channel adapter.
//!
//! This module defines all the data structures used to communicate with
//! the Feishu / Lark Open API: message envelopes, event payloads, API
//! responses, and internal state helpers for streaming and rendering.
//!
//! # Organization
//!
//! - **API models** (de)serialize the JSON wire format for Feishu's REST
//!   and WebSocket APIs — [`FeishuIncomingEnvelope`], `FeishuTenantTokenResponse`,
//!   `FeishuApiResponse`, etc.
//! - **Rendering** — [`RenderMode`] controls whether outbound text is sent
//!   as plain text or as an interactive card.
//! - **Streaming state** — [`StreamEditState`] tracks per-message edit
//!   counts and content lengths for batching and sharding.
//! - **Caching** — [`CachedTenantAccessToken`] holds a bearer token with
//!   its expiration time.
//! - **Shared state** — [`FeishuCallbackState`] is passed to the Axum
//!   HTTP handler for incoming events.

use std::fmt;
use std::time::Instant;

use chrono::{DateTime, Utc};
use nanobot_bus::MessageBus;
use serde::{Deserialize, Serialize};

/// Message rendering mode for Feishu.
///
/// Controls how outbound text is formatted when sent to the Feishu platform.
/// The mode is resolved once per message (static for the message's lifetime).
///
/// | Variant | Behavior |
/// |---------|----------|
/// | [`Raw`](RenderMode::Raw) | Plain text (`msg_type = text`). ASCII table fallback for tables. |
/// | [`Card`](RenderMode::Card)  | Interactive card (`msg_type = interactive`) with `lark_md` for rich markdown rendering. |
/// | [`Auto`](RenderMode::Auto)  | Content-sniffing: code blocks, bold, inline code, lists trigger card; plain text stays raw. |
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderMode {
    /// Plain text msg_type + ASCII table fallback.
    Raw,
    /// Interactive card with lark_md for rich rendering.
    Card,
    /// Content sniffing: code blocks/bold/lists → card, else → raw.
    Auto,
}

impl RenderMode {
    /// Returns the Feishu `msg_type` string for this mode.
    ///
    /// `Raw` / `Auto` return `"text"`, `Card` returns `"interactive"`.
    pub fn as_msg_type(self) -> &'static str {
        match self {
            Self::Raw => "text",
            Self::Card => "interactive",
            Self::Auto => "text",
        }
    }

    /// Resolves a render mode, evaluating `Auto` via content sniffing.
    ///
    /// Non-`Auto` modes pass through unchanged.
    #[must_use]
    pub fn resolve(self, text: &str) -> RenderMode {
        match self {
            Self::Auto => sniff(text),
            other => other,
        }
    }
}

impl From<&str> for RenderMode {
    fn from(s: &str) -> Self {
        match s {
            "card" => Self::Card,
            "auto" => Self::Auto,
            _ => Self::Raw,
        }
    }
}

impl fmt::Display for RenderMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Raw => write!(f, "raw"),
            Self::Card => write!(f, "card"),
            Self::Auto => write!(f, "auto"),
        }
    }
}

/// Content-sniffing heuristic for `Auto` render mode.
///
/// Returns [`RenderMode::Card`] if the text contains markdown markers
/// (code fences, bold, inline code, links) or line-starting patterns
/// (headings, list markers, emoji bullets).  Tables and plain text
/// stay [`RenderMode::Raw`] because `lark_md` table support is limited.
///
/// This is a private implementation detail of [`RenderMode::resolve`].
fn sniff(text: &str) -> RenderMode {
    // Fast-path check for common markdown markers that benefit from card rendering.
    if text.contains("```")
        || text.contains("**")
        || text.contains('`')
        || (text.contains('[') && text.contains("]("))
    {
        return RenderMode::Card;
    }

    // Line-by-line check for headings, lists, and emoji bullets.
    let line_triggers = [
        "```", "# ", "## ", "### ", "- ", "* ", "▫️", "▪️", "•", "▲", "▼",
    ];
    if text.lines().any(|l| {
        let t = l.trim_start();
        line_triggers.iter().any(|pat| t.starts_with(pat))
    }) {
        return RenderMode::Card;
    }

    RenderMode::Raw
}

/// Per-message state for batching Feishu edit API calls and sharding long streams.
///
/// When the agent produces a stream of progress updates, the Feishu adapter
/// updates the same message repeatedly.  This struct tracks:
///
/// - How many edits have been made (Feishu limits edits per message).
/// - How much content has been flushed (to decide when to flush next).
/// - When the last flush happened (to rate-limit API calls).
///
/// When thresholds are exceeded, the adapter "shards" — sends a new message
/// and switches to editing that one instead.
pub struct StreamEditState {
    /// The actual message_id being edited (may differ from the dispatch key after sharding).
    pub actual_message_id: String,
    /// Number of edits performed on the current message.
    pub edit_count: usize,
    /// Content length (in chars) at last successful flush.
    pub last_flushed_len: usize,
    /// Timestamp of last successful flush.
    pub last_flush: Instant,
}

/// Payload for Feishu webhook text messages.
///
/// Serialized according to the
/// [Feishu webhook message format](https://open.feishu.cn/document/uAjLw4CM/ukTMukTMukTM/bot-v3/bot-overview).
#[derive(Debug, Serialize)]
pub struct FeishuWebhookMessage {
    /// Message type identifier (e.g. `"text"`, `"interactive"`).
    pub msg_type: String,
    /// The text content body.
    pub content: FeishuTextContent,
    /// Unix timestamp (seconds) for signature verification.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    /// HMAC-SHA256 signature for webhook authentication.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sign: Option<String>,
}

/// Text content block for Feishu webhook messages.
#[derive(Debug, Serialize)]
pub struct FeishuTextContent {
    /// The message text (supports some markdown in webhook mode).
    pub text: String,
}

/// Top-level envelope received from Feishu events (HTTP callback or WebSocket).
///
/// This covers the two Feishu callback shapes:
/// - URL verification (has `type` + `challenge`, no `header`/`event`).
/// - Message events (have `header` + `event`, no `challenge`).
#[derive(Debug, Deserialize)]
pub struct FeishuIncomingEnvelope {
    /// Event type discriminator: e.g. `"url_verification"`.
    #[serde(default)]
    pub r#type: Option<String>,
    /// Challenge string for URL verification handshake.
    #[serde(default)]
    pub challenge: Option<String>,
    /// Event header with event_type and optional verify_token.
    #[serde(default)]
    pub header: Option<FeishuEventHeader>,
    /// The actual message event payload.
    #[serde(default)]
    pub event: Option<FeishuMessageEvent>,
}

/// Event header from the Feishu event subscription framework.
#[derive(Debug, Deserialize)]
pub struct FeishuEventHeader {
    /// Verification token for callback authentication.
    #[serde(default)]
    pub token: Option<String>,
    /// Event type identifier (e.g. `"im.message.receive_v1"`).
    #[serde(default)]
    pub event_type: Option<String>,
}

/// An IM message event received from Feishu.
#[derive(Debug, Deserialize)]
pub struct FeishuMessageEvent {
    /// The sender of the message.
    #[serde(default)]
    pub sender: Option<FeishuSender>,
    /// The message content.
    #[serde(default)]
    pub message: Option<FeishuMessage>,
}

/// Sender information for a Feishu message event.
#[derive(Debug, Deserialize)]
pub struct FeishuSender {
    /// The sender's ID object (user_id / union_id / open_id).
    #[serde(default)]
    pub sender_id: Option<FeishuSenderId>,
}

/// Feishu sender identifiers at three levels of stability.
///
/// The [`extract_inbound_message`](super::util::extract_inbound_message) helper
/// prefers `user_id` (tenant-stable) > `union_id` (cross-app) > `open_id`
/// (app-specific) for the sender identifier used in allow-from checks.
#[derive(Debug, Deserialize)]
pub struct FeishuSenderId {
    /// user_id is the most stable identifier (tenant-wide employee ID, permanent).
    #[serde(default)]
    pub user_id: Option<String>,
    /// union_id is stable across all apps by the same developer.
    #[serde(default)]
    pub union_id: Option<String>,
    /// open_id is app-scoped and may change.
    #[serde(default)]
    pub open_id: Option<String>,
}

/// A Feishu message object from an inbound event.
#[derive(Debug, Deserialize)]
pub struct FeishuMessage {
    /// Platform-assigned message ID.
    #[serde(default)]
    pub message_id: Option<String>,
    /// Chat / conversation ID.
    #[serde(default)]
    pub chat_id: Option<String>,
    /// Message content type: `"text"`, `"image"`, etc.
    #[serde(default)]
    pub message_type: Option<String>,
    /// JSON-serialized content string (varies by message_type).
    #[serde(default)]
    pub content: Option<String>,
}

/// Shared state passed to the Axum HTTP callback handler.
///
/// Contains the channel name, message bus reference, access-control list,
/// and optional verification token for callback authentication.
#[derive(Clone)]
pub struct FeishuCallbackState {
    /// The channel instance name (for routing inbound messages).
    pub name: String,
    /// Shared message bus for publishing inbound messages.
    pub bus: MessageBus,
    /// Access-control list for inbound message filtering.
    pub allow_from: Vec<String>,
    /// Optional verification token for callback security.
    pub verify_token: Option<String>,
}

/// A cached Feishu tenant access token with its expiration time.
///
/// The token is valid for the duration of `expires_at` (typically 2 hours).
/// The adapter refreshes it automatically before expiry, with a 60-second
/// safety margin.
#[derive(Clone, Debug)]
pub struct CachedTenantAccessToken {
    /// The bearer token string for API authentication.
    pub access_token: String,
    /// UTC timestamp after which the token is considered expired.
    pub expires_at: DateTime<Utc>,
}

/// Response from the Feishu [`tenant_access_token/internal`](https://open.feishu.cn/document/server-docs/authentication-management/access-token/tenant_access_token_internal) endpoint.
#[derive(Debug, Deserialize)]
pub struct FeishuTenantTokenResponse {
    /// `0` on success.
    #[serde(default)]
    pub code: i64,
    /// Optional error message.
    #[serde(default)]
    pub msg: Option<String>,
    /// The tenant access token string.
    #[serde(default)]
    pub tenant_access_token: Option<String>,
    /// Token lifetime in seconds (default 7200).
    #[serde(default)]
    pub expire: Option<i64>,
}

/// Generic Feishu API response wrapper.
///
/// Most Feishu Open API endpoints return a JSON object with `code`, `msg`,
/// and `data` fields.  This generic struct captures that pattern.
#[derive(Debug, Deserialize)]
pub struct FeishuApiResponse<T> {
    /// API response code: `0` for success.
    #[serde(default)]
    pub code: i64,
    /// Optional error message.
    #[serde(default)]
    pub msg: Option<String>,
    /// Response payload (type depends on the endpoint).
    #[serde(default)]
    pub data: Option<T>,
}

/// Data payload from a successful send-message API call.
#[derive(Debug, Default, Deserialize)]
pub struct FeishuSendMessageData {
    /// The platform-assigned message ID for the sent message.
    #[serde(default)]
    pub message_id: Option<String>,
}

/// Data payload from a successful image-upload API call.
#[derive(Debug, Default, Deserialize)]
pub struct FeishuUploadImageData {
    /// The image key to use when sending the image as a message.
    #[serde(default)]
    pub image_key: Option<String>,
}
