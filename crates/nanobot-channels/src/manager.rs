//! Channel lifecycle manager: builds adapter instances, validates config,
//! and runs the outbound message dispatcher.
//!
//! # Overview
//!
//! [`ChannelManager`] is the central orchestrator for all channel adapters.
//! It performs four roles:
//!
//! 1. **Construction** — Reads [`ChannelsConfig`], creates adapter instances
//!    for each enabled channel (including the always-present CLI channel),
//!    validates their allow-from rules, and merges per-instance runtime
//!    settings with channel defaults.
//! 2. **Lifecycle** — [`start_all`](ChannelManager::start_all) launches
//!    every adapter's background listener and the shared outbound dispatcher
//!    task.  [`stop_all`](ChannelManager::stop_all) tears everything down.
//! 3. **Outbound routing** — The dispatcher listens on the shared
//!    [`MessageBus`] outbound channel, applies runtime filters (progress /
//!    tool-hint suppression), and delegates each message to the correct
//!    adapter via `dispatch_outbound`, which also handles streaming edits.
//! 4. **Introspection** — [`enabled_channels`](ChannelManager::enabled_channels)
//!    and [`status`](ChannelManager::status) provide runtime visibility.
//!
//! # Thread Safety
//!
//! The dispatch task is stored inside a `tokio::sync::Mutex` because it
//! is written once at startup and read during shutdown — both are rare
//! operations that cross `.await` points (not hot paths).

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
#[cfg(feature = "channel-telegram")]
use crate::telegram::TelegramChannel;
use nanobot_bus::{MessageBus, OutboundMessage};
use nanobot_config::schema::{ChannelDefaults, ChannelInstanceConfig, ChannelsConfig, StreamMode};

const LOG_TARGET: &str = "nanobot::channels::manager";

/// Per-instance resolved runtime settings (defaults merged with overrides).
///
/// Each channel instance can override the global defaults for progress
/// messages, tool-hint visibility, usage summaries, and streaming behavior.
#[derive(Debug, Clone)]
pub struct InstanceRuntimeConfig {
    /// Whether to deliver progress messages (e.g. "thinking...").
    pub send_progress: bool,
    /// Whether to deliver tool-hint messages.
    pub send_tool_hints: bool,
    /// Whether to deliver usage summary messages.
    pub send_usage_summary: bool,
    /// How streaming updates should be delivered.
    pub stream_mode: StreamMode,
}

/// Merges per-instance config overrides into the global defaults, producing
/// a resolved [`InstanceRuntimeConfig`] for one channel instance.
fn resolve_runtime(
    cfg: &ChannelInstanceConfig,
    defaults: &ChannelDefaults,
) -> InstanceRuntimeConfig {
    let (sp, sth, sus, sm) = match cfg {
        ChannelInstanceConfig::Telegram(c) => (
            c.send_progress,
            c.send_tool_hints,
            c.send_usage_summary,
            c.stream_mode,
        ),
        ChannelInstanceConfig::Feishu(c) => (
            c.send_progress,
            c.send_tool_hints,
            c.send_usage_summary,
            c.stream_mode,
        ),
    };
    InstanceRuntimeConfig {
        send_progress: sp.unwrap_or(defaults.send_progress),
        send_tool_hints: sth.unwrap_or(defaults.send_tool_hints),
        send_usage_summary: sus.unwrap_or(defaults.send_usage_summary),
        stream_mode: sm.unwrap_or(defaults.stream_mode),
    }
}

/// Central orchestrator for all channel adapters.
///
/// Owns the adapter instances, their runtime configs, and the outbound
/// dispatcher task handle.  Constructed once at startup and typically
/// wrapped in an `Arc` for shared access.
pub struct ChannelManager {
    bus: MessageBus,
    channels: HashMap<String, Arc<dyn ChannelAdapter>>,
    runtime_configs: HashMap<String, InstanceRuntimeConfig>,
    dispatch_task: Mutex<Option<JoinHandle<()>>>,
}

