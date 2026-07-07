//! Utility functions for the Feishu channel adapter.
//!
//! This module provides:
//!
//! - **Inbound parsing** — [`extract_inbound_message`] converts raw Feishu
//!   event payloads into the crate's internal [`InboundMessage`] format.
//! - **Webhook helpers** — [`build_webhook_url`], [`build_signature`] for
//!   configuring and authenticating webhook messages.
//! - **Text processing** — [`split_text`] (with char-boundary awareness),
//!   [`serialize_text_content`], [`convert_markdown_tables`] for converting
//!   Markdown tables to plain-ASCII format.
//! - **Media helpers** — [`infer_file_name`], [`infer_image_mime_from_name`],
//!   [`extract_feishu_image_key_ref`] for handling image attachments.
//! - **Error classification** — [`is_retryable_auth_send_error`],
//!   [`is_success_response`], [`error_message`] for processing API responses.
//! - **Path utilities** — [`normalize_path`] ensures callback paths are
//!   well-formed.
//! - **Char-boundary safety** — [`floor_char_boundary`] ensures string
//!   slicing does not split a multi-byte UTF-8 character.

use std::path::Path;

use crate::base::is_sender_allowed;
use crate::error::{ChannelError, ChannelResult};
use crate::feishu::types::*;

use base64::Engine;
use hmac::{Hmac, KeyInit, Mac};
use nanobot_bus::{InboundMessage, MessageId, MessageMetadata};
use nanobot_config::schema::FeishuChannelConfig;
type HmacSha256 = Hmac<sha2::Sha256>;

/// Extracts an [`InboundMessage`] from a Feishu event envelope, if applicable.
///
/// Only processes `im.message.receive_v1` events with `text` or `image`
/// message types.  Other event types and message types are silently ignored
/// (returns `Ok(None)`).
///
/// # Arguments
/// * `channel_name` — The channel instance name (used for message routing).
/// * `payload` — The raw Feishu event envelope from HTTP callback or WebSocket.
/// * `allow_from` — Access-control list; sender IDs not in this list are
///   silently dropped (returns `Ok(None)`).
///
/// # Returns
/// * `Ok(Some(InboundMessage))` — A valid inbound message was parsed.
/// * `Ok(None)` — The event was not a supported message type or the sender
///   is not allowed.
/// * `Err(ChannelError)` — The payload was structurally invalid (missing
///   required fields, unparseable content JSON).
///
/// # Sender ID Priority
/// Uses `user_id` (tenant-wide stable) > `union_id` (cross-app stable) >
/// `open_id` (app-specific) for the sender identifier.
pub fn extract_inbound_message(
    channel_name: &str,
    payload: &FeishuIncomingEnvelope,
    allow_from: &[String],
) -> ChannelResult<Option<InboundMessage>> {
    let event_type = payload
        .header
        .as_ref()
        .and_then(|h| h.event_type.as_deref())
        .unwrap_or_default();
    if event_type != "im.message.receive_v1" {
        return Ok(None);
    }

    let Some(event) = payload.event.as_ref() else {
        return Ok(None);
    };
    let Some(message) = event.message.as_ref() else {
        return Ok(None);
    };
    let message_type = message.message_type.as_deref().unwrap_or_default();
    if message_type != "text" && message_type != "image" {
        return Ok(None);
    }

    let sender_id = event
        .sender
        .as_ref()
        .and_then(|s| s.sender_id.as_ref())
        .and_then(|s| {
            // Priority: user_id (tenant-stable) > union_id (cross-app) > open_id (app-specific)
            s.user_id
                .as_deref()
                .or(s.union_id.as_deref())
                .or(s.open_id.as_deref())
        })
        .ok_or_else(|| ChannelError::adapter("feishu", "missing sender id"))?
        .to_string();
    if !is_sender_allowed(allow_from, &sender_id) {
        return Ok(None);
    }

    let chat_id = message
        .chat_id
        .as_deref()
        .ok_or_else(|| ChannelError::adapter("feishu", "missing chat_id"))?
        .to_string();
    let message_id = message
        .message_id
        .as_deref()
        .ok_or_else(|| ChannelError::adapter("feishu", "missing message_id"))?
        .to_string();
    let content_json = message
        .content
        .as_deref()
        .ok_or_else(|| ChannelError::adapter("feishu", "missing content"))?;
    let content_value: serde_json::Value = serde_json::from_str(content_json)
        .map_err(|err| ChannelError::adapter("feishu", format!("invalid content json: {}", err)))?;
    let (text, media) = if message_type == "text" {
        let text = content_value
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        if text.is_empty() {
            return Ok(None);
        }
        (text, Vec::new())
    } else {
        let image_key = content_value
            .get("image_key")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        if image_key.is_empty() {
            return Ok(None);
        }
        (
            format!("[image: {}]", image_key),
            vec![format!("feishu:image_key:{}", image_key)],
        )
    };

    Ok(Some(InboundMessage {
        channel: channel_name.to_string(),
        sender_id,
        chat_id,
        content: text.into(),
        timestamp: chrono::Utc::now(),
        media,
        metadata: MessageMetadata {
            message_id: Some(MessageId::External(message_id)),
            stream_id: None,
        },
        session_key_override: None,
    }))
}

