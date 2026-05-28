use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

use crate::base::ChannelAdapter;
use crate::cli::CliChannel;
use crate::error::{ChannelError, ChannelResult};
#[cfg(feature = "channel-feishu")]
use crate::feishu::FeishuChannel;
#[cfg(feature = "channel-discord")]
use crate::placeholder::PlaceholderChannel;
#[cfg(feature = "channel-telegram")]
use crate::telegram::TelegramChannel;
use nanobot_bus::{MessageBus, OutboundMessage};
use nanobot_config::schema::GenericChannelConfig;
use nanobot_config::schema::{ChannelsConfig, StreamMode};

const LOG_TARGET: &str = "nanobot::channels::manager";

type ChannelFactory =
    fn(GenericChannelConfig, MessageBus) -> ChannelResult<Arc<dyn ChannelAdapter>>;

struct ChannelRegistration {
    key: &'static str,
    aliases: &'static [&'static str],
    config: fn(&ChannelsConfig) -> &GenericChannelConfig,
    compiled_in: bool,
    factory: Option<ChannelFactory>,
}

pub struct ChannelManager {
    config: ChannelsConfig,
    bus: MessageBus,
    channels: HashMap<String, Arc<dyn ChannelAdapter>>,
    dispatch_task: Mutex<Option<JoinHandle<()>>>,
}

impl ChannelManager {
    pub fn new(config: ChannelsConfig, bus: MessageBus) -> ChannelResult<Self> {
        let mut channels: HashMap<String, Arc<dyn ChannelAdapter>> = HashMap::new();
        channels.insert("cli".to_string(), Arc::new(CliChannel::new()));

        for registration in channel_registrations() {
            let cfg = (registration.config)(&config);
            if !cfg.enabled {
                continue;
            }
            validate_allow_from(registration.key, cfg)?;
            if !registration.compiled_in {
                return Err(ChannelError::config(format!(
                    "{} channel is enabled in config but not compiled in; rebuild with feature 'channel-{}'",
                    registration.key, registration.key
                )));
            }
            let Some(factory) = registration.factory else {
                return Err(ChannelError::config(format!(
                    "{} channel is enabled but has no registered factory",
                    registration.key
                )));
            };
            channels.insert(
                registration.key.to_string(),
                factory(cfg.clone(), bus.clone())?,
            );
        }

        Ok(Self {
            config,
            bus,
            channels,
            dispatch_task: Mutex::new(None),
        })
    }

