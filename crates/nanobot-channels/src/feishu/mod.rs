//! Feishu / Lark channel adapter.
//!
//! This module implements the [`ChannelAdapter`] trait for the
//! [Feishu Open Platform](https://open.feishu.cn/).  It supports two
//! delivery modes (app-based and webhook) and two inbound modes
//! (HTTP callback and WebSocket).
//!
//! # Modes
//!
//! | Mode | Direction | Config |
//! |------|-----------|--------|
//! | **App (IM API)** | inbound + outbound | `appId` + `appSecret` |
//! | **Webhook** | outbound only | `webhookUrl` / `botKey` (optionally with `secret`) |
//!
//! # Inbound Events
//!
//! Inbound messages can arrive via:
//! - **HTTP callback** — starts a local Axum server listening on
//!   `callbackListen:callbackPath`.
//! - **WebSocket** — uses `open_lark` WS client for persistent connection
//!   (auto-reconnect with exponential backoff).
//!
//! # Streaming
//!
//! When `appId`/`appSecret` are configured, the adapter supports streaming
//! message updates: a placeholder is created via `begin_stream`, then
//! subsequent chunks update the same message via `update`.  Batching and
//! sharding protect against Feishu's per-message edit limits.
//!
//! # Rendering
//!
//! Outbound text can be rendered as plain text or as an interactive card
//! (controlled by the `renderMode` config).  The `Auto` mode heuristically
//! sniffs the content to choose the best format.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use chrono::Utc;
use open_lark::Config as OpenLarkConfig;
use open_lark::ws_client::{EventDispatcherHandler, LarkWsClient};
use reqwest::Client;
use reqwest::multipart::{Form, Part};
use serde_json::json;
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

use crate::base::{ChannelAdapter, SendOutcome};
use crate::error::{ChannelError, ChannelResult};
use nanobot_bus::{MessageBus, OutboundMessage};
use nanobot_config::schema::FeishuChannelConfig;

/// Default Feishu Open API base URL.
const FEISHU_API_DEFAULT: &str = "https://open.feishu.cn";
/// Maximum byte length for a single Feishu text message.
const FEISHU_TEXT_LIMIT: usize = 15000;
/// Default address for the HTTP callback listener.
const FEISHU_CALLBACK_LISTEN_DEFAULT: &str = "0.0.0.0:19820";
/// Default path for the HTTP callback endpoint.
const FEISHU_CALLBACK_PATH_DEFAULT: &str = "/feishu/events";
/// Minimum new content (in chars) before flushing a batched edit.
const FEISHU_EDIT_BATCH_NEW_CHARS: usize = 500;
/// Minimum interval between batched edits.
const FEISHU_EDIT_BATCH_INTERVAL: Duration = Duration::from_secs(2);
/// Max edits per message before sharding to a new message. Feishu limit is 20.
const FEISHU_EDIT_SHARD_EDITS: usize = 18;
/// Max content length (in chars) before sharding to a new message.
const FEISHU_EDIT_SHARD_CHARS: usize = 24000;
const LOG_TARGET: &str = "nanobot::channels::feishu";

/// Per-stream state for batching edit API calls and sharding long streams.
pub mod card;
pub mod types;
pub mod util;
use self::card::*;
use self::types::*;
use self::util::*;

/// Feishu / Lark channel adapter.
///
/// Implements [`ChannelAdapter`] for the Feishu Open Platform.
/// Supports both app-based (IM API) and webhook delivery modes.
///
/// # Lifecycle
///
/// 1. **Construction** ([`new`](FeishuChannel::new)) — validates config,
///    builds the reqwest client, and resolves delivery/event modes.
/// 2. **Start** ([`ChannelAdapter::start`]) — runs connectivity checks,
///    then starts either a WebSocket client or an HTTP callback server
///    for inbound events.
/// 3. **Runtime** — outbound messages are delivered via `send` / `update`;
///    inbound events are parsed and published to the `MessageBus`.
/// 4. **Stop** ([`ChannelAdapter::stop`]) — sets the running flag to false
///    and aborts background tasks.
///
/// # Thread Safety
///
/// Mutable state (callbacks, WebSocket, token cache, edit tracking) is
/// protected by `tokio::sync::Mutex` because operations cross `.await`
/// points.  The `running` flag is an `Arc<AtomicBool>` shared with
/// background tasks.
pub struct FeishuChannel {
    /// Channel instance name (from config).
    name: String,
    /// Reusable HTTP client shared across all API calls.
    client: Client,
    /// Shared message bus for publishing inbound messages.
    bus: MessageBus,
    /// Access-control list for inbound message filtering.
    allow_from: Vec<String>,
    /// Feishu Open API base URL (defaults to `FEISHU_API_DEFAULT`).
    api_base: String,
    /// Webhook URL for outbound-only mode (or `None` when using app mode).
    webhook_url: Option<String>,
    /// HMAC secret for webhook signature verification.
    secret: Option<String>,
    /// Feishu app ID (required for app mode).
    app_id: Option<String>,
    /// Feishu app secret (required for app mode).
    app_secret: Option<String>,
    /// Optional verification token for callback authentication.
    verify_token: Option<String>,
    /// Address for the HTTP callback server (e.g. `"0.0.0.0:19820"`).
    callback_listen: Option<String>,
    /// Path for the HTTP callback endpoint.
    callback_path: String,
    /// Whether to use WebSocket (instead of HTTP callback) for inbound events.
    ws_enabled: bool,
    /// Whether to send a placeholder message at the start of a stream.
    stream_placeholder_enabled: bool,
    /// Text for the stream placeholder message.
    stream_placeholder_text: String,
    /// Message rendering mode (Raw, Card, or Auto).
    render_mode: RenderMode,
    /// Whether the channel is currently running (shared with background tasks).
    running: Arc<AtomicBool>,
    /// Handle for the HTTP callback server task.
    callback_task: Mutex<Option<JoinHandle<()>>>,
    /// Handle for the WebSocket client task.
    ws_task: Mutex<Option<JoinHandle<()>>>,
    /// Cached tenant access token with expiration tracking.
    tenant_access_token: Mutex<Option<CachedTenantAccessToken>>,
    /// Per-stream edit state for batching + sharding.
    edit_states: Mutex<HashMap<String, StreamEditState>>,
}

