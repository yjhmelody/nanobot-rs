use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Result, bail};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

use crate::bus::{MessageBus, OutboundMessage};
use crate::channels::base::ChannelAdapter;
use crate::channels::cli::CliChannel;
use crate::channels::placeholder::PlaceholderChannel;
use crate::channels::telegram::TelegramChannel;
use crate::config::schema::{ChannelsConfig, GenericChannelConfig};
use crate::observability::TARGET_CHANNELS;

pub struct ChannelManager {
    config: ChannelsConfig,
    bus: MessageBus,
    channels: HashMap<String, Arc<dyn ChannelAdapter>>,
    dispatch_task: Mutex<Option<JoinHandle<()>>>,
}

impl ChannelManager {
    pub fn new(config: ChannelsConfig, bus: MessageBus) -> Result<Self> {
        let mut channels: HashMap<String, Arc<dyn ChannelAdapter>> = HashMap::new();
        channels.insert("cli".to_string(), Arc::new(CliChannel::new()));

        if config.telegram.enabled {
            validate_allow_from("telegram", &config.telegram)?;
            let tg = TelegramChannel::new(config.telegram.clone(), bus.clone())?;
            channels.insert("telegram".to_string(), Arc::new(tg));
        }

        for (name, cfg) in [("discord", &config.discord), ("feishu", &config.feishu)] {
            if cfg.enabled {
                validate_allow_from(name, cfg)?;
                channels.insert(name.to_string(), Arc::new(PlaceholderChannel::new(name)));
            }
        }

        Ok(Self {
            config,
            bus,
            channels,
            dispatch_task: Mutex::new(None),
        })
    }

    pub async fn start_all(&self) -> Result<()> {
        for (name, ch) in &self.channels {
            if let Err(err) = ch.start().await {
                error!(
                    target: TARGET_CHANNELS,
                    "failed to start channel '{}': {}",
                    name,
                    err
                );
            }
        }

        let bus = self.bus.clone();
        let channels = self.channels.clone();
        let send_progress = self.config.send_progress;
        let send_tool_hints = self.config.send_tool_hints;

        let handle = tokio::spawn(async move {
            info!(target: TARGET_CHANNELS, "outbound dispatcher started");
            let mut outbound_rx = bus.subscribe_outbound();
            loop {
                let Ok(msg) = outbound_rx.recv().await else {
                    continue;
                };
                if !should_deliver(&msg, send_progress, send_tool_hints) {
                    continue;
                }
                if let Some(channel) = channels.get(&msg.channel) {
                    if let Err(err) = channel.send(msg).await {
                        error!(
                            target: TARGET_CHANNELS,
                            "failed to send outbound via '{}': {}",
                            channel.name(),
                            err
                        );
                    }
                } else {
                    warn!(target: TARGET_CHANNELS, "unknown channel '{}'", msg.channel);
                }
            }
        });
        *self.dispatch_task.lock().await = Some(handle);
        Ok(())
    }

    pub async fn stop_all(&self) {
        if let Some(task) = self.dispatch_task.lock().await.take() {
            task.abort();
        }
        for (name, ch) in &self.channels {
            if let Err(err) = ch.stop().await {
                error!(
                    target: TARGET_CHANNELS,
                    "failed to stop channel '{}': {}",
                    name,
                    err
                );
            }
        }
    }

    pub fn enabled_channels(&self) -> Vec<String> {
        self.channels.keys().cloned().collect()
    }

    pub fn status(&self) -> HashMap<String, bool> {
        self.channels
            .iter()
            .map(|(name, c)| (name.clone(), c.is_running()))
            .collect()
    }
}

fn validate_allow_from(name: &str, cfg: &GenericChannelConfig) -> Result<()> {
    if cfg.allow_from.is_empty() {
        bail!(
            "\"{}\" has empty allowFrom (denies all). set [\"*\"] or explicit ids",
            name
        );
    }
    Ok(())
}

fn should_deliver(msg: &OutboundMessage, send_progress: bool, send_tool_hints: bool) -> bool {
    let Some(raw_message_id) = msg.metadata.message_id.as_deref() else {
        return true;
    };

    // Progress/tool-hint delivery is toggled via message_id tags.
    if raw_message_id == "__progress__" {
        return send_progress;
    }
    if raw_message_id == "__tool_hint__" {
        return send_tool_hints;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::config::schema::Config;

    #[test]
    fn manager_rejects_empty_allow_from_for_enabled_channel() {
        let mut cfg = Config::default();
        cfg.channels.telegram.enabled = true;
        cfg.channels.telegram.allow_from = Vec::new();
        cfg.channels.telegram.extra.insert(
            "token".to_string(),
            serde_json::Value::String("x".to_string()),
        );

        let bus = MessageBus::new();
        let out = ChannelManager::new(cfg.channels, bus);
        assert!(out.is_err());
        assert!(
            out.err()
                .map(|e| e.to_string())
                .unwrap_or_default()
                .contains("empty allowFrom")
        );
    }

    #[tokio::test]
    async fn manager_dispatches_to_cli_channel() {
        let cfg = ChannelsConfig::default();
        let bus = MessageBus::new();
        let manager = ChannelManager::new(cfg, bus.clone()).expect("manager new");
        manager.start_all().await.expect("manager start");

        // Give the dispatcher task time to subscribe
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        let publish = bus.publish_outbound(OutboundMessage {
            channel: "cli".to_string(),
            chat_id: "direct".to_string(),
            content: "hello".to_string(),
            reply_to: None,
            media: Vec::new(),
            metadata: crate::bus::MessageMetadata::default(),
        });
        assert!(publish.is_ok());

        manager.stop_all().await;
    }
}
