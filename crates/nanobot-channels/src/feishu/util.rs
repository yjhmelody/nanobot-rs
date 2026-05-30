//! Utility functions for the feishu channel adapter.

use std::path::Path;

use crate::base::is_sender_allowed;
use crate::error::{ChannelError, ChannelResult};
use crate::feishu::types::*;

use base64::Engine;
use hmac::{Hmac, Mac};
use nanobot_bus::{InboundMessage, MessageId, MessageMetadata};
use nanobot_config::schema::FeishuChannelConfig;
type HmacSha256 = Hmac<sha2::Sha256>;

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

pub fn build_signature(timestamp: &str, secret: &str) -> ChannelResult<String> {
    let string_to_sign = format!("{}\n{}", timestamp, secret);
    let mut mac = HmacSha256::new_from_slice(string_to_sign.as_bytes()).map_err(|err| {
        ChannelError::adapter("feishu", format!("failed to build signature key: {}", err))
    })?;
    mac.update(&[]);
    Ok(base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes()))
}

pub fn normalize_path(path: &str) -> String {
    if path.is_empty() || path == "/" {
        return "/feishu/events".to_string();
    }
    if path.starts_with('/') {
        return path.to_string();
    }
    format!("/{}", path)
}

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

pub fn extract_feishu_image_key_ref(media_ref: &str) -> Option<&str> {
    media_ref
        .strip_prefix("feishu:image_key:")
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

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

pub fn serialize_text_content(text: &str) -> ChannelResult<String> {
    serde_json::to_string(&FeishuTextContent {
        text: text.to_string(),
    })
    .map_err(|err| ChannelError::adapter("feishu", format!("serialize content failed: {err}")))
}

pub fn floor_char_boundary(input: &str, max_len: usize) -> usize {
    let mut boundary = max_len.min(input.len());
    while boundary > 0 && !input.is_char_boundary(boundary) {
        boundary -= 1;
    }
    boundary
}

pub fn is_retryable_auth_send_error(err: &ChannelError) -> bool {
    let message = err.to_string().to_ascii_lowercase();
    message.contains("401")
        || message.contains("403")
        || message.contains("99991661")
        || message.contains("99991663")
        || message.contains("invalid tenant access token")
        || message.contains("access token")
}

pub fn is_success_response(body: &serde_json::Value) -> bool {
    if let Some(code) = body.get("code").and_then(|v| v.as_i64()) {
        return code == 0;
    }
    if let Some(code) = body.get("StatusCode").and_then(|v| v.as_i64()) {
        return code == 0;
    }
    true
}

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
