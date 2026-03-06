use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use reqwest::Client;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

use crate::bus::{InboundMessage, MessageBus, MessageMetadata, OutboundMessage};
use crate::channels::base::{ChannelAdapter, is_sender_allowed};
use crate::config::schema::GenericChannelConfig;
use crate::types::channels::{TelegramSendMessage, TelegramUpdatesResponse};

const TELEGRAM_API_DEFAULT: &str = "https://api.telegram.org";
const TELEGRAM_TEXT_LIMIT: usize = 4000;

pub struct TelegramChannel {
    config: GenericChannelConfig,
    bus: Arc<MessageBus>,
    client: Client,
    token: String,
    api_base: String,
    running: Arc<AtomicBool>,
    offset: Arc<AtomicI64>,
    poll_task: Mutex<Option<JoinHandle<()>>>,
}

impl TelegramChannel {
    pub fn new(config: GenericChannelConfig, bus: Arc<MessageBus>) -> Result<Self> {
        let token = extra_string(&config, &["token", "botToken"])
            .ok_or_else(|| anyhow::anyhow!("telegram.token is required"))?;
        if token.trim().is_empty() {
            bail!("telegram.token is empty");
        }

        let api_base =
            extra_string(&config, &["apiBase"]).unwrap_or_else(|| TELEGRAM_API_DEFAULT.to_string());
        Ok(Self {
            config,
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
        "telegram"
    }

    async fn start(&self) -> Result<()> {
        if self.running.swap(true, Ordering::SeqCst) {
            return Ok(());
        }

        let running = self.running.clone();
        let offset = self.offset.clone();
        let client = self.client.clone();
        let bus = self.bus.clone();
        let allow_from = self.config.allow_from.clone();
        let get_updates_url = self.endpoint("getUpdates");

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
                        warn!("telegram getUpdates request failed: {}", err);
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                        continue;
                    }
                };

                let payload: TelegramUpdatesResponse = match response.json().await {
                    Ok(v) => v,
                    Err(err) => {
                        warn!("telegram getUpdates decode failed: {}", err);
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                        continue;
                    }
                };

                if !payload.ok {
                    warn!("telegram getUpdates returned ok=false");
                    continue;
                }

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

                    let inbound = InboundMessage {
                        channel: "telegram".to_string(),
                        sender_id: sender,
                        chat_id: message.chat.id.to_string(),
                        content: text,
                        timestamp: chrono::Utc::now(),
                        media: Vec::new(),
                        metadata: MessageMetadata {
                            message_id: Some(message.message_id.to_string()),
                        },
                        session_key_override: None,
                    };
                    if let Err(err) = bus.publish_inbound(inbound) {
                        error!("telegram publish inbound failed: {}", err);
                    }
                }
            }
        });

        *self.poll_task.lock().await = Some(handle);
        info!("telegram channel started");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.poll_task.lock().await.take() {
            handle.abort();
        }
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        let chat_id = msg
            .chat_id
            .parse::<i64>()
            .with_context(|| format!("invalid telegram chat_id '{}'", msg.chat_id))?;

        for chunk in split_text(&msg.content, TELEGRAM_TEXT_LIMIT) {
            self.client
                .post(self.endpoint("sendMessage"))
                .json(&TelegramSendMessage {
                    chat_id,
                    text: chunk,
                })
                .send()
                .await
                .context("telegram sendMessage failed")?;
        }
        Ok(())
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

fn extra_string(cfg: &GenericChannelConfig, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(v) = cfg.extra.get(*key).and_then(|v| v.as_str()) {
            return Some(v.to_string());
        }
    }
    None
}