/// Build the full Feishu webhook URL from config.
///
/// If the config value is already a full URL (`http://...` or `https://...`),
/// it is returned as-is.  Otherwise it is treated as a bot key and expanded to
/// `{api_base}/open-apis/bot/v2/hook/{key}`.
///
/// Returns `None` if no webhook URL or bot key is configured.
pub fn build_webhook_url(cfg: &FeishuChannelConfig, api_base: &str) -> Option<String> {
    let webhook_or_key = cfg.webhook_url.as_deref()?;
    if webhook_or_key.starts_with("http://") || webhook_or_key.starts_with("https://") {
        return Some(webhook_or_key.to_string());
    }
    Some(format!(
        "{}/open-apis/bot/v2/hook/{}",
        api_base.trim_end_matches('/'),
        webhook_or_key
    ))
}

/// Build an HMAC-SHA256 signature for Feishu webhook authentication.
///
/// The signature is computed over the string `"{timestamp}\n{secret}"` using
/// the SHA-256 HMAC algorithm, then base64-encoded.  This matches the Feishu
/// [webhook signature verification](https://open.feishu.cn/document/uAjLw4CM/ukTMukTMukTM/bot-v3/bot-overview#2c87c4e2) spec.
///
/// # Errors
/// Returns [`ChannelError::Adapter`] if HMAC key initialization fails.
pub fn build_signature(timestamp: &str, secret: &str) -> ChannelResult<String> {
    let string_to_sign = format!("{}\n{}", timestamp, secret);
    let mut mac = HmacSha256::new_from_slice(string_to_sign.as_bytes()).map_err(|err| {
        ChannelError::adapter("feishu", format!("failed to build signature key: {}", err))
    })?;
    mac.update(&[]);
    Ok(base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes()))
}

/// Normalize a callback path: ensures it starts with `/` and provides a
/// sensible default (`"/feishu/events"`) for empty or root paths.
pub fn normalize_path(path: &str) -> String {
    if path.is_empty() || path == "/" {
        return "/feishu/events".to_string();
    }
    if path.starts_with('/') {
        return path.to_string();
    }
    format!("/{}", path)
}

/// Infer a filename from a URL or local file path.
///
/// Strips query parameters, handles URLs ending with `/`, and falls back
/// to `"image.jpg"` when no filename can be determined.
///
/// # Examples
/// ```
/// # use nanobot_channels::feishu::util::infer_file_name;
/// assert_eq!(infer_file_name("https://example.com/pic.png?x=1"), "pic.png");
/// assert_eq!(infer_file_name("/tmp/demo.jpg"), "demo.jpg");
/// assert_eq!(infer_file_name("https://example.com/"), "image.jpg");
/// ```
pub fn infer_file_name(input: &str) -> String {
    let source = input.split('?').next().unwrap_or(input);
    if source.ends_with('/') {
        return "image.jpg".to_string();
    }
    let source = source.trim_end_matches('/');
    if let Some(index) = source.find("://") {
        let remainder = &source[index + 3..];
        if !remainder.contains('/') {
            return "image.jpg".to_string();
        }
    }
    let name = source.rsplit('/').next().unwrap_or("image.jpg");
    if name.is_empty() {
        "image.jpg".to_string()
    } else {
        name.to_string()
    }
}

/// Extract the Feishu image key from a `"feishu:image_key:{key}"` media reference.
///
/// Returns `None` if the reference is not a Feishu image key or is empty.
pub fn extract_feishu_image_key_ref(media_ref: &str) -> Option<&str> {
    media_ref
        .strip_prefix("feishu:image_key:")
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

/// Infer the MIME type of an image from its filename extension.
///
/// Supports: png, jpg/jpeg, gif, webp, bmp, tif/tiff, heic, heif.
/// Returns `None` for unknown extensions.
pub fn infer_image_mime_from_name(name: &str) -> Option<&'static str> {
    let ext = Path::new(name)
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase());
    match ext.as_deref() {
        Some("png") => Some("image/png"),
        Some("jpg") | Some("jpeg") => Some("image/jpeg"),
        Some("gif") => Some("image/gif"),
        Some("webp") => Some("image/webp"),
        Some("bmp") => Some("image/bmp"),
        Some("tif") | Some("tiff") => Some("image/tiff"),
        Some("heic") => Some("image/heic"),
        Some("heif") => Some("image/heif"),
        _ => None,
    }
}