impl FeishuChannel {
    /// Construct a new `FeishuChannel` from configuration.
    ///
    /// Validates the configuration:
    /// - `appId` and `appSecret` must be provided together.
    /// - At least one delivery method must be configured (webhook URL or app
    ///   credentials).
    /// - Resolves callback vs. WebSocket event listening mode.
    ///
    /// Does **not** start any listeners or verify credentials — call
    /// [`ChannelAdapter::start`] for that.
    ///
    /// # Errors
    /// Returns [`ChannelError::Config`] if:
    /// - Only one of `appId` / `appSecret` is present.
    /// - Neither a webhook URL nor app credentials are configured.
    /// - The reqwest client cannot be built.
    pub fn new(name: String, cfg: FeishuChannelConfig, bus: MessageBus) -> ChannelResult<Self> {
        let api_base = cfg
            .api_base
            .clone()
            .unwrap_or_else(|| FEISHU_API_DEFAULT.to_string());
        let webhook_url = build_webhook_url(&cfg, &api_base);
        let secret = None;

        let app_id = if cfg.app_id.is_empty() {
            None
        } else {
            Some(cfg.app_id.clone())
        };
        let app_secret = if cfg.app_secret.is_empty() {
            None
        } else {
            Some(cfg.app_secret.clone())
        };
        if (app_id.is_some() && app_secret.is_none()) || (app_id.is_none() && app_secret.is_some())
        {
            return Err(ChannelError::config(
                "feishu.appId and feishu.appSecret must be configured together",
            ));
        }
        if webhook_url.is_none() && app_id.is_none() {
            return Err(ChannelError::config(
                "feishu requires either webhook/webhookUrl/url/botKey or appId+appSecret",
            ));
        }

        let verify_token = match &cfg.verify_token {
            Some(t) if !t.is_empty() => Some(t.clone()),
            _ => None,
        };
        let explicit_callback = cfg.event_enabled;
        let ws_enabled = cfg
            .ws_enabled
            .unwrap_or_else(|| app_id.is_some() && explicit_callback != Some(true));

        let callback_listen = if explicit_callback == Some(true)
            || (explicit_callback.is_none() && app_id.is_some() && !ws_enabled)
        {
            Some(
                cfg.callback_listen
                    .clone()
                    .unwrap_or_else(|| FEISHU_CALLBACK_LISTEN_DEFAULT.to_string()),
            )
        } else {
            None
        };

        let callback_path = cfg
            .callback_path
            .clone()
            .unwrap_or_else(|| FEISHU_CALLBACK_PATH_DEFAULT.to_string());
        let stream_placeholder_enabled = cfg.stream_placeholder_enabled.unwrap_or(false);
        let stream_placeholder_text = cfg
            .stream_placeholder_text
            .clone()
            .unwrap_or_else(|| "thinking...".to_string());
        let render_mode = cfg
            .render_mode
            .as_deref()
            .map(RenderMode::from)
            .unwrap_or(RenderMode::Raw);
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|err| {
                ChannelError::adapter("feishu", format!("build reqwest client failed: {err}"))
            })?;

