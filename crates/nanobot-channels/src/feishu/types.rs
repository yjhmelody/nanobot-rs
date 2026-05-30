//! Internal types for the feishu channel adapter.

use std::time::Instant;

use chrono::{DateTime, Utc};
use nanobot_bus::MessageBus;
use serde::{Deserialize, Serialize};

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

#[derive(Debug, Serialize)]
pub struct FeishuWebhookMessage {
    pub msg_type: String,
    pub content: FeishuTextContent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sign: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct FeishuTextContent {
    pub text: String,
}

#[derive(Debug, Deserialize)]
pub struct FeishuIncomingEnvelope {
    #[serde(default)]
    pub r#type: Option<String>,
    #[serde(default)]
    pub challenge: Option<String>,
    #[serde(default)]
    pub header: Option<FeishuEventHeader>,
    #[serde(default)]
    pub event: Option<FeishuMessageEvent>,
}

#[derive(Debug, Deserialize)]
pub struct FeishuEventHeader {
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub event_type: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct FeishuMessageEvent {
    #[serde(default)]
    pub sender: Option<FeishuSender>,
    #[serde(default)]
    pub message: Option<FeishuMessage>,
}

#[derive(Debug, Deserialize)]
pub struct FeishuSender {
    #[serde(default)]
    pub sender_id: Option<FeishuSenderId>,
}

#[derive(Debug, Deserialize)]
pub struct FeishuSenderId {
    /// user_id is the most stable identifier (tenant-wide employee ID, permanent).
    #[serde(default)]
    pub user_id: Option<String>,
    /// union_id is stable across all apps by the same developer.
    #[serde(default)]
    pub union_id: Option<String>,
    #[serde(default)]
    pub open_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct FeishuMessage {
    #[serde(default)]
    pub message_id: Option<String>,
    #[serde(default)]
    pub chat_id: Option<String>,
    #[serde(default)]
    pub message_type: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
}

#[derive(Clone)]
pub struct FeishuCallbackState {
    pub name: String,
    pub bus: MessageBus,
    pub allow_from: Vec<String>,
    pub verify_token: Option<String>,
}

#[derive(Clone, Debug)]
pub struct CachedTenantAccessToken {
    pub access_token: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct FeishuTenantTokenResponse {
    #[serde(default)]
    pub code: i64,
    #[serde(default)]
    pub msg: Option<String>,
    #[serde(default)]
    pub tenant_access_token: Option<String>,
    #[serde(default)]
    pub expire: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct FeishuApiResponse<T> {
    #[serde(default)]
    pub code: i64,
    #[serde(default)]
    pub msg: Option<String>,
    #[serde(default)]
    pub data: Option<T>,
}

#[derive(Debug, Default, Deserialize)]
pub struct FeishuSendMessageData {
    #[serde(default)]
    pub message_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct FeishuUploadImageData {
    #[serde(default)]
    pub image_key: Option<String>,
}
