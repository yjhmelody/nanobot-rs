//! Interactive card message builder for Feishu.
//!
//! Converts plain text into the Feishu interactive card JSON format,
//! using `lark_md` for rich markdown rendering.
//!
//! Two entry points are provided:
//!
//! - [`build_card_content`] — for the IM API (`send_interactive_by_app`).
//!   Produces a card with a header (first non-empty line) and markdown body.
//! - [`build_webhook_card_content`] — for webhook mode.  Produces a
//!   header-less card, as webhooks cannot set a custom header.

use serde_json::json;

use crate::error::{ChannelError, ChannelResult};

/// Maximum length (in chars) for the card title extracted from the first line.
const CARD_TITLE_MAX: usize = 100;

/// Build the content JSON **string** for an interactive card message (IM API mode).
///
/// The card structure:
/// - `config.wide_screen_mode`: `true`
/// - `header`: title extracted from the first non-empty line (truncated to
///   `CARD_TITLE_MAX` characters).
/// - `elements[0]`: a markdown block with the full text.
///
/// # Errors
/// Returns `ChannelError::Adapter` if JSON serialization fails (should not
/// happen in practice since the input is always valid UTF-8).
///
/// # Examples
/// ```
/// # use nanobot_channels::feishu::card::build_card_content;
/// let json = build_card_content("## Hello\nWorld").unwrap();
/// assert!(json.contains("wide_screen_mode"));
/// assert!(json.contains("Hello"));
/// ```
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

/// Build card JSON **value** for webhook mode (no header).
///
/// Webhook mode does not support setting a per-message header; the card
/// title is determined by the webhook bot's config.
///
/// # Errors
/// Currently infallible (returns `Ok`), but returns a `Result` for
/// consistency with [`build_card_content`].
///
/// # Examples
/// ```
/// # use nanobot_channels::feishu::card::build_webhook_card_content;
/// let v = build_webhook_card_content("**bold**").unwrap();
/// assert!(v.get("elements").is_some());
/// ```
pub fn build_webhook_card_content(text: &str) -> ChannelResult<serde_json::Value> {
    let card = json!({
        "config": { "wide_screen_mode": true },
        "elements": [
            { "tag": "markdown", "content": text }
        ]
    });
    Ok(card)
}

/// Extracts the card title from the first non-empty line of text.
///
/// Removes leading `#` characters (markdown heading markers) and whitespace,
/// then truncates to [`CARD_TITLE_MAX`] characters.  Returns an empty string
/// if the text is blank.
fn extract_title(text: &str) -> String {
    // Iterate through lines to find the first non-whitespace line.
    text.lines()
        .find(|l| !l.trim().is_empty())
        .map(|l| {
            let clean = l.trim().trim_start_matches('#').trim();
            clean.chars().take(CARD_TITLE_MAX).collect()
        })
        .unwrap_or_default()
}