        Ok(Self {
            name,
            client,
            bus,
            allow_from: cfg.allow_from,
            api_base,
            webhook_url,
            secret,
            app_id,
            app_secret,
            verify_token,
            callback_listen,
            callback_path,
            ws_enabled,
            stream_placeholder_enabled,
            stream_placeholder_text,
            render_mode,
            running: Arc::new(AtomicBool::new(false)),
            callback_task: Mutex::new(None),
            ws_task: Mutex::new(None),
            tenant_access_token: Mutex::new(None),
            edit_states: Mutex::new(HashMap::new()),
        })
    }

    /// Build the `open_lark` WebSocket client configuration from app credentials.
    ///
    /// # Errors
    /// Returns [`ChannelError::Config`] if `appId` or `appSecret` are missing,
    /// or if the `open_lark` builder fails.
    fn build_openlark_ws_config(&self) -> ChannelResult<OpenLarkConfig> {
        let app_id = self
            .app_id
            .clone()
            .ok_or_else(|| ChannelError::config("feishu.appId is required for WebSocket mode"))?;
        let app_secret = self.app_secret.clone().ok_or_else(|| {
            ChannelError::config("feishu.appSecret is required for WebSocket mode")
        })?;
        OpenLarkConfig::builder()
            .app_id(app_id)
            .app_secret(app_secret)
            .base_url(self.api_base.clone())
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|err| {
                ChannelError::adapter("feishu", format!("build openlark ws config failed: {err}"))
            })
    }

    /// Verify that the Feishu API is reachable and credentials are valid.
    ///
    /// Fetches a tenant access token.  Called during [`start`](ChannelAdapter::start)
    /// before enabling inbound listeners.
    ///
    /// # Errors
    /// Returns [`ChannelError::Adapter`] if the token fetch fails.
    async fn verify_auth_connectivity(&self) -> ChannelResult<()> {
        self.fetch_tenant_access_token().await.map_err(|err| {
            ChannelError::adapter(
                "feishu",
                format!("startup auth connectivity check failed: {err}"),
            )
        })?;

        info!(target: LOG_TARGET, "feishu startup auth connectivity check passed");
        Ok(())
    }

    /// Verify that the Feishu IM API is usable by listing chats.
    ///
    /// This is a best-effort check — failures are logged as warnings but do
    /// not prevent startup (the issue may be transient or permissions-related).
    async fn verify_im_readiness(&self) -> ChannelResult<()> {
        let access_token = self.tenant_access_token().await.map_err(|err| {
            ChannelError::adapter("feishu", format!("startup IM readiness auth failed: {err}"))
        })?;
        let url = format!(
            "{}/open-apis/im/v1/chats",
            self.api_base.trim_end_matches('/')
        );
        let response = self
            .client
            .get(url)
            .query(&[("page_size", "1")])
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|err| {
                ChannelError::adapter(
                    "feishu",
                    format!("startup IM readiness check request failed: {err}"),
                )
            })?;
        let status = response.status();
        let body_text = response.text().await.map_err(|err| {
            ChannelError::adapter(
                "feishu",
                format!("startup IM readiness response read failed: {err}"),
            )
        })?;
        if !status.is_success() {
            return Err(ChannelError::adapter(
                "feishu",
                format!("startup IM readiness http {}: {}", status, body_text),
            ));
        }

        let body: FeishuApiResponse<serde_json::Value> =
            serde_json::from_str(&body_text).map_err(|err| {
                ChannelError::adapter(
                    "feishu",
                    format!("startup IM readiness parse failed: {err}; body={body_text}"),
                )
            })?;
        if body.code != 0 {
            return Err(ChannelError::adapter(
                "feishu",
                format!(
                    "startup IM readiness rejected: code={} msg={}",
                    body.code,
                    body.msg.unwrap_or(body_text)
                ),
            ));
        }

        info!(target: LOG_TARGET, "feishu startup IM readiness check passed");
        Ok(())
    }

    /// Start the Feishu WebSocket event subscription client.
    ///
    /// Spawns a background task that:
    /// 1. Opens a persistent WebSocket connection via `open_lark`.
    /// 2. Feeds received payloads through a channel to a parser task.
    /// 3. Parses events, verifies tokens, and publishes inbound messages
    ///    to the `MessageBus`.
    ///
    /// On disconnect, reconnects with exponential backoff (1s .. 60s).
    async fn start_websocket(&self) -> ChannelResult<()> {
        let ws_config = Arc::new(self.build_openlark_ws_config()?);
        let name = self.name.clone();
        let bus = self.bus.clone();
        let allow_from = self.allow_from.clone();
        let verify_token = self.verify_token.clone();
        let running = self.running.clone();

        let ws_task = tokio::spawn(async move {
            const MAX_BACKOFF: Duration = Duration::from_secs(60);
            let mut retry_delay = Duration::from_secs(1);

            loop {
                if !running.load(Ordering::SeqCst) {
                    break;
                }

                let (payload_tx, mut payload_rx) = mpsc::unbounded_channel::<Vec<u8>>();
                let dispatcher = EventDispatcherHandler::builder()
                    .payload_sender(payload_tx)
                    .build();

                let payload_task = tokio::spawn({
                    let running = running.clone();
                    let bus = bus.clone();
                    let name = name.clone();
                    let allow_from = allow_from.clone();
                    let verify_token = verify_token.clone();
                    async move {
                        while let Some(payload) = payload_rx.recv().await {
                            if !running.load(Ordering::SeqCst) {
                                break;
                            }
                            match serde_json::from_slice::<FeishuIncomingEnvelope>(&payload) {
                                Ok(envelope) => {
                                    if let Some(expected) = verify_token.as_deref()
                                        && !expected.is_empty()
                                    {
                                        let actual = envelope
                                            .header
                                            .as_ref()
                                            .and_then(|h| h.token.as_deref())
                                            .unwrap_or_default();
                                        if actual != expected {
                                            warn!(
                                                target: LOG_TARGET,
                                                "feishu WS event token mismatch: got '{}', expected '{}'",
                                                actual,
                                                expected
                                            );
                                            continue;
                                        }
                                    }
                                    match extract_inbound_message(&name, &envelope, &allow_from) {
                                        Ok(Some(message)) => {
                                            if let Err(err) = bus.publish_inbound(message) {
                                                warn!(target: LOG_TARGET, "feishu WS publish inbound failed: {}", err);
                                            }
                                        }
                                        Ok(None) => {}
                                        Err(err) => {
                                            warn!(target: LOG_TARGET, "feishu WS event parse skipped: {}", err);
                                        }
                                    }
                                }
                                Err(err) => {
                                    warn!(target: LOG_TARGET, "feishu WS payload decode failed: {}", err);
                                }
                            }
                        }
                    }
                });

                info!(target: LOG_TARGET, "feishu WebSocket connecting");
                match LarkWsClient::open(ws_config.clone(), dispatcher).await {
                    Ok(()) => {
                        info!(target: LOG_TARGET, "feishu WebSocket closed");
                        retry_delay = Duration::from_secs(1);
                    }
                    Err(err) => {
                        error!(target: LOG_TARGET, "feishu WebSocket error: {}", err);
                    }
                }

                payload_task.abort();

                if !running.load(Ordering::SeqCst) {
                    break;
                }

                info!(
                    target: LOG_TARGET,
                    retry_delay_ms = retry_delay.as_millis(),
                    "feishu WebSocket reconnecting in {}ms",
                    retry_delay.as_millis()
                );
                tokio::time::sleep(retry_delay).await;
                retry_delay = (retry_delay * 2).min(MAX_BACKOFF);
            }
        });

        *self.ws_task.lock().await = Some(ws_task);
        info!(target: LOG_TARGET, "feishu WebSocket event subscription started");
        Ok(())
    }

    /// Build the Axum router for the HTTP callback endpoint.
    fn callback_router(state: FeishuCallbackState, path: &str) -> Router {
        Router::new()
            .route(path, post(feishu_event_handler))
            .with_state(state)
    }

    /// Send a text message to a chat using the IM API.
    ///
    /// Retries once with a fresh token if the first attempt fails with an
    /// auth error.
    ///
    /// # Arguments
    /// * `receive_id` — The chat ID to send to.
    /// * `text` — Message text content.
    ///
    /// # Returns
    /// The platform-assigned `message_id` on success.
    async fn send_message_by_app(&self, receive_id: &str, text: &str) -> ChannelResult<String> {
        let content = serialize_text_content(text)?;

        let mut last_err: Option<ChannelError> = None;
        for attempt in 0..2 {
            let token = if attempt == 0 {
                self.tenant_access_token().await?
            } else {
                self.refresh_tenant_access_token().await?
            };
            match self
                .send_message_by_app_with_token(receive_id, &content, &token)
                .await
            {
                Ok(message_id) => return Ok(message_id),
                Err(err) if attempt == 0 && is_retryable_auth_send_error(&err) => {
                    warn!(
                        target: LOG_TARGET,
                        "feishu app send failed with cached token, refreshing tenant token and retrying: {}",
                        err
                    );
                    last_err = Some(err);
                }
                Err(err) => return Err(err),
            }
        }

        Err(last_err.unwrap_or_else(|| {
            ChannelError::adapter("feishu", "send app message failed after retry")
        }))
    }

    /// Send text using the IM API with an explicit access token (no retry).
    async fn send_message_by_app_with_token(
        &self,
        receive_id: &str,
        content: &str,
        access_token: &str,
    ) -> ChannelResult<String> {
        self.send_im_message_by_app_with_token(receive_id, "text", content, access_token)
            .await
    }

    /// Send an IM message of any `msg_type` using the IM API with a token.
    ///
    /// This is the low-level API call used by text, image, and interactive
    /// message senders.
    async fn send_im_message_by_app_with_token(
        &self,
        receive_id: &str,
        msg_type: &str,
        content: &str,
        access_token: &str,
    ) -> ChannelResult<String> {
        let url = format!(
            "{}/open-apis/im/v1/messages",
            self.api_base.trim_end_matches('/')
        );
        let response = self
            .client
            .post(url)
            .query(&[("receive_id_type", "chat_id")])
            .bearer_auth(access_token)
            .json(&json!({
                "receive_id": receive_id,
                "msg_type": msg_type,
                "content": content,
            }))
            .send()
            .await
            .map_err(|err| {
                ChannelError::adapter(
                    "feishu",
                    format!("send app {msg_type} message request failed: {err}"),
                )
            })?;

        let status = response.status();
        let body_text = response.text().await.map_err(|err| {
            ChannelError::adapter("feishu", format!("read app message response failed: {err}"))
        })?;
        if !status.is_success() {
            return Err(ChannelError::adapter(
                "feishu",
                format!("send app {msg_type} message http {}: {}", status, body_text),
            ));
        }

        let body: FeishuApiResponse<FeishuSendMessageData> = serde_json::from_str(&body_text)
            .map_err(|err| {
                ChannelError::adapter(
                    "feishu",
                    format!(
                        "parse app {msg_type} message response failed: {err}; body={body_text}"
                    ),
                )
            })?;
        if body.code != 0 {
            return Err(ChannelError::adapter(
                "feishu",
                format!(
                    "send app {msg_type} message rejected: code={} msg={}",
                    body.code,
                    body.msg.unwrap_or_else(|| body_text.clone())
                ),
            ));
        }

        Ok(body
            .data
            .and_then(|data| data.message_id)
            .unwrap_or_default())
    }

    /// Send an image message to a chat using the IM API.
    ///
    /// Retries once with a fresh token on auth errors.
    async fn send_image_by_app(&self, receive_id: &str, media_ref: &str) -> ChannelResult<String> {
        let mut last_err: Option<ChannelError> = None;
        for attempt in 0..2 {
            let token = if attempt == 0 {
                self.tenant_access_token().await?
            } else {
                self.refresh_tenant_access_token().await?
            };
            match self
                .send_image_by_app_with_token(receive_id, media_ref, &token)
                .await
            {
                Ok(message_id) => return Ok(message_id),
                Err(err) if attempt == 0 && is_retryable_auth_send_error(&err) => {
                    warn!(
                        target: LOG_TARGET,
                        "feishu app image send failed with cached token, refreshing tenant token and retrying: {}",
                        err
                    );
                    last_err = Some(err);
                }
                Err(err) => return Err(err),
            }
        }

        Err(last_err.unwrap_or_else(|| {
            ChannelError::adapter("feishu", "send app image message failed after retry")
        }))
    }

    /// Send an image using the IM API with an explicit access token.
    ///
    /// If `media_ref` is a Feishu image key (`feishu:image_key:{key}`), uses
    /// it directly. Otherwise, downloads/uploads the image first.
    async fn send_image_by_app_with_token(
        &self,
        receive_id: &str,
        media_ref: &str,
        access_token: &str,
    ) -> ChannelResult<String> {
        let image_key = if let Some(image_key) = extract_feishu_image_key_ref(media_ref) {
            image_key.to_string()
        } else {
            self.upload_image_and_get_key(media_ref, access_token)
                .await?
        };
        let content = serde_json::to_string(&json!({ "image_key": image_key })).map_err(|err| {
            ChannelError::adapter(
                "feishu",
                format!("serialize image message content failed: {err}; media={media_ref}"),
            )
        })?;
        self.send_im_message_by_app_with_token(receive_id, "image", &content, access_token)
            .await
    }

    /// Upload an image to Feishu and return its `image_key`.
    ///
    /// Resolves the image (download from URL or read from local file), then
    /// uploads it via the Feishu IM API.
    async fn upload_image_and_get_key(
        &self,
        media_ref: &str,
        access_token: &str,
    ) -> ChannelResult<String> {
        let (image_bytes, file_name, mime_type) = self.resolve_image(media_ref).await?;
        let file_part = Part::bytes(image_bytes)
            .file_name(file_name)
            .mime_str(&mime_type)
            .map_err(|err| {
                ChannelError::adapter(
                    "feishu",
                    format!("invalid image mime type '{mime_type}': {err}"),
                )
            })?;
        let form = Form::new()
            .text("image_type", "message")
            .part("image", file_part);

        let upload_url = format!(
            "{}/open-apis/im/v1/images",
            self.api_base.trim_end_matches('/')
        );
        let upload_response = self
            .client
            .post(upload_url)
            .bearer_auth(access_token)
            .multipart(form)
            .send()
            .await
            .map_err(|err| {
                ChannelError::adapter(
                    "feishu",
                    format!("upload image request failed: {err}; media={media_ref}"),
                )
            })?;
        let upload_status = upload_response.status();
        let upload_body_text = upload_response.text().await.map_err(|err| {
            ChannelError::adapter(
                "feishu",
                format!("read image upload response failed: {err}"),
            )
        })?;
        if !upload_status.is_success() {
            return Err(ChannelError::adapter(
                "feishu",
                format!(
                    "upload image http {}: {}; media={}",
                    upload_status, upload_body_text, media_ref
                ),
            ));
        }
        let upload_body: FeishuApiResponse<FeishuUploadImageData> =
            serde_json::from_str(&upload_body_text).map_err(|err| {
                ChannelError::adapter(
                    "feishu",
                    format!("parse image upload response failed: {err}; body={upload_body_text}"),
                )
            })?;
        if upload_body.code != 0 {
            return Err(ChannelError::adapter(
                "feishu",
                format!(
                    "upload image rejected: code={} msg={}; media={}",
                    upload_body.code,
                    upload_body.msg.unwrap_or_else(|| upload_body_text.clone()),
                    media_ref
                ),
            ));
        }
        upload_body
            .data
            .and_then(|data| data.image_key)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                ChannelError::adapter(
                    "feishu",
                    format!("image upload response missing image_key; media={media_ref}"),
                )
            })
    }

    /// Resolve an image reference into bytes, filename, and MIME type.
    ///
    /// Supports URLs (`http://`, `https://`) and local file paths.
    async fn resolve_image(&self, media_ref: &str) -> ChannelResult<(Vec<u8>, String, String)> {
        if media_ref.starts_with("http://") || media_ref.starts_with("https://") {
            return self.resolve_image_from_url(media_ref).await;
        }
        self.resolve_image_from_file(media_ref).await
    }

    /// Download an image from a URL and infer its MIME type from headers and extension.
    async fn resolve_image_from_url(
        &self,
        media_ref: &str,
    ) -> ChannelResult<(Vec<u8>, String, String)> {
        let response = self.client.get(media_ref).send().await.map_err(|err| {
            ChannelError::adapter(
                "feishu",
                format!("download image failed: {err}; media={media_ref}"),
            )
        })?;
        let status = response.status();
        if !status.is_success() {
            return Err(ChannelError::adapter(
                "feishu",
                format!("download image http {}: media={}", status, media_ref),
            ));
        }
        let header_mime = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(|value| {
                value
                    .split(';')
                    .next()
                    .unwrap_or(value)
                    .trim()
                    .to_ascii_lowercase()
            });
        let bytes = response.bytes().await.map_err(|err| {
            ChannelError::adapter(
                "feishu",
                format!("read downloaded image bytes failed: {err}; media={media_ref}"),
            )
        })?;
        if bytes.is_empty() {
            return Err(ChannelError::adapter(
                "feishu",
                format!("downloaded image is empty; media={media_ref}"),
            ));
        }

        let file_name = infer_file_name(media_ref);
        let inferred_mime = infer_image_mime_from_name(&file_name);
        let mime = match header_mime {
            Some(value) if value.starts_with("image/") => value,
            Some(value) if inferred_mime.is_some() => {
                inferred_mime.unwrap_or("image/jpeg").to_string()
            }
            Some(value) => {
                return Err(ChannelError::adapter(
                    "feishu",
                    format!(
                        "downloaded content-type is not image ('{}'); media={}",
                        value, media_ref
                    ),
                ));
            }
            None => inferred_mime.unwrap_or("image/jpeg").to_string(),
        };
        Ok((bytes.to_vec(), file_name, mime))
    }

    /// Read an image from a local file and infer its MIME type from the extension.
    async fn resolve_image_from_file(
        &self,
        media_ref: &str,
    ) -> ChannelResult<(Vec<u8>, String, String)> {
        let path = Path::new(media_ref);
        let bytes = tokio::fs::read(path).await.map_err(|err| {
            ChannelError::adapter(
                "feishu",
                format!("read local image file failed: {err}; media={media_ref}"),
            )
        })?;
        if bytes.is_empty() {
            return Err(ChannelError::adapter(
                "feishu",
                format!("local image file is empty; media={media_ref}"),
            ));
        }
        let file_name = path
            .file_name()
            .and_then(|value| value.to_str())
            .map(ToString::to_string)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "image.jpg".to_string());
        let mime = infer_image_mime_from_name(&file_name).ok_or_else(|| {
            ChannelError::adapter(
                "feishu",
                format!(
                    "unsupported local image extension; media={media_ref} (supported: png/jpg/jpeg/gif/webp/bmp/tif/tiff/heic/heif)"
                ),
            )
        })?;

        Ok((bytes, file_name, mime.to_string()))
    }

    /// Get a cached tenant access token, refreshing if expired or absent.
    ///
    /// Uses a 60-second safety margin before the actual expiry.
    /// Tokens are cached inside a `tokio::sync::Mutex`.
    async fn tenant_access_token(&self) -> ChannelResult<String> {
        {
            let cached = self.tenant_access_token.lock().await;
            if let Some(token) = cached.as_ref()
                && token.expires_at > Utc::now() + chrono::Duration::seconds(60)
            {
                return Ok(token.access_token.clone());
            }
        }

        self.refresh_tenant_access_token().await
    }

    /// Force-refresh the tenant access token from the Feishu API and cache it.
    async fn refresh_tenant_access_token(&self) -> ChannelResult<String> {
        let token = self.fetch_tenant_access_token().await?;
        let access_token = token.access_token.clone();
        *self.tenant_access_token.lock().await = Some(token);
        Ok(access_token)
    }

    /// Actually call the Feishu tenant token endpoint (up to 3 retries).
    ///
    /// Uses linear backoff: 250ms * attempt number between retries.
    async fn fetch_tenant_access_token(&self) -> ChannelResult<CachedTenantAccessToken> {
        let app_id = self
            .app_id
            .as_deref()
            .ok_or_else(|| ChannelError::adapter("feishu", "appId is not configured"))?;
        let app_secret = self
            .app_secret
            .as_deref()
            .ok_or_else(|| ChannelError::adapter("feishu", "appSecret is not configured"))?;
        let url = format!(
            "{}/open-apis/auth/v3/tenant_access_token/internal",
            self.api_base.trim_end_matches('/')
        );

        let mut last_error = None;
        for attempt in 1..=3 {
            let request = self
                .client
                .post(&url)
                .json(&json!({ "app_id": app_id, "app_secret": app_secret }));
            match request.send().await {
                Ok(response) => {
                    let status = response.status();
                    let body_text = response.text().await.map_err(|err| {
                        ChannelError::adapter(
                            "feishu",
                            format!("read tenant token response failed: {err}"),
                        )
                    })?;
                    if !status.is_success() {
                        last_error = Some(ChannelError::adapter(
                            "feishu",
                            format!("tenant token http {}: {}", status, body_text),
                        ));
                    } else {
                        let body: FeishuTenantTokenResponse =
                            serde_json::from_str(&body_text).map_err(|err| {
                                ChannelError::adapter(
                                    "feishu",
                                    format!(
                                        "parse tenant token response failed: {err}; body={body_text}"
                                    ),
                                )
                            })?;
                        if body.code == 0 {
                            let access_token = body.tenant_access_token.ok_or_else(|| {
                                ChannelError::adapter(
                                    "feishu",
                                    "tenant token response missing tenant_access_token",
                                )
                            })?;
                            let expire = body.expire.unwrap_or(7200).max(120);
                            return Ok(CachedTenantAccessToken {
                                access_token,
                                expires_at: Utc::now() + chrono::Duration::seconds(expire),
                            });
                        }
                        last_error = Some(ChannelError::adapter(
                            "feishu",
                            format!(
                                "tenant token rejected: code={} msg={}",
                                body.code,
                                body.msg.unwrap_or_else(|| body_text.clone())
                            ),
                        ));
                    }
                }
                Err(err) => {
                    last_error = Some(ChannelError::adapter(
                        "feishu",
                        format!("request tenant token failed: {err}"),
                    ));
                }
            }

            if attempt < 3 {
                tokio::time::sleep(Duration::from_millis(250 * attempt as u64)).await;
            }
        }

        Err(last_error
            .unwrap_or_else(|| ChannelError::adapter("feishu", "request tenant token failed")))
    }

    /// Update a previously sent message by appending/setting new text content.
    ///
    /// If the new text exceeds the Feishu limit, sends the first chunk as an
    /// update and remaining chunks as new messages.
    ///
    /// Retries once with a fresh token on auth errors.
    async fn update_message_by_app(
        &self,
        message_id: &str,
        receive_id: &str,
        text: &str,
    ) -> ChannelResult<()> {
        let chunks = split_text(text, FEISHU_TEXT_LIMIT);
        let first_chunk = chunks.first().cloned().unwrap_or_default();
        let content = serialize_text_content(&first_chunk)?;

        let mut last_err: Option<ChannelError> = None;
        for attempt in 0..2 {
            let token = if attempt == 0 {
                self.tenant_access_token().await?
            } else {
                self.refresh_tenant_access_token().await?
            };
            match self
                .update_message_by_app_with_token(message_id, &content, &token)
                .await
            {
                Ok(()) => {
                    for chunk in chunks.into_iter().skip(1) {
                        self.send_message_by_app(receive_id, &chunk).await?;
                    }
                    return Ok(());
                }
                Err(err) if attempt == 0 && is_retryable_auth_send_error(&err) => {
                    warn!(
                        target: LOG_TARGET,
                        "feishu app update failed with cached token, refreshing tenant token and retrying: {}",
                        err
                    );
                    last_err = Some(err);
                }
                Err(err) => return Err(err),
            }
        }

        Err(last_err.unwrap_or_else(|| {
            ChannelError::adapter("feishu", "update app message failed after retry")
        }))
    }

    /// Update a message via the IM API with an explicit access token (no retry).
    async fn update_message_by_app_with_token(
        &self,
        message_id: &str,
        content: &str,
        access_token: &str,
    ) -> ChannelResult<()> {
        let url = format!(
            "{}/open-apis/im/v1/messages/{}",
            self.api_base.trim_end_matches('/'),
            message_id
        );
        let response = self
            .client
            .put(url)
            .bearer_auth(access_token)
            .json(&json!({
                "msg_type": "text",
                "content": content,
            }))
            .send()
            .await
            .map_err(|err| {
                ChannelError::adapter(
                    "feishu",
                    format!("update app message request failed: {err}"),
                )
            })?;

        let status = response.status();
        let body_text = response.text().await.map_err(|err| {
            ChannelError::adapter(
                "feishu",
                format!("read update message response failed: {err}"),
            )
        })?;
        if !status.is_success() {
            return Err(ChannelError::adapter(
                "feishu",
                format!("update app message http {}: {}", status, body_text),
            ));
        }

        let body: FeishuApiResponse<serde_json::Value> =
            serde_json::from_str(&body_text).map_err(|err| {
                ChannelError::adapter(
                    "feishu",
                    format!("parse update message response failed: {err}; body={body_text}"),
                )
            })?;
        if body.code != 0 {
            return Err(ChannelError::adapter(
                "feishu",
                format!(
                    "update app message rejected: code={} msg={}",
                    body.code,
                    body.msg.unwrap_or(body_text)
                ),
            ));
        }

        Ok(())
    }

    /// Send a text message via the Feishu webhook URL.
    ///
    /// Adds HMAC signature if a `secret` is configured.
    async fn send_message_by_webhook(&self, text: &str) -> ChannelResult<()> {
        let webhook_url = self
            .webhook_url
            .as_deref()
            .ok_or_else(|| ChannelError::adapter("feishu", "webhook url is not configured"))?;
        let mut payload = FeishuWebhookMessage {
            msg_type: "text".to_string(),
            content: FeishuTextContent {
                text: text.to_string(),
            },
            timestamp: None,
            sign: None,
        };
        if let Some(secret) = self.secret.as_deref() {
            let timestamp = chrono::Utc::now().timestamp().to_string();
            let sign = build_signature(&timestamp, secret)?;
            payload.timestamp = Some(timestamp);
            payload.sign = Some(sign);
        }
        let response = self
            .client
            .post(webhook_url)
            .json(&payload)
            .send()
            .await
            .map_err(|err| {
                ChannelError::adapter("feishu", format!("send webhook request failed: {}", err))
            })?;
        let status = response.status();
        let body: serde_json::Value = response.json().await.map_err(|err| {
            ChannelError::adapter("feishu", format!("parse webhook response failed: {}", err))
        })?;
        if !status.is_success() {
            return Err(ChannelError::adapter(
                "feishu",
                format!("webhook response status {}: {}", status, body),
            ));
        }
        if !is_success_response(&body) {
            return Err(ChannelError::adapter(
                "feishu",
                format!("webhook rejected message: {}", error_message(&body)),
            ));
        }
        Ok(())
    }

    /// Send an interactive card message via the IM API.
    ///
    /// Retries once with a fresh token on auth errors.
    async fn send_interactive_by_app(
        &self,
        receive_id: &str,
        content: &str,
    ) -> ChannelResult<String> {
        let mut last_err: Option<ChannelError> = None;
        for attempt in 0..2 {
            let token = if attempt == 0 {
                self.tenant_access_token().await?
            } else {
                self.refresh_tenant_access_token().await?
            };
            match self
                .send_im_message_by_app_with_token(receive_id, "interactive", content, &token)
                .await
            {
                Ok(message_id) => return Ok(message_id),
                Err(err) if attempt == 0 && is_retryable_auth_send_error(&err) => {
                    warn!(target: LOG_TARGET, "feishu interactive send failed, refreshing token: {}", err);
                    last_err = Some(err);
                }
                Err(err) => return Err(err),
            }
        }
        Err(last_err.unwrap_or_else(|| {
            ChannelError::adapter("feishu", "send interactive message failed after retry")
        }))
    }

    /// Send an interactive card via the webhook URL.
    ///
    /// Adds HMAC signature if a `secret` is configured.
    async fn send_card_by_webhook(&self, card: &serde_json::Value) -> ChannelResult<()> {
        let webhook_url = self
            .webhook_url
            .as_deref()
            .ok_or_else(|| ChannelError::adapter("feishu", "webhook url is not configured"))?;
        let mut payload = serde_json::json!({
            "msg_type": "interactive",
            "card": card,
        });
        if let Some(secret) = self.secret.as_deref() {
            let timestamp = chrono::Utc::now().timestamp().to_string();
            let sign = build_signature(&timestamp, secret)?;
            payload["timestamp"] = serde_json::Value::String(timestamp);
            payload["sign"] = serde_json::Value::String(sign);
        }
        let response = self
            .client
            .post(webhook_url)
            .json(&payload)
            .send()
            .await
            .map_err(|err| {
                ChannelError::adapter("feishu", format!("send card webhook failed: {err}"))
            })?;
        let status = response.status();
        let body: serde_json::Value = response.json().await.map_err(|err| {
            ChannelError::adapter(
                "feishu",
                format!("parse card webhook response failed: {err}"),
            )
        })?;
        if !status.is_success() {
            return Err(ChannelError::adapter(
                "feishu",
                format!("card webhook status {}: {}", status, body),
            ));
        }
        if !is_success_response(&body) {
            return Err(ChannelError::adapter(
                "feishu",
                format!("card webhook rejected: {}", error_message(&body)),
            ));
        }
        Ok(())
    }
}

