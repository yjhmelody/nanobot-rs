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

const TELEGRAM_API_DEFAULT: &str = "https://api.telegram.org";
const TELEGRAM_TEXT_LIMIT: usize = 4000;
const LOG_TARGET: &str = "nanobot::channels::telegram";

#[derive(Debug, Serialize, Deserialize)]
struct TelegramUpdatesResponse {
    ok: bool,
    #[serde(default)]
    result: Vec<TelegramUpdate>,
}

#[derive(Debug, Serialize, Deserialize)]
struct TelegramUpdate {
    update_id: i64,
    message: Option<TelegramMessage>,
}

#[derive(Debug, Serialize, Deserialize)]
struct TelegramMessage {
    message_id: i64,
    from: Option<TelegramUser>,
    chat: TelegramChat,
    text: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct TelegramUser {
    id: i64,
}

#[derive(Debug, Serialize, Deserialize)]
struct TelegramChat {
    id: i64,
}

#[derive(Debug, Serialize, Deserialize)]
struct TelegramSendMessage {
    chat_id: i64,
    text: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct TelegramSendMessageResponse {
    ok: bool,
    result: TelegramMessage,
}

pub struct TelegramChannel {
    name: String,
    allow_from: Vec<String>,
    bus: MessageBus,
    client: Client,
    token: String,
    api_base: String,
    running: Arc<AtomicBool>,
    offset: Arc<AtomicI64>,
    poll_task: Mutex<Option<JoinHandle<()>>>,
}

impl TelegramChannel {
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

    async fn stop(&self) -> ChannelResult<()> {
        self.running.store(false, Ordering::Release);
        if let Some(task) = self.poll_task.lock().await.take() {
            task.abort();
        }
        info!(target: LOG_TARGET, name = %self.name, "stopped");
        Ok(())
    }

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

fn split_text(text: &str, limit: usize) -> Vec<String> {
    if text.len() <= limit {
        return vec![text.to_string()];
    }
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let end = (start + limit).min(text.len());
        // Try to break at char boundary
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