impl ChannelManager {
    /// Build a new `ChannelManager` from configuration.
    ///
    /// This validates all enabled channel configurations and constructs
    /// their adapters, but does **not** start any listeners or the
    /// dispatcher task (use [`start_all`](ChannelManager::start_all) for that).
    ///
    /// # Errors
    /// Returns [`ChannelError::Config`] if any enabled channel has an invalid
    /// allow-from list, missing credentials, or an incompatible configuration
    /// (e.g. Feishu app_id without app_secret).
    pub fn new(config: ChannelsConfig, bus: MessageBus) -> ChannelResult<Self> {
        let mut channels: HashMap<String, Arc<dyn ChannelAdapter>> = HashMap::new();
        let mut runtime_configs: HashMap<String, InstanceRuntimeConfig> = HashMap::new();

        // The CLI channel is always available.
        channels.insert("cli".to_string(), Arc::new(CliChannel::new()));

        // Build adapters for each explicitly configured channel instance.
        for (name, instance_cfg) in &config.instances {
            if !instance_cfg.enabled() {
                continue;
            }
            validate_allow_from(name, instance_cfg)?;
            let runtime = resolve_runtime(instance_cfg, &config.defaults);
            let adapter = build_adapter(name.clone(), instance_cfg.clone(), bus.clone())?;
            runtime_configs.insert(name.clone(), runtime);
            channels.insert(name.clone(), adapter);
        }

        Ok(Self {
            bus,
            channels,
            runtime_configs,
            dispatch_task: Mutex::new(None),
        })
    }