/// Split text into chunks not exceeding `max_len` bytes.
///
/// Splits at newline boundaries when possible, then at space boundaries,
/// and finally at exact `max_len` (ensuring no multi-byte characters are
/// split via [`floor_char_boundary`]).
///
/// Useful for platforms like Feishu and Telegram that have per-message
/// byte limits.
pub fn split_text(text: &str, max_len: usize) -> Vec<String> {
    if text.len() <= max_len {
        return vec![text.to_string()];
    }
    let mut content = text.to_string();
    let mut chunks = Vec::new();
    while !content.is_empty() {
        if content.len() <= max_len {
            chunks.push(content);
            break;
        }
        let safe_end = floor_char_boundary(&content, max_len);
        let cut = &content[..safe_end];
        let mut pos = cut.rfind('\n').unwrap_or(0);
        if pos == 0 {
            pos = cut.rfind(' ').unwrap_or(safe_end);
        }
        if pos == 0 {
            pos = safe_end;
        }
        chunks.push(content[..pos].to_string());
        content = content[pos..].trim_start().to_string();
    }
    chunks
}

/// Serialize text into the Feishu text content JSON format.
///
/// Produces `{"text": "..."}` which is the content body used by Feishu
/// text messages in the IM API.
///
/// # Errors
/// Returns [`ChannelError::Adapter`] if serialization fails.
pub fn serialize_text_content(text: &str) -> ChannelResult<String> {
    serde_json::to_string(&FeishuTextContent {
        text: text.to_string(),
    })
    .map_err(|err| ChannelError::adapter("feishu", format!("serialize content failed: {err}")))
}

/// Walk backwards from `max_len` to find the nearest UTF-8 char boundary.
///
/// This ensures we never split a multi-byte character when slicing strings.
/// Used by [`split_text`] to produce valid UTF-8 chunks.
pub fn floor_char_boundary(input: &str, max_len: usize) -> usize {
    let mut boundary = max_len.min(input.len());
    while boundary > 0 && !input.is_char_boundary(boundary) {
        boundary -= 1;
    }
    boundary
}

/// Convert Markdown table syntax to plain-ASCII format for raw text mode.
///
/// Feishu raw text does not support Markdown tables.  This function converts
/// them to a simple pipe-free ASCII layout:
///
/// ```text
/// A | B
/// --- | ---
/// 1 | 2
/// ```
///
/// Non-table content passes through unchanged.
pub fn convert_markdown_tables(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut in_table = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('|') && trimmed.ends_with('|') {
            let cols: Vec<&str> = trimmed.split('|').filter(|c| !c.is_empty()).collect();
            if cols.is_empty() {
                continue;
            }
            let is_sep = cols
                .iter()
                .all(|c| c.trim().chars().all(|c| c == '-' || c == ':'));
            if is_sep {
                let dashes: Vec<&str> = std::iter::repeat_n("---", cols.len()).collect();
                if in_table {
                    result.push('\n');
                }
                result.push_str(&dashes.join(" | "));
                in_table = true;
                continue;
            }
            let cleaned = cols
                .iter()
                .map(|c| c.trim())
                .collect::<Vec<_>>()
                .join(" | ");
            if in_table {
                result.push('\n');
            }
            result.push_str(&cleaned);
            in_table = true;
        } else {
            if in_table {
                result.push('\n');
                in_table = false;
            } else if !result.is_empty() {
                result.push('\n');
            }
            result.push_str(line);
        }
    }
    result
}

/// Determine whether a `ChannelError` is a retryable auth failure.
///
/// Checks for HTTP 401/403 status codes, Feishu-specific error codes
/// (`99991661`, `99991663`), and messages containing "invalid tenant
/// access token" or "access token".
///
/// Used by the send/update methods to decide whether to refresh the
/// tenant token and retry once.
pub fn is_retryable_auth_send_error(err: &ChannelError) -> bool {
    let message = err.to_string().to_ascii_lowercase();
    message.contains("401")
        || message.contains("403")
        || message.contains("99991661")
        || message.contains("99991663")
        || message.contains("invalid tenant access token")
        || message.contains("access token")
}

/// Check if a Feishu API response body indicates success.
///
/// Handles two response formats:
/// - `{"code": 0, ...}` (modern Feishu API)
/// - `{"StatusCode": 0, ...}` (legacy API)
///
/// Falls back to `true` if neither field is present (best-effort).
pub fn is_success_response(body: &serde_json::Value) -> bool {
    if let Some(code) = body.get("code").and_then(|v| v.as_i64()) {
        return code == 0;
    }
    if let Some(code) = body.get("StatusCode").and_then(|v| v.as_i64()) {
        return code == 0;
    }
    true
}

/// Extract a human-readable error message from a Feishu API response body.
///
/// Checks for `msg`, `message`, or `StatusMessage` fields in order of
/// priority.  Falls back to the raw JSON representation.
pub fn error_message(body: &serde_json::Value) -> String {
    if let Some(v) = body
        .get("msg")
        .or_else(|| body.get("message"))
        .or_else(|| body.get("StatusMessage"))
        .and_then(|v| v.as_str())
    {
        return v.to_string();
    }
    body.to_string()
}