    pub async fn start_all(&self) -> ChannelResult<()> {
        for (name, ch) in &self.channels {
            if let Err(err) = ch.start().await {
                error!(
                    target: LOG_TARGET,
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
        let stream_mode = self.config.stream_mode;

        let handle = tokio::spawn(async move {
            info!(target: LOG_TARGET, "outbound dispatcher started");
            let mut outbound_rx = bus.subscribe_outbound();
            let mut stream_registry: HashMap<String, String> = HashMap::new();
            loop {
                let Ok(msg) = outbound_rx.recv().await else {
                    continue;
                };
                if !should_deliver(&msg, send_progress, send_tool_hints) {
                    continue;
                }
                let channel_name = canonical_channel_name(&msg.channel);
                if let Some(channel) = channels.get(channel_name) {
                    if let Err(err) =
                        dispatch_outbound(channel.as_ref(), &mut stream_registry, msg, stream_mode)
                            .await
                    {
                        error!(
                            target: LOG_TARGET,
                            "failed to send outbound via '{}': {}",
                            channel.name(),
                            err
                        );
                    }
                } else {
                    warn!(target: LOG_TARGET, "unknown channel '{}'", msg.channel);
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
                    target: LOG_TARGET,
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
fn validate_allow_from(name: &str, cfg: &GenericChannelConfig) -> ChannelResult<()> {
    if cfg.allow_from.is_empty() {
        return Err(ChannelError::config(format!(
            "\"{}\" has empty allowFrom (denies all). set [\"*\"] or explicit ids",
            name
        )));
    }
    let mut has_valid = false;
    let mut has_wildcard = false;
    for entry in &cfg.allow_from {
        if entry.is_empty() {
            return Err(ChannelError::config(format!(
                "\"{}\" has empty allowFrom entry. remove empty strings",
                name
            )));
        }
        if entry.trim() != entry {
            return Err(ChannelError::config(format!(
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
        return Err(ChannelError::config(format!(
            "\"{}\" has allowFrom '*' alongside explicit ids. keep only '*' or explicit ids",
            name
        )));
    }
    if !has_valid {
        return Err(ChannelError::config(format!(
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
    if message_id.is_progress() {
        return send_progress;
    }
    if message_id.is_tool_hint() {
        return send_tool_hints;
    }
    true
}

fn canonical_channel_name(channel: &str) -> &str {
    for registration in channel_registrations() {
        if registration.key == channel || registration.aliases.contains(&channel)
        {
            return registration.key;
        }
    }
    channel
}

fn channel_registrations() -> Vec<ChannelRegistration> {
    vec![
        ChannelRegistration {
            key: "telegram",
            aliases: &[],
            config: |cfg| &cfg.telegram,
            compiled_in: cfg!(feature = "channel-telegram"),
            factory: telegram_factory(),
        },
        ChannelRegistration {
            key: "feishu",
            aliases: &["lark"],
            config: |cfg| &cfg.feishu,
            compiled_in: cfg!(feature = "channel-feishu"),
            factory: feishu_factory(),
        },
        ChannelRegistration {
            key: "discord",
            aliases: &[],
            config: |cfg| &cfg.discord,
            compiled_in: cfg!(feature = "channel-discord"),
            factory: discord_factory(),
        },
    ]
}

#[cfg(feature = "channel-telegram")]
fn build_telegram_channel(
    cfg: GenericChannelConfig,
    bus: MessageBus,
) -> ChannelResult<Arc<dyn ChannelAdapter>> {
    Ok(Arc::new(TelegramChannel::new(cfg, bus)?))
}

#[cfg(feature = "channel-telegram")]
fn telegram_factory() -> Option<ChannelFactory> {
    Some(build_telegram_channel)
}

#[cfg(not(feature = "channel-telegram"))]
fn telegram_factory() -> Option<ChannelFactory> {
    None
}

#[cfg(feature = "channel-feishu")]
fn build_feishu_channel(
    cfg: GenericChannelConfig,
    bus: MessageBus,
) -> ChannelResult<Arc<dyn ChannelAdapter>> {
    Ok(Arc::new(FeishuChannel::new(cfg, bus)?))
}

#[cfg(feature = "channel-feishu")]
fn feishu_factory() -> Option<ChannelFactory> {
    Some(build_feishu_channel)
}

#[cfg(not(feature = "channel-feishu"))]
fn feishu_factory() -> Option<ChannelFactory> {
    None
}

#[cfg(feature = "channel-discord")]
fn build_discord_channel(
    _cfg: GenericChannelConfig,
    _bus: MessageBus,
) -> ChannelResult<Arc<dyn ChannelAdapter>> {
    Ok(Arc::new(PlaceholderChannel::new("discord")))
}

#[cfg(feature = "channel-discord")]
fn discord_factory() -> Option<ChannelFactory> {
    Some(build_discord_channel)
}

#[cfg(not(feature = "channel-discord"))]
fn discord_factory() -> Option<ChannelFactory> {
    None
}

async fn dispatch_outbound(
    channel: &dyn ChannelAdapter,
    stream_registry: &mut HashMap<String, String>,
    msg: OutboundMessage,
    stream_mode: StreamMode,
) -> ChannelResult<()> {
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

    if !is_tool_hint
        && stream_mode != StreamMode::Append
        && let Some(stream_id) = stream_id
    {
        let key = format!("{}:{}:{}", msg.channel, msg.chat_id, stream_id);
        if let Some(message_id) = stream_registry.get(&key).cloned() {
            if channel.supports_stream_updates() {
                if stream_mode == StreamMode::UpdateProgress && !is_progress {
                    stream_registry.remove(&key);
                    let _ = channel.send(msg).await?;
                    return Ok(());
                }
                channel.update(&message_id, msg).await?;
                if !is_progress {
                    stream_registry.remove(&key);
                }
                return Ok(());
            }
        } else if is_progress && channel.supports_stream_updates() {
            let outcome = if let Some(outcome) = channel.begin_stream(&msg).await? {
                outcome
            } else {
                channel.send(msg).await?
            };
            if let Some(sent_id) = outcome.message_id {
                stream_registry.insert(key, sent_id);
            }
            return Ok(());
        }
    }

    let _ = channel.send(msg).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use nanobot_config::schema::Config;

    #[test]
    fn manager_rejects_empty_allow_from_for_enabled_channel() {
        let mut cfg = Config::default();
        cfg.channels.telegram.enabled = true;
        cfg.channels.telegram.allow_from = Vec::new();
        cfg.channels.telegram.extra.insert(
            "token".to_string(),
            serde_json::Value::String("x".to_string()),
        );

        let bus = nanobot_bus::MessageBus::new();
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

        let bus = nanobot_bus::MessageBus::new();
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

        let bus = nanobot_bus::MessageBus::new();
        let out = ChannelManager::new(cfg.channels, bus);
        assert!(out.is_err());
        assert!(
            out.err()
                .map(|e| e.to_string())
                .unwrap_or_default()
                .contains("alongside explicit ids")
        );
    }

    #[test]
    fn canonical_channel_maps_lark_alias() {
        assert_eq!(canonical_channel_name("lark"), "feishu");
        assert_eq!(canonical_channel_name("feishu"), "feishu");
        assert_eq!(canonical_channel_name("telegram"), "telegram");
    }

    #[test]
    fn manager_rejects_feishu_without_delivery_config() {
        let mut cfg = Config::default();
        cfg.channels.feishu.enabled = true;
        cfg.channels.feishu.allow_from = vec!["*".to_string()];

        let bus = nanobot_bus::MessageBus::new();
        let out = ChannelManager::new(cfg.channels, bus);
        assert!(out.is_err());
        assert!(
            out.err()
                .map(|e| e.to_string())
                .unwrap_or_default()
                .contains("requires either")
        );
    }

    #[cfg(not(feature = "channel-feishu"))]
    #[test]
    fn manager_rejects_enabled_feishu_when_feature_is_disabled() {
        let mut cfg = Config::default();
        cfg.channels.feishu.enabled = true;
        cfg.channels.feishu.allow_from = vec!["*".to_string()];

        let bus = nanobot_bus::MessageBus::new();
        let out = ChannelManager::new(cfg.channels, bus);
        assert!(out.is_err());
        assert!(
            out.err()
                .map(|e| e.to_string())
                .unwrap_or_default()
                .contains("not compiled in")
        );
    }

    #[tokio::test]
    async fn manager_dispatches_to_cli_channel() {
        let cfg = nanobot_config::schema::ChannelsConfig::default();
        let bus = nanobot_bus::MessageBus::new();
        let manager = ChannelManager::new(cfg, bus.clone()).expect("manager new");
        manager.start_all().await.expect("manager start");

        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        let publish = bus.publish_outbound(OutboundMessage {
            channel: "cli".to_string(),
            chat_id: "direct".to_string(),
            content: "hello".to_string(),
            reply_to: None,
            media: Vec::new(),
            metadata: nanobot_bus::MessageMetadata::default(),
        });
        assert!(publish.is_ok());

        manager.stop_all().await;
    }
}
