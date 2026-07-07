//! Telegram Bot API channel adapter.
//!
//! This module implements the [`ChannelAdapter`] trait for the
//! [Telegram Bot API](https://core.telegram.org/bots/api).
//!
//! # Inbound Messages
//!
//! Inbound messages are received via **long polling** (`getUpdates`).
//! The adapter spawns a background task that polls the API with a
//! 20-second timeout, parses incoming text messages, and publishes
//! them to the shared [`MessageBus`].
//!
//! # Outbound Messages
//!
//! Outbound messages are sent via `sendMessage`.  Text longer than
//! `TELEGRAM_TEXT_LIMIT` bytes is automatically split into multiple
//! chunks.
//!
//! # Limitations
//!
//! - No streaming updates (Telegram does not support editing messages
//!   created by the bot in a way compatible with this streaming model).
//! - Only `text` message type is currently handled for inbound messages.
//! - Image/media attachments are not yet supported.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

use crate::base::{ChannelAdapter, SendOutcome, is_sender_allowed};
use crate::error::{ChannelError, ChannelResult};
use nanobot_bus::{InboundMessage, MessageBus, MessageMetadata, OutboundMessage};
use nanobot_config::schema::TelegramChannelConfig;

/// Default Telegram Bot API base URL.
const TELEGRAM_API_DEFAULT: &str = "https://api.telegram.org";
/// Maximum byte length for a single Telegram message (Telegram's limit is 4096).
const TELEGRAM_TEXT_LIMIT: usize = 4000;
const LOG_TARGET: &str = "nanobot::channels::telegram";

/// Response from the `getUpdates` endpoint.
#[derive(Debug, Serialize, Deserialize)]
struct TelegramUpdatesResponse {
    ok: bool,
    #[serde(default)]
    result: Vec<TelegramUpdate>,
}

/// A single update from the `getUpdates` long-poll endpoint.
#[derive(Debug, Serialize, Deserialize)]
struct TelegramUpdate {
    update_id: i64,
    message: Option<TelegramMessage>,
}

/// An incoming message from Telegram.
#[derive(Debug, Serialize, Deserialize)]
struct TelegramMessage {
    message_id: i64,
    from: Option<TelegramUser>,
    chat: TelegramChat,
    text: Option<String>,
}

/// A Telegram user (sender).
#[derive(Debug, Serialize, Deserialize)]
struct TelegramUser {
    id: i64,
}

/// A Telegram chat (conversation).
#[derive(Debug, Serialize, Deserialize)]
struct TelegramChat {
    id: i64,
}

/// Payload for the `sendMessage` API call.
#[derive(Debug, Serialize, Deserialize)]
struct TelegramSendMessage {
    chat_id: i64,
    text: String,
}

/// Response from the `sendMessage` endpoint.
#[derive(Debug, Serialize, Deserialize)]
struct TelegramSendMessageResponse {
    ok: bool,
    result: TelegramMessage,
}

/// Telegram Bot API channel adapter.
///
/// Implements [`ChannelAdapter`] using the Telegram Bot API's long-polling
/// model for inbound messages and the `sendMessage` method for outbound.
///
/// # State
/// - `running` — `Arc<AtomicBool>` shared with the polling background task.
/// - `offset` — `Arc<AtomicI64>` tracking the last processed `update_id`
///   for the `getUpdates` offset parameter.
/// - `poll_task` — Handle for the background polling task, stored in a
///   `tokio::sync::Mutex` because it's written once and read during shutdown.
pub struct TelegramChannel {
    /// Channel instance name (from config).
    name: String,
    /// Access-control list for inbound message filtering.
    allow_from: Vec<String>,
    /// Shared message bus for publishing inbound messages.
    bus: MessageBus,
    /// Reusable HTTP client for API calls.
    client: Client,
    /// Telegram bot token.
    token: String,
    /// Telegram Bot API base URL (defaults to `TELEGRAM_API_DEFAULT`).
    api_base: String,
    /// Whether the channel is currently running (shared with poll task).
    running: Arc<AtomicBool>,
    /// Last processed `update_id` (`AtomicI64` for lock-free access).
    offset: Arc<AtomicI64>,
    /// Handle for the long-polling background task.
    poll_task: Mutex<Option<JoinHandle<()>>>,
}

impl TelegramChannel {
    /// Construct a new `TelegramChannel` from configuration.
    ///
    /// # Errors
    /// Returns [`ChannelError::Config`] if the token is empty or blank.
    pub fn new(name: String, cfg: TelegramChannelConfig, bus: MessageBus) -> ChannelResult<Self> {
        let token = cfg.token;
        if token.trim().is_empty() {
            return Err(ChannelError::config(
                "telegram instance '{name}' has empty token",
            ));
        }

        let api_base = cfg
            .api_base
            .unwrap_or_else(|| TELEGRAM_API_DEFAULT.to_string());
        Ok(Self {
            name,
            allow_from: cfg.allow_from,
            bus,
            client: Client::new(),
            token,
            api_base,
            running: Arc::new(AtomicBool::new(false)),
            offset: Arc::new(AtomicI64::new(0)),
            poll_task: Mutex::new(None),
        })
    }

    /// Build the full URL for a Telegram Bot API method.
    ///
    /// Returns `{api_base}/bot{token}/{method}`.
    fn endpoint(&self, method: &str) -> String {
        format!(
            "{}/bot{}/{}",
            self.api_base.trim_end_matches('/'),
            self.token,
            method
        )
    }
}

