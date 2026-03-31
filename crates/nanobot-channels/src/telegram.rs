use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

use crate::LOG_TARGET;
use crate::base::{ChannelAdapter, SendOutcome, is_sender_allowed};
use crate::error::{ChannelError, ChannelResult};
use nanobot_bus::{InboundMessage, MessageBus, MessageId, MessageMetadata, OutboundMessage};
use nanobot_config::schema::GenericChannelConfig;

const TELEGRAM_API_DEFAULT: &str = "https://api.telegram.org";
const TELEGRAM_TEXT_LIMIT: usize = 4000;

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
struct TelegramSendChatAction {
    chat_id: i64,
    action: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct TelegramEditMessageText {
    chat_id: i64,
    message_id: i64,
    text: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct TelegramSendMessageResponse {
    ok: bool,
    result: TelegramMessage,
}

pub struct TelegramChannel {
    config: GenericChannelConfig,
    bus: MessageBus,
    client: Client,
    token: String,
    api_base: String,
    receive_ack: bool,
    running: Arc<AtomicBool>,
    offset: Arc<AtomicI64>,
    poll_task: Mutex<Option<JoinHandle<()>>>,
}

impl TelegramChannel {
    pub fn new(config: GenericChannelConfig, bus: MessageBus) -> ChannelResult<Self> {
        let token = extra_string(&config, &["token", "botToken"])
            .ok_or_else(|| ChannelError::config("telegram.token is required"))?;
        if token.trim().is_empty() {
            return Err(ChannelError::config("telegram.token is empty"));
        }

        let api_base =
            extra_string(&config, &["apiBase"]).unwrap_or_else(|| TELEGRAM_API_DEFAULT.to_string());
        let receive_ack = extra_bool(&config, &["receiveAck"]).unwrap_or(false);
        Ok(Self {
            config,
            bus,
            client: Client::new(),
            token,
            api_base,
            receive_ack,
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
        "telegram"
    }

    async fn start(&self) -> ChannelResult<()> {
        if self.running.swap(true, Ordering::SeqCst) {
            return Ok(());
        }

        let running = self.running.clone();
        let offset = self.offset.clone();
        let client = self.client.clone();
        let bus = self.bus.clone();
        let allow_from = self.config.allow_from.clone();
        let get_updates_url = self.endpoint("getUpdates");
        let send_action_url = self.endpoint("sendChatAction");
        let receive_ack = self.receive_ack;

        let handle = tokio::spawn(async move {
            while running.load(Ordering::SeqCst) {
                let next_offset = offset.load(Ordering::SeqCst).saturating_add(1);
                let request = client.get(&get_updates_url).query(&[
                    ("timeout", "20"),
                    ("offset", next_offset.to_string().as_str()),
                ]);

                let response = match request.send().await {
                    Ok(v) => v,
                    Err(err) => {
                        warn!(
                            target: LOG_TARGET,
                            "telegram getUpdates request failed: {}",
                            err
                        );
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                        continue;
                    }
                };

                let payload: TelegramUpdatesResponse = match response.json().await {
                    Ok(v) => v,
                    Err(err) => {
                        warn!(
                            target: LOG_TARGET,
                            "telegram getUpdates parse failed: {}",
                            err
                        );
                        continue;
                    }
                };

                for update in payload.result {
                    offset.store(update.update_id, Ordering::SeqCst);
                    let Some(message) = update.message else {
                        continue;
                    };
                    let Some(text) = message.text else {
                        continue;
                    };
                    let Some(from) = message.from else {
                        continue;
                    };

                    let sender = from.id.to_string();
                    if !is_sender_allowed(&allow_from, &sender) {
                        continue;
                    }

                    if receive_ack {
                        let _ = client
                            .post(&send_action_url)
                            .json(&TelegramSendChatAction {
                                chat_id: message.chat.id,
                                action: "typing".to_string(),
                            })
                            .send()
                            .await;
                    }

                    let inbound = InboundMessage {
                        channel: "telegram".to_string(),
                        sender_id: sender,
                        chat_id: message.chat.id.to_string(),
                        content: text.into(),
                        timestamp: chrono::Utc::now(),
                        media: Vec::new(),
                        metadata: MessageMetadata {
                            message_id: Some(MessageId::External(message.message_id.to_string())),
                            stream_id: None,
                        },
                        session_key_override: None,
                    };
                    if let Err(err) = bus.publish_inbound(inbound) {
                        error!(
                            target: LOG_TARGET,
                            "telegram publish inbound failed: {}",
                            err
                        );
                    }
                }
            }
        });

        *self.poll_task.lock().await = Some(handle);
        info!(target: LOG_TARGET, "telegram channel started");
        Ok(())
    }

    async fn stop(&self) -> ChannelResult<()> {
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.poll_task.lock().await.take() {
            handle.abort();
        }
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> ChannelResult<SendOutcome> {
        let chat_id = msg.chat_id.parse::<i64>().map_err(|_| {
            ChannelError::adapter("telegram", format!("invalid chat_id '{}'", msg.chat_id))
        })?;
        let chunks = split_text(&msg.content, TELEGRAM_TEXT_LIMIT);
        let mut last_message_id: Option<i64> = None;
        for chunk in chunks {
            let response = self
                .client
                .post(self.endpoint("sendMessage"))
                .json(&TelegramSendMessage {
                    chat_id,
                    text: chunk,
                })
                .send()
                .await
                .map_err(|err| {
                    ChannelError::adapter("telegram", format!("sendMessage failed: {}", err))
                })?;
            if let Ok(payload) = response.json::<TelegramSendMessageResponse>().await {
                if payload.ok {
                    last_message_id = Some(payload.result.message_id);
                }
            }
        }
        Ok(SendOutcome {
            message_id: last_message_id.map(|id| id.to_string()),
        })
    }

    async fn update(&self, message_id: &str, msg: OutboundMessage) -> ChannelResult<()> {
        let chat_id = msg.chat_id.parse::<i64>().map_err(|_| {
            ChannelError::adapter("telegram", format!("invalid chat_id '{}'", msg.chat_id))
        })?;
        let message_id = message_id.parse::<i64>().map_err(|_| {
            ChannelError::adapter("telegram", format!("invalid message_id '{}'", message_id))
        })?;
        let text = truncate_text(&msg.content, TELEGRAM_TEXT_LIMIT);
        self.client
            .post(self.endpoint("editMessageText"))
            .json(&TelegramEditMessageText {
                chat_id,
                message_id,
                text,
            })
            .send()
            .await
            .map_err(|err| {
                ChannelError::adapter("telegram", format!("editMessageText failed: {}", err))
            })?;
        Ok(())
    }

    fn supports_stream_updates(&self) -> bool {
        true
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

fn split_text(text: &str, max_len: usize) -> Vec<String> {
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
        let cut = &content[..max_len];
        let mut pos = cut.rfind('\n').unwrap_or(0);
        if pos == 0 {
            pos = cut.rfind(' ').unwrap_or(max_len);
        }
        chunks.push(content[..pos].to_string());
        content = content[pos..].trim_start().to_string();
    }
    chunks
}

fn truncate_text(text: &str, max_len: usize) -> String {
    if text.len() <= max_len {
        return text.to_string();
    }
    let mut out = text.to_string();
    out.truncate(max_len);
    out
}

fn extra_string(cfg: &GenericChannelConfig, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(v) = cfg.extra.get(*key).and_then(|v| v.as_str()) {
            return Some(v.to_string());
        }
    }
    None
}

fn extra_bool(cfg: &GenericChannelConfig, keys: &[&str]) -> Option<bool> {
    for key in keys {
        if let Some(v) = cfg.extra.get(*key) {
            if let Some(value) = v.as_bool() {
                return Some(value);
            }
            if let Some(value) = v.as_str() {
                let normalized = value.trim().to_ascii_lowercase();
                match normalized.as_str() {
                    "true" | "1" | "yes" | "y" | "on" => return Some(true),
                    "false" | "0" | "no" | "n" | "off" => return Some(false),
                    _ => {}
                }
            }
        }
    }
    None
}