    /// Start all channel adapters and the outbound dispatcher.
    ///
    /// For each adapter, calls [`ChannelAdapter::start`] (which begins
    /// inbound listening). Then spawns a long-lived background task that
    /// subscribes to the outbound message channel on [`MessageBus`] and
    /// routes each message to the appropriate adapter.
    ///
    /// # Errors
    /// Errors from individual adapters are logged but do not abort the
    /// overall startup.  Returns `Ok` as long as the dispatcher task
    /// was spawned successfully.
    pub async fn start_all(&self) -> ChannelResult<()> {
        // Start each adapter's inbound listener.
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
        let runtime_configs = self.runtime_configs.clone();

        // Outbound dispatcher: runs forever until the manager is stopped.
        let handle = tokio::spawn(async move {
            info!(target: LOG_TARGET, "outbound dispatcher started");
            let mut outbound_rx = bus.subscribe_outbound();
            // Maps (channel:chat_id:stream_id) -> platform message_id for streaming edits.
            let mut stream_registry: HashMap<String, String> = HashMap::new();
            loop {
                let Ok(msg) = outbound_rx.recv().await else {
                    continue;
                };
                let channel_name = &msg.channel;

                // Per-instance runtime config controls filtering of progress / tool-hints.
                if let Some(runtime) = runtime_configs.get(channel_name)
                    && !should_deliver(&msg, runtime.send_progress, runtime.send_tool_hints)
                {
                    continue;
                }

                if let Some(channel) = channels.get(channel_name) {
                    let stream_mode = runtime_configs
                        .get(channel_name)
                        .map(|r| r.stream_mode)
                        .unwrap_or(StreamMode::UpdateAll);
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

    /// Stop all channel adapters and abort the outbound dispatcher.
    ///
    /// Signals each adapter to stop, then aborts the dispatcher task.
    /// Adapter stop errors are logged but do not abort the process.
    pub async fn stop_all(&self) {
        // Abort the outbound dispatcher first.
        if let Some(task) = self.dispatch_task.lock().await.take() {
            task.abort();
        }
        // Then stop each adapter.
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

    /// Returns the list of enabled channel names.
    pub fn enabled_channels(&self) -> Vec<String> {
        self.channels.keys().cloned().collect()
    }

    /// Returns the runtime status (running / stopped) of each channel.
    pub fn status(&self) -> HashMap<String, bool> {
        self.channels
            .iter()
            .map(|(name, c)| (name.clone(), c.is_running()))
            .collect()
    }
}

/// Validates the allow-from list for a channel instance.
///
/// Rules:
/// - Must not be empty (that would deny all senders).
/// - Each entry must be non-empty and free of leading/trailing whitespace.
/// - `"*"` (wildcard) must not appear alongside explicit IDs.
fn validate_allow_from(name: &str, cfg: &ChannelInstanceConfig) -> ChannelResult<()> {
    let allow_from = cfg.allow_from();
    if allow_from.is_empty() {
        return Err(ChannelError::config(format!(
            "\"{}\" has empty allowFrom (denies all). set [\"*\"] or explicit ids",
            name
        )));
    }
    let mut has_valid = false;
    let mut has_wildcard = false;
    for entry in allow_from {
        if entry.is_empty() {
            return Err(ChannelError::config(format!(
                "\"{}\" has empty allowFrom entry. remove empty strings",
                name
            )));
        }
        if entry.trim() != entry.as_str() {
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
    if has_wildcard && allow_from.len() > 1 {
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

/// Decides whether a particular outbound message should be delivered
/// based on runtime settings for progress and tool-hint suppression.
///
/// Messages without a `message_id` (plain responses) are always delivered.
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

/// Constructs the appropriate adapter for a channel instance config.
///
/// Feature-gated: the Telegram adapter is only available with
/// `channel-telegram`, and the Feishu adapter with `channel-feishu`.
/// The returned error message tells the user which feature flag is missing.
fn build_adapter(
    name: String,
    cfg: ChannelInstanceConfig,
    bus: MessageBus,
) -> ChannelResult<Arc<dyn ChannelAdapter>> {
    match cfg {
        ChannelInstanceConfig::Telegram(c) => {
            #[cfg(feature = "channel-telegram")]
            {
                Ok(Arc::new(TelegramChannel::new(name, c, bus)?))
            }
            #[cfg(not(feature = "channel-telegram"))]
            {
                let _ = (name, c, bus);
                Err(ChannelError::config(
                    "telegram channel is enabled but not compiled in; rebuild with feature 'channel-telegram'".to_string(),
                ))
            }
        }
        ChannelInstanceConfig::Feishu(c) => {
            #[cfg(feature = "channel-feishu")]
            {
                Ok(Arc::new(FeishuChannel::new(name, c, bus)?))
            }
            #[cfg(not(feature = "channel-feishu"))]
            {
                let _ = (name, c, bus);
                Err(ChannelError::config(
                    "feishu channel is enabled but not compiled in; rebuild with feature 'channel-feishu'".to_string(),
                ))
            }
        }
    }
}

/// Routes a single outbound message to a channel adapter, handling streaming.
///
/// Streaming logic (when `stream_mode != Append` and the message has a
/// `stream_id`):
/// - If we already have a message_id for this stream, call `update` (or
///   fall back to `send` if the adapter doesn't support updates).
/// - If this is the first progress message and the adapter supports updates,
///   call `begin_stream` to create a placeholder, then register the returned
///   message_id.
/// - In `UpdateProgress` mode, only progress messages update the placeholder;
///   final messages are sent as new messages (removing the stream from the
///   registry).
///
/// Non-streaming messages (tool hints, plain responses) are always sent
/// directly via `channel.send`.
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
                    // Final message in UpdateProgress mode: send as new.
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
        cfg.channels.instances.insert(
            "test_bot".into(),
            ChannelInstanceConfig::Telegram(nanobot_config::schema::TelegramChannelConfig {
                enabled: true,
                allow_from: Vec::new(),
                token: "x".to_string(),
                ..Default::default()
            }),
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
        cfg.channels.instances.insert(
            "test_bot".into(),
            ChannelInstanceConfig::Telegram(nanobot_config::schema::TelegramChannelConfig {
                enabled: true,
                allow_from: vec![" ".to_string()],
                token: "x".to_string(),
                ..Default::default()
            }),
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
        cfg.channels.instances.insert(
            "test_bot".into(),
            ChannelInstanceConfig::Telegram(nanobot_config::schema::TelegramChannelConfig {
                enabled: true,
                allow_from: vec!["*".to_string(), "123".to_string()],
                token: "x".to_string(),
                ..Default::default()
            }),
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
    fn manager_rejects_feishu_without_delivery_config() {
        let mut cfg = Config::default();
        cfg.channels.instances.insert(
            "test_feishu".into(),
            ChannelInstanceConfig::Feishu(nanobot_config::schema::FeishuChannelConfig {
                enabled: true,
                allow_from: vec!["*".to_string()],
                ..Default::default()
            }),
        );

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
        cfg.channels.instances.insert(
            "test_feishu".into(),
            ChannelInstanceConfig::Feishu(nanobot_config::schema::FeishuChannelConfig {
                enabled: true,
                allow_from: vec!["*".to_string()],
                ..Default::default()
            }),
        );

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