#[async_trait]
impl ChannelAdapter for FeishuChannel {
    fn name(&self) -> &str {
        &self.name
    }

    /// Start the Feishu channel.
    ///
    /// 1. Runs auth connectivity and IM readiness checks if `appId` is set.
    /// 2. Starts either a WebSocket client or an HTTP callback server
    ///    for inbound events.
    /// 3. In outbound-only mode (no listener), logs a notice.
    ///
    /// # Errors
    /// Returns [`ChannelError::Adapter`] if auth connectivity fails or
    /// the callback listener cannot bind.
    async fn start(&self) -> ChannelResult<()> {
        if self.running.swap(true, Ordering::SeqCst) {
            return Ok(());
        }

        if self.app_id.is_some() {
            if let Err(err) = self.verify_auth_connectivity().await {
                self.running.store(false, Ordering::SeqCst);
                return Err(err);
            }
            if let Err(err) = self.verify_im_readiness().await {
                warn!(target: LOG_TARGET, "{}", err);
            }
        }

        if self.ws_enabled {
            self.start_websocket().await?;
        } else if let Some(listen) = self.callback_listen.clone() {
            let addr: SocketAddr = listen.parse().map_err(|_| {
                ChannelError::config(format!("invalid feishu.callbackListen '{}'", listen))
            })?;
            let state = FeishuCallbackState {
                name: self.name.clone(),
                bus: self.bus.clone(),
                allow_from: self.allow_from.clone(),
                verify_token: self.verify_token.clone(),
            };
            let path = normalize_path(&self.callback_path);
            let app = Self::callback_router(state, &path);
            let listener = tokio::net::TcpListener::bind(addr).await.map_err(|err| {
                ChannelError::adapter("feishu", format!("bind callback listener failed: {}", err))
            })?;
            let running = self.running.clone();
            let handle = tokio::spawn(async move {
                let serve = axum::serve(listener, app);
                tokio::select! {
                    result = serve => {
                        if let Err(err) = result {
                            error!(target: LOG_TARGET, "feishu callback server exited: {}", err);
                        }
                    }
                    _ = async {
                        while running.load(Ordering::SeqCst) {
                            tokio::time::sleep(Duration::from_millis(200)).await;
                        }
                    } => {}
                }
            });
            *self.callback_task.lock().await = Some(handle);
            info!(target: LOG_TARGET, "feishu callback server started at {}{}", listen, path);
        } else {
            info!(
                target: LOG_TARGET,
                "feishu callback server disabled; outbound-only mode"
            );
        }

        info!(target: LOG_TARGET, "feishu channel started");
        Ok(())
    }

