use std::collections::HashMap;
use std::sync::Arc;

use crate::error::{NanobotError, Result};
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
            let mut stream_registry: HashMap<String, String> = HashMap::new();
            loop {
                let Ok(msg) = outbound_rx.recv().await else {
                    continue;
                };
                if !should_deliver(&msg, send_progress, send_tool_hints) {
                    continue;
                }
                if let Some(channel) = channels.get(&msg.channel) {
                    if let Err(err) =
                        dispatch_outbound(channel.as_ref(), &mut stream_registry, msg).await
                    {
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
        return Err(NanobotError::config(format!(
            "\"{}\" has empty allowFrom (denies all). set [\"*\"] or explicit ids",
            name
        )));
    }
    let mut has_valid = false;
    let mut has_wildcard = false;
    for entry in &cfg.allow_from {
        if entry.trim().is_empty() {
            return Err(NanobotError::config(format!(
                "\"{}\" has empty allowFrom entry. remove empty strings",
                name
            )));
        }
        if entry.trim() != entry {
            return Err(NanobotError::config(format!(
                "\"{}\" has allowFrom entry with leading/trailing whitespace: '{}'",
                name, entry
            )));
        }
        if entry == "*" {
            has_wildcard = true;
        }
        has_valid = true;
    }
    if has_wildcard && cfg.allow_from.len() > 1 {
        return Err(NanobotError::config(format!(
            "\"{}\" has allowFrom '*' alongside explicit ids. keep only '*' or explicit ids",
            name
        )));
    }
    if !has_valid {
        return Err(NanobotError::config(format!(
            "\"{}\" has no valid allowFrom entries",
            name
        )));
    }
    Ok(())
}

fn should_deliver(msg: &OutboundMessage, send_progress: bool, send_tool_hints: bool) -> bool {
    let Some(message_id) = msg.metadata.message_id.as_ref() else {
        return true;
    };

    // Progress/tool-hint delivery is toggled via message_id tags.
    if message_id.is_progress() {
        return send_progress;
    }
    if message_id.is_tool_hint() {
        return send_tool_hints;
    }
    true
}

async fn dispatch_outbound(
    channel: &dyn ChannelAdapter,
    stream_registry: &mut HashMap<String, String>,
    msg: OutboundMessage,
) -> crate::error::Result<()> {
    let is_tool_hint = msg
        .metadata
        .message_id
        .as_ref()
        .map(|id| id.is_tool_hint())
        .unwrap_or(false);
    let is_progress = msg
        .metadata
        .message_id
        .as_ref()
        .map(|id| id.is_progress())
        .unwrap_or(false);
    let stream_id = msg.metadata.stream_id.clone();

    if !is_tool_hint {
        if let Some(stream_id) = stream_id {
            let key = format!("{}:{}:{}", msg.channel, msg.chat_id, stream_id);
            if let Some(message_id) = stream_registry.get(&key).cloned() {
                if channel.supports_stream_updates() {
                    channel.update(&message_id, msg).await?;
                    if !is_progress {
                        stream_registry.remove(&key);
                    }
                    return Ok(());
                }
            } else if is_progress && channel.supports_stream_updates() {
                let outcome = channel.send(msg).await?;
                if let Some(sent_id) = outcome.message_id {
                    stream_registry.insert(key, sent_id);
                }
                return Ok(());
            }
        }
    }

    let _ = channel.send(msg).await?;
    Ok(())
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

    #[test]
    fn manager_rejects_blank_allow_from_entries() {
        let mut cfg = Config::default();
        cfg.channels.telegram.enabled = true;
        cfg.channels.telegram.allow_from = vec![" ".to_string()];
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
                .contains("empty allowFrom entry")
        );
    }

    #[test]
    fn manager_rejects_allow_from_with_whitespace() {
        let mut cfg = Config::default();
        cfg.channels.telegram.enabled = true;
        cfg.channels.telegram.allow_from = vec![" 123 ".to_string()];
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
                .contains("leading/trailing whitespace")
        );
    }

    #[test]
    fn manager_rejects_wildcard_with_explicit_ids() {
        let mut cfg = Config::default();
        cfg.channels.telegram.enabled = true;
        cfg.channels.telegram.allow_from = vec!["*".to_string(), "123".to_string()];
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
                .contains("alongside explicit ids")
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