#[async_trait]
impl ChannelAdapter for TelegramChannel {
    fn name(&self) -> &str {
        &self.name
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    /// Start the Telegram channel's long-polling loop.
    ///
    /// Spawns a background task that repeatedly calls `getUpdates` with
    /// a 20-second timeout and published inbound messages to the
    /// [`MessageBus`].  Updates are tracked via the `offset` parameter
    /// to avoid re-processing.
    ///
    /// # Errors
    /// Returns an error if the channel is already running (logged as a
    /// warning, returns `Ok`).
    async fn start(&self) -> ChannelResult<()> {
        if self.running.swap(true, Ordering::Release) {
            warn!(target: LOG_TARGET, name = %self.name, "already running");
            return Ok(());
        }
        info!(target: LOG_TARGET, name = %self.name, "starting");

        let allow_from = self.allow_from.clone();
        let token = self.token.clone();
        let api_base = self.api_base.clone();
        let bus = self.bus.clone();
        let name = self.name.clone();
        let running = self.running.clone();
        let offset = self.offset.clone();

        let handle = tokio::spawn(async move {
            let client = Client::new();
            loop {
                if !running.load(Ordering::Acquire) {
                    break;
                }

                let offset_val = offset.load(Ordering::Acquire);
                let url = format!(
                    "{}/bot{}/getUpdates?offset={}&timeout={}",
                    api_base.trim_end_matches('/'),
                    token,
                    offset_val,
                    20
                );

                match client.get(&url).send().await {
                    Ok(resp) => match resp.json::<TelegramUpdatesResponse>().await {
                        Ok(updates) => {
                            for update in updates.result {
                                let new_offset = update.update_id + 1;
                                offset.store(new_offset, Ordering::Release);

                                if let Some(msg) = update.message {
                                    if !is_sender_allowed(&allow_from, &msg.chat.id.to_string()) {
                                        continue;
                                    }

                                    let text = msg.text.unwrap_or_default();
                                    if text.trim().is_empty() {
                                        continue;
                                    }

                                    let _ = bus.publish_inbound(InboundMessage {
                                        channel: name.clone(),
                                        sender_id: msg
                                            .from
                                            .map(|u| u.id.to_string())
                                            .unwrap_or_default(),
                                        chat_id: msg.chat.id.to_string(),
                                        content: text.into(),
                                        timestamp: chrono::Utc::now(),
                                        media: Vec::new(),
                                        metadata: MessageMetadata::default(),
                                        session_key_override: None,
                                    });
                                }
                            }
                        }
                        Err(e) => {
                            error!(target: LOG_TARGET, name = %name, "failed to parse updates: {}", e);
                        }
                    },
                    Err(e) => {
                        if !e.is_timeout() {
                            error!(target: LOG_TARGET, name = %name, "poll error: {}", e);
                        }
                    }
                }
            }
            info!(target: LOG_TARGET, name = %name, "stopped");
        });

        *self.poll_task.lock().await = Some(handle);
        Ok(())
    }

    /// Stop the Telegram channel.
    ///
    /// Sets the running flag and aborts the polling background task.
    async fn stop(&self) -> ChannelResult<()> {
        self.running.store(false, Ordering::Release);
        if let Some(task) = self.poll_task.lock().await.take() {
            task.abort();
        }
        info!(target: LOG_TARGET, name = %self.name, "stopped");
        Ok(())
    }

    /// Send an outbound text message to a Telegram chat.
    ///
    /// Splits text longer than `TELEGRAM_TEXT_LIMIT` bytes into multiple
    /// chunks (at UTF-8 character boundaries) and sends each as a separate
    /// message.
    ///
    /// # Errors
    /// Returns [`ChannelError::Adapter`] if:
    /// - The `chat_id` is not a valid integer (Telegram uses numeric IDs).
    /// - The API request or response parsing fails.
    async fn send(&self, msg: OutboundMessage) -> ChannelResult<SendOutcome> {
        let chat_id: i64 = msg.chat_id.parse().map_err(|e| {
            ChannelError::adapter(
                &self.name,
                format!("invalid chat_id '{}': {}", msg.chat_id, e),
            )
        })?;

        for chunk in split_text(&msg.content, TELEGRAM_TEXT_LIMIT) {
            let payload = TelegramSendMessage {
                chat_id,
                text: chunk,
            };

            let resp = self
                .client
                .post(self.endpoint("sendMessage"))
                .json(&payload)
                .send()
                .await
                .map_err(|e| {
                    ChannelError::adapter(&self.name, format!("telegram send error: {}", e))
                })?;

            let body: TelegramSendMessageResponse = resp.json().await.map_err(|e| {
                ChannelError::adapter(&self.name, format!("telegram send response error: {}", e))
            })?;

            if !body.ok {
                error!(
                    target: LOG_TARGET,
                    name = %self.name,
                    chat_id = %msg.chat_id,
                    "telegram API returned ok=false"
                );
            }
        }

        Ok(SendOutcome { message_id: None })
    }
}

/// Split text into chunks not exceeding `limit` bytes, respecting UTF-8 char boundaries.
///
/// This is a simpler splitter than the Feishu equivalent (no newline/space
/// preference) because Telegram's `sendMessage` handles rendering; we just
/// need to stay under the message size limit.
fn split_text(text: &str, limit: usize) -> Vec<String> {
    if text.len() <= limit {
        return vec![text.to_string()];
    }
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let end = (start + limit).min(text.len());
        // Walk back to a UTF-8 char boundary to avoid splitting a multi-byte character.
        let end = if !text.is_char_boundary(end) {
            let mut bound = end;
            while bound > start && !text.is_char_boundary(bound) {
                bound -= 1;
            }
            if bound == start { end } else { bound }
        } else {
            end
        };
        chunks.push(text[start..end].to_string());
        start = end;
    }
    chunks
}