    /// Stop the Feishu channel.
    ///
    /// Sets the running flag and aborts any background tasks
    /// (callback server and/or WebSocket client).
    async fn stop(&self) -> ChannelResult<()> {
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.callback_task.lock().await.take() {
            handle.abort();
        }
        if let Some(handle) = self.ws_task.lock().await.take() {
            handle.abort();
        }
        Ok(())
    }

    /// Send an outbound message to Feishu.
    ///
    /// Supports three content paths:
    /// - **Text** — via webhook or app mode, split into chunks if needed.
    /// - **Card** — interactive card rendering (app or webhook).
    /// - **Images** — uploaded and sent via app mode.
    ///
    /// # Errors
    /// Returns [`ChannelError::Adapter`] if any API call fails.
    /// Returns an error if media is included but the adapter is in
    /// webhook-only mode.
    async fn send(&self, msg: OutboundMessage) -> ChannelResult<SendOutcome> {
        let text = msg.content.trim();
        if text.is_empty() && msg.media.is_empty() {
            return Ok(SendOutcome::default());
        }
        if !msg.media.is_empty() && self.app_id.is_none() {
            return Err(ChannelError::adapter(
                "feishu",
                "sending image media requires appId/appSecret mode (webhook mode only supports text)",
            ));
        }

        let mode = self.render_mode.resolve(text);
        let mut last_message_id: Option<String> = None;
        if !text.is_empty() {
            match mode {
                RenderMode::Card if self.app_id.is_some() => {
                    let card_content = build_card_content(text)?;
                    let message_id = self
                        .send_interactive_by_app(&msg.chat_id, &card_content)
                        .await?;
                    if !message_id.is_empty() {
                        info!(target: LOG_TARGET, chat_id = %msg.chat_id, %message_id, "card sent");
                        last_message_id = Some(message_id);
                    }
                }
                RenderMode::Card => {
                    let card = build_webhook_card_content(text)?;
                    self.send_card_by_webhook(&card).await?;
                }
                RenderMode::Raw | RenderMode::Auto => {
                    let raw_text = convert_markdown_tables(text);
                    let chunks: Vec<_> = split_text(&raw_text, FEISHU_TEXT_LIMIT);
                    info!(
                        target: LOG_TARGET,
                        chat_id = %msg.chat_id,
                        chunks = chunks.len(),
                        content_len = raw_text.len(),
                        "sending message"
                    );
                    for chunk in chunks {
                        if self.app_id.is_some() {
                            let message_id = self.send_message_by_app(&msg.chat_id, &chunk).await?;
                            if !message_id.is_empty() {
                                info!(target: LOG_TARGET, chat_id = %msg.chat_id, %message_id, "message sent");
                                last_message_id = Some(message_id);
                            }
                        } else {
                            self.send_message_by_webhook(&chunk).await?;
                        }
                    }
                }
            }
        }
        for media_ref in &msg.media {
            let message_id = self.send_image_by_app(&msg.chat_id, media_ref).await?;
            if !message_id.is_empty() {
                last_message_id = Some(message_id);
            }
        }
        Ok(SendOutcome {
            message_id: last_message_id,
        })
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Update a previously sent message (streaming edit).
    ///
    /// Uses batching and sharding to respect Feishu's per-message edit limits:
    ///
    /// - **Batch**: if the new content since last flush is below
    ///   `FEISHU_EDIT_BATCH_NEW_CHARS` chars **and** less than
    ///   `FEISHU_EDIT_BATCH_INTERVAL` has passed, the update is skipped.
    /// - **Shard**: if the current message has been edited more than
    ///   `FEISHU_EDIT_SHARD_EDITS` times **or** the content exceeds
    ///   `FEISHU_EDIT_SHARD_CHARS` chars, a new message is sent and
    ///   the edit state switches to that new message.
    ///
    /// In webhook mode (no `app_id`), falls back to `send` (new message).
    async fn update(&self, message_id: &str, msg: OutboundMessage) -> ChannelResult<()> {
        let text = msg.content.trim().to_string();
        if text.is_empty() {
            return Ok(());
        }
        if self.app_id.is_none() {
            let _ = self.send(msg).await?;
            return Ok(());
        }

        let content_len = text.chars().count();
        let mut states = self.edit_states.lock().await;
        let state = states
            .entry(message_id.to_string())
            .or_insert_with(|| StreamEditState {
                actual_message_id: message_id.to_string(),
                edit_count: 0,
                last_flushed_len: 0,
                last_flush: Instant::now(),
            });

        let new_chars = content_len.saturating_sub(state.last_flushed_len);
        let elapsed = state.last_flush.elapsed();

        // Batch: skip if not enough new content AND not enough time passed.
        if new_chars < FEISHU_EDIT_BATCH_NEW_CHARS && elapsed < FEISHU_EDIT_BATCH_INTERVAL {
            return Ok(());
        }

        // Shard: if the current message has too many edits or is too long,
        // send a new message and switch to editing that one instead.
        if state.edit_count >= FEISHU_EDIT_SHARD_EDITS || content_len >= FEISHU_EDIT_SHARD_CHARS {
            info!(
                target: LOG_TARGET,
                chat_id = %msg.chat_id,
                edits = state.edit_count,
                content_len,
                "sharding stream to new message"
            );
            let new_message_id = self.send_message_by_app(&msg.chat_id, &text).await?;
            state.actual_message_id = new_message_id;
            state.edit_count = 0;
            state.last_flushed_len = content_len;
            state.last_flush = Instant::now();
            return Ok(());
        }

        // Normal edit on the current message.
        self.update_message_by_app(&state.actual_message_id, &msg.chat_id, &text)
            .await?;

        state.edit_count += 1;
        state.last_flushed_len = content_len;
        state.last_flush = Instant::now();

        info!(
            target: LOG_TARGET,
            chat_id = %msg.chat_id,
            edit = state.edit_count,
            content_len,
            "message updated"
        );

        Ok(())
    }

    /// Whether the adapter supports streaming message updates.
    /// Returns `true` when `appId` is configured (IM API mode).
    /// Webhook mode cannot edit messages.
    fn supports_stream_updates(&self) -> bool {
        self.app_id.is_some()
    }

    /// Send a placeholder message to begin a streaming response.
    ///
    /// Only applies when `stream_placeholder_enabled` is true AND `appId`
    /// is configured.  The placeholder text is configurable via
    /// `stream_placeholder_text` (default: "thinking...").
    ///
    /// Returns the `SendOutcome` with the message ID of the placeholder,
    /// which the dispatcher uses for subsequent `update` calls.
    async fn begin_stream(&self, msg: &OutboundMessage) -> ChannelResult<Option<SendOutcome>> {
        if !self.stream_placeholder_enabled || self.app_id.is_none() {
            return Ok(None);
        }

        let message_id = self
            .send_message_by_app(&msg.chat_id, &self.stream_placeholder_text)
            .await?;
        info!(
            target: LOG_TARGET,
            chat_id = %msg.chat_id,
            %message_id,
            "stream started"
        );
        Ok(Some(SendOutcome {
            message_id: if message_id.is_empty() {
                None
            } else {
                Some(message_id)
            },
        }))
    }
}

/// Axum request handler for Feishu HTTP callbacks.
///
/// Handles two kinds of requests:
///
/// 1. **URL verification** — responds to the `url_verification` challenge
///    by echoing back the challenge string.
/// 2. **Message events** — parses `im.message.receive_v1` events, verifies
///    the `verify_token` (if configured), checks the allow-from list,
///    and publishes the extracted [`InboundMessage`](nanobot_bus::InboundMessage)
///    to the [`MessageBus`].
///
/// # Returns
/// - `200 OK` with `{"challenge": "..."}` for URL verification.
/// - `200 OK` with `{"code": 0}` for successfully handled message events.
/// - `401 Unauthorized` with `{"code": 401, "msg": "invalid token"}` if
///   the verification token does not match.
///
/// # Non-message events
/// Events of other types are silently acknowledged with `200 OK`.
async fn feishu_event_handler(
    State(state): State<FeishuCallbackState>,
    Json(payload): Json<FeishuIncomingEnvelope>,
) -> (StatusCode, Json<serde_json::Value>) {
    if payload.r#type.as_deref() == Some("url_verification")
        && let Some(challenge) = payload.challenge
    {
        return (StatusCode::OK, Json(json!({ "challenge": challenge })));
    }

    if let Some(expected) = state.verify_token.as_deref()
        && !expected.is_empty()
    {
        let actual = payload
            .header
            .as_ref()
            .and_then(|h| h.token.as_deref())
            .unwrap_or_default();
        if actual != expected {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "code": 401, "msg": "invalid token" })),
            );
        }
    }

    match extract_inbound_message(&state.name, &payload, &state.allow_from) {
        Ok(Some(message)) => {
            if let Err(err) = state.bus.publish_inbound(message) {
                error!(target: LOG_TARGET, "feishu publish inbound failed: {}", err);
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "code": 500, "msg": "publish inbound failed" })),
                );
            }
        }
        Ok(None) => {}
        Err(err) => {
            warn!(target: LOG_TARGET, "feishu inbound parse skipped: {}", err);
        }
    }

    (StatusCode::OK, Json(json!({ "code": 0 })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn env_var(key: &str) -> Option<String> {
        std::env::var(key)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    }

    #[test]
    fn feishu_new_requires_delivery_config() {
        let cfg = FeishuChannelConfig::default();
        let out = FeishuChannel::new("test".into(), cfg, MessageBus::new());
        assert!(out.is_err());
        assert!(
            out.err()
                .map(|e| e.to_string())
                .unwrap_or_default()
                .contains("requires either")
        );
    }

    #[test]
    fn feishu_accepts_bot_key() {
        let cfg = FeishuChannelConfig {
            allow_from: vec!["*".to_string()],
            webhook_url: Some("abc123".to_string()),
            ..Default::default()
        };
        let channel =
            FeishuChannel::new("test".into(), cfg, MessageBus::new()).expect("feishu channel");
        assert_eq!(
            channel.webhook_url.as_deref(),
            Some("https://open.feishu.cn/open-apis/bot/v2/hook/abc123")
        );
    }

    #[test]
    fn split_text_handles_multibyte_content() {
        let parts = split_text("你好 世界\n第二行", 5);
        assert_eq!(
            parts,
            vec![
                "你".to_string(),
                "好".to_string(),
                "世".to_string(),
                "界".to_string(),
                "第".to_string(),
                "二".to_string(),
                "行".to_string()
            ]
        );
    }

    #[test]
    fn feishu_reads_stream_placeholder_config() {
        let cfg = FeishuChannelConfig {
            allow_from: vec!["*".to_string()],
            app_id: "demo".to_string(),
            app_secret: "secret".to_string(),
            stream_placeholder_enabled: Some(true),
            stream_placeholder_text: Some("处理中...".to_string()),
            ..Default::default()
        };

        let channel =
            FeishuChannel::new("test".into(), cfg, MessageBus::new()).expect("feishu channel");
        assert!(channel.stream_placeholder_enabled);
        assert_eq!(channel.stream_placeholder_text, "处理中...");
    }

    #[test]
    fn infer_file_name_from_url_or_path() {
        assert_eq!(
            infer_file_name("https://example.com/assets/pic.png?x=1"),
            "pic.png"
        );
        assert_eq!(infer_file_name("/tmp/demo.jpg"), "demo.jpg");
        assert_eq!(infer_file_name("https://example.com/"), "image.jpg");
    }

    #[test]
    fn infer_image_mime_from_name_supports_common_extensions() {
        assert_eq!(infer_image_mime_from_name("a.png"), Some("image/png"));
        assert_eq!(infer_image_mime_from_name("a.JPG"), Some("image/jpeg"));
        assert_eq!(infer_image_mime_from_name("a.webp"), Some("image/webp"));
        assert_eq!(infer_image_mime_from_name("a.txt"), None);
    }

    #[test]
    fn extract_feishu_image_key_ref_works() {
        assert_eq!(
            extract_feishu_image_key_ref("feishu:image_key:img_123"),
            Some("img_123")
        );
        assert_eq!(
            extract_feishu_image_key_ref("https://example.com/a.png"),
            None
        );
    }

    #[test]
    fn extract_inbound_message_supports_image_event() {
        let payload: FeishuIncomingEnvelope = serde_json::from_value(json!({
            "header": {
                "event_type": "im.message.receive_v1"
            },
            "event": {
                "sender": {
                    "sender_id": {
                        "union_id": "on_test"
                    }
                },
                "message": {
                    "message_id": "om_test",
                    "chat_id": "oc_test",
                    "message_type": "image",
                    "content": "{\"image_key\":\"img_v3_test\"}"
                }
            }
        }))
        .expect("parse payload");
        let inbound = extract_inbound_message("test_channel", &payload, &["*".to_string()])
            .expect("extract inbound");
        let inbound = inbound.expect("image inbound exists");
        assert_eq!(inbound.channel, "test_channel");
        assert_eq!(inbound.chat_id, "oc_test");
        assert_eq!(inbound.content_text(), "[image: img_v3_test]");
        assert_eq!(inbound.media, vec!["feishu:image_key:img_v3_test"]);
    }

    // --- RenderMode tests ---

    #[test]
    fn render_mode_from_str() {
        assert_eq!(RenderMode::from("raw"), RenderMode::Raw);
        assert_eq!(RenderMode::from("card"), RenderMode::Card);
        assert_eq!(RenderMode::from("auto"), RenderMode::Auto);
        assert_eq!(RenderMode::from(""), RenderMode::Raw);
    }

    #[test]
    fn render_mode_display() {
        assert_eq!(RenderMode::Raw.to_string(), "raw");
        assert_eq!(RenderMode::Card.to_string(), "card");
        assert_eq!(RenderMode::Auto.to_string(), "auto");
    }

    #[test]
    fn render_mode_resolve_auto_code_block() {
        assert_eq!(
            RenderMode::Auto.resolve("```rust\nfn main() {}"),
            RenderMode::Card
        );
    }

    #[test]
    fn render_mode_resolve_auto_bold() {
        assert_eq!(RenderMode::Auto.resolve("**bold** text"), RenderMode::Card);
    }

    #[test]
    fn render_mode_resolve_auto_inline_code() {
        assert_eq!(RenderMode::Auto.resolve("use `foo`"), RenderMode::Card);
    }

    #[test]
    fn render_mode_resolve_auto_link() {
        assert_eq!(
            RenderMode::Auto.resolve("click [here](url)"),
            RenderMode::Card
        );
    }

    #[test]
    fn render_mode_resolve_auto_table_stays_raw() {
        assert_eq!(
            RenderMode::Auto.resolve("| A | B |\n|---|---|\n| 1 | 2 |"),
            RenderMode::Raw
        );
    }

    #[test]
    fn render_mode_resolve_auto_plain_stays_raw() {
        assert_eq!(RenderMode::Auto.resolve("hello world"), RenderMode::Raw);
    }

    #[test]
    fn render_mode_resolve_auto_emoji_bullet_triggers_card() {
        let text = "▫️ 标普500：7,580（+0.22%）\n▲ 领涨：科技\n📊 市场情绪";
        assert_eq!(RenderMode::Auto.resolve(text), RenderMode::Card);
    }

    #[test]
    fn render_mode_resolve_raw_stays_raw() {
        assert_eq!(RenderMode::Raw.resolve("```code```"), RenderMode::Raw);
    }

    #[test]
    fn render_mode_resolve_card_stays_card() {
        assert_eq!(RenderMode::Card.resolve("hello"), RenderMode::Card);
    }

    // --- Card builder tests ---

    #[test]
    fn card_content_includes_header_and_markdown() {
        let r = card::build_card_content("hello\nworld").expect("build card");
        assert!(r.contains("wide_screen_mode"));
        assert!(r.contains("markdown"));
        assert!(r.contains("hello"));
    }

    #[test]
    fn card_webhook_content_is_valid() {
        let r = card::build_webhook_card_content("**bold**").expect("build webhook card");
        assert!(r.get("elements").is_some());
    }

    #[test]
    fn card_title_truncates() {
        let long = "A".repeat(150);
        let text = format!("# {}\nbody", long);
        let r = card::build_card_content(&text).expect("build card");
        let json: serde_json::Value = serde_json::from_str(&r).expect("valid json");
        let title = json["header"]["title"]["content"].as_str().expect("title");
        assert_eq!(title.len(), 100);
    }

    // --- ASCII table conversion tests ---

    #[test]
    fn convert_markdown_tables_basic() {
        let result = convert_markdown_tables("| A | B |\n|---|---|\n| 1 | 2 |");
        assert_eq!(result, "A | B\n--- | ---\n1 | 2");
    }

    #[test]
    fn convert_markdown_tables_preserves_surrounding() {
        let result = convert_markdown_tables("Header\n\n| X |\n|---|\n| 1 |\n\nFooter");
        assert!(result.contains("Header"));
        assert!(result.contains("X"));
        assert!(result.contains("Footer"));
    }

    #[test]
    fn convert_markdown_tables_no_tables() {
        assert_eq!(convert_markdown_tables("plain"), "plain");
        assert_eq!(convert_markdown_tables(""), "");
    }

    #[tokio::test]
    async fn feishu_connectivity_check_with_env_credentials() {
        let Some(app_id) = env_var("FEISHU_TEST_APP_ID") else {
            return;
        };
        let Some(app_secret) = env_var("FEISHU_TEST_APP_SECRET") else {
            return;
        };

        let cfg = FeishuChannelConfig {
            allow_from: vec!["*".to_string()],
            app_id: app_id.clone(),
            app_secret: app_secret.clone(),
            api_base: env_var("FEISHU_TEST_API_BASE"),
            ..Default::default()
        };

        let channel =
            FeishuChannel::new("test".into(), cfg, MessageBus::new()).expect("feishu channel");
        channel
            .verify_auth_connectivity()
            .await
            .expect("verify auth connectivity");
        channel
            .verify_im_readiness()
            .await
            .expect("verify IM readiness");
    }
}
