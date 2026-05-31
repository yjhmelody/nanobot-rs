//! Interactive card message builder for Feishu.
//!
//! Converts text into Feishu interactive card JSON with lark_md rendering.

use serde_json::json;

use crate::error::{ChannelError, ChannelResult};

const CARD_TITLE_MAX: usize = 100;

/// Build the content JSON string for an interactive card message (IM API mode).
pub fn build_card_content(text: &str) -> ChannelResult<String> {
    let title = extract_title(text);
    let card = json!({
        "config": { "wide_screen_mode": true },
        "header": {
            "title": { "tag": "plain_text", "content": title }
        },
        "elements": [
            { "tag": "markdown", "content": text }
        ]
    });
    serde_json::to_string(&card)
        .map_err(|e| ChannelError::adapter("feishu", format!("serialize card failed: {e}")))
}

/// Build card JSON value for webhook mode (no header).
pub fn build_webhook_card_content(text: &str) -> ChannelResult<serde_json::Value> {
    let card = json!({
        "config": { "wide_screen_mode": true },
        "elements": [
            { "tag": "markdown", "content": text }
        ]
    });
    Ok(card)
}

fn extract_title(text: &str) -> String {
    text.lines()
        .find(|l| !l.trim().is_empty())
        .map(|l| {
            let clean = l.trim().trim_start_matches('#').trim();
            clean.chars().take(CARD_TITLE_MAX).collect()
        })
        .unwrap_or_default()
}
