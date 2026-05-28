use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use base64::Engine;
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use open_lark::Config as OpenLarkConfig;
use open_lark::ws_client::{EventDispatcherHandler, LarkWsClient};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::Sha256;
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

use crate::base::{ChannelAdapter, SendOutcome, is_sender_allowed};
use crate::error::{ChannelError, ChannelResult};
use nanobot_bus::{InboundMessage, MessageBus, MessageId, MessageMetadata, OutboundMessage};
use nanobot_config::schema::GenericChannelConfig;

const FEISHU_API_DEFAULT: &str = "https://open.feishu.cn";
const FEISHU_TEXT_LIMIT: usize = 3000;
const FEISHU_CALLBACK_LISTEN_DEFAULT: &str = "0.0.0.0:19820";
const FEISHU_CALLBACK_PATH_DEFAULT: &str = "/feishu/events";
const LOG_TARGET: &str = "nanobot::channels::feishu";

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Serialize)]
struct FeishuWebhookMessage {
    msg_type: String,
    content: FeishuTextContent,
    #[serde(skip_serializing_if = "Option::is_none")]
    timestamp: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sign: Option<String>,
}

#[derive(Debug, Serialize)]
struct FeishuTextContent {
    text: String,
}

#[derive(Debug, Deserialize)]
struct FeishuIncomingEnvelope {
    #[serde(default)]
    r#type: Option<String>,
    #[serde(default)]
    challenge: Option<String>,
    #[serde(default)]
    header: Option<FeishuEventHeader>,
    #[serde(default)]
    event: Option<FeishuMessageEvent>,
}

#[derive(Debug, Deserialize)]
struct FeishuEventHeader {
    #[serde(default)]
    token: Option<String>,
    #[serde(default)]
    event_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FeishuMessageEvent {
    #[serde(default)]
    sender: Option<FeishuSender>,
    #[serde(default)]
    message: Option<FeishuMessage>,
}

#[derive(Debug, Deserialize)]
struct FeishuSender {
    #[serde(default)]
    sender_id: Option<FeishuSenderId>,
}

#[derive(Debug, Deserialize)]
struct FeishuSenderId {
    #[serde(default)]
    open_id: Option<String>,
    #[serde(default)]
    union_id: Option<String>,
    #[serde(default)]
    user_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FeishuMessage {
    #[serde(default)]
    message_id: Option<String>,
    #[serde(default)]
    chat_id: Option<String>,
    #[serde(default)]
    message_type: Option<String>,
    #[serde(default)]
    content: Option<String>,
}

#[derive(Clone)]
struct FeishuCallbackState {
    bus: MessageBus,
    allow_from: Vec<String>,
    verify_token: Option<String>,
}

#[derive(Clone, Debug)]
struct CachedTenantAccessToken {
    access_token: String,
    expires_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
struct FeishuTenantTokenResponse {
    #[serde(default)]
    code: i64,
    #[serde(default)]
    msg: Option<String>,
    #[serde(default)]
    tenant_access_token: Option<String>,
    #[serde(default)]
    expire: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct FeishuApiResponse<T> {
    #[serde(default)]
    code: i64,
    #[serde(default)]
    msg: Option<String>,
    #[serde(default)]
    data: Option<T>,
}

#[derive(Debug, Default, Deserialize)]
struct FeishuSendMessageData {
    #[serde(default)]
    message_id: Option<String>,
}

pub struct FeishuChannel {
    client: Client,
    bus: MessageBus,
    allow_from: Vec<String>,
    api_base: String,
    webhook_url: Option<String>,
    secret: Option<String>,
    app_id: Option<String>,
    app_secret: Option<String>,
    verify_token: Option<String>,
    callback_listen: Option<String>,
    callback_path: String,
    ws_enabled: bool,
    stream_placeholder_enabled: bool,
    stream_placeholder_text: String,
    running: Arc<AtomicBool>,
    callback_task: Mutex<Option<JoinHandle<()>>>,
    ws_task: Mutex<Option<JoinHandle<()>>>,
    tenant_access_token: Mutex<Option<CachedTenantAccessToken>>,
}

impl FeishuChannel {
    pub fn new(config: GenericChannelConfig, bus: MessageBus) -> ChannelResult<Self> {
        let api_base =
            extra_string(&config, &["apiBase"]).unwrap_or_else(|| FEISHU_API_DEFAULT.to_string());
        let webhook_url = build_webhook_url(&config, &api_base);
        let secret = extra_string(&config, &["secret", "signSecret"]);

        let app_id = extra_string(&config, &["appId", "app_id"]);
        let app_secret = extra_string(&config, &["appSecret", "app_secret"]);
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

        let verify_token = extra_string(&config, &["verifyToken", "eventToken", "token"]);
        let explicit_callback = extra_bool(&config, &["eventEnabled", "callbackEnabled"]);
        let ws_enabled = extra_bool(&config, &["wsEnabled", "ws_enabled", "useWebSocket"])
            .unwrap_or_else(|| app_id.is_some() && explicit_callback != Some(true));

        let callback_listen = if explicit_callback == Some(true)
            || (explicit_callback.is_none() && app_id.is_some() && !ws_enabled)
        {
            Some(
                extra_string(&config, &["callbackListen", "listen"])
                    .unwrap_or_else(|| FEISHU_CALLBACK_LISTEN_DEFAULT.to_string()),
            )
        } else {
            None
        };

        let callback_path = extra_string(&config, &["callbackPath", "eventPath"])
            .unwrap_or_else(|| FEISHU_CALLBACK_PATH_DEFAULT.to_string());
        let stream_placeholder_enabled = extra_bool(
            &config,
            &["streamPlaceholderEnabled", "typingIndicatorEnabled"],
        )
        .unwrap_or(false);
        let stream_placeholder_text =
            extra_string(&config, &["streamPlaceholderText", "typingIndicatorText"])
                .unwrap_or_else(|| "思考中...".to_string());
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|err| {
                ChannelError::adapter("feishu", format!("build reqwest client failed: {err}"))
            })?;

        Ok(Self {
            client,
            bus,
            allow_from: config.allow_from,
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
            running: Arc::new(AtomicBool::new(false)),
            callback_task: Mutex::new(None),
            ws_task: Mutex::new(None),
            tenant_access_token: Mutex::new(None),
        })
    }

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

    async fn start_websocket(&self) -> ChannelResult<()> {
        let ws_config = Arc::new(self.build_openlark_ws_config()?);
        let bus = self.bus.clone();
        let allow_from = self.allow_from.clone();
        let verify_token = self.verify_token.clone();
        let running = self.running.clone();

        let (payload_tx, mut payload_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let dispatcher = EventDispatcherHandler::builder()
            .payload_sender(payload_tx)
            .build();

        let payload_task = tokio::spawn(async move {
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
                        match extract_inbound_message(&envelope, &allow_from) {
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
        });

        let ws_task = tokio::spawn(async move {
            let result = LarkWsClient::open(ws_config, dispatcher).await;
            if let Err(err) = result {
                error!(target: LOG_TARGET, "feishu WebSocket client exited: {}", err);
            }
            payload_task.abort();
        });

        *self.ws_task.lock().await = Some(ws_task);
        info!(target: LOG_TARGET, "feishu WebSocket event subscription started");
        Ok(())
    }

    fn callback_router(state: FeishuCallbackState, path: &str) -> Router {
        Router::new()
            .route(path, post(feishu_event_handler))
            .with_state(state)
    }

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

    async fn send_message_by_app_with_token(
        &self,
        receive_id: &str,
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
                "msg_type": "text",
                "content": content,
            }))
            .send()
            .await
            .map_err(|err| {
                ChannelError::adapter("feishu", format!("send app message request failed: {err}"))
            })?;

        let status = response.status();
        let body_text = response.text().await.map_err(|err| {
            ChannelError::adapter("feishu", format!("read app message response failed: {err}"))
        })?;
        if !status.is_success() {
            return Err(ChannelError::adapter(
                "feishu",
                format!("send app message http {}: {}", status, body_text),
            ));
        }

        let body: FeishuApiResponse<FeishuSendMessageData> = serde_json::from_str(&body_text)
            .map_err(|err| {
                ChannelError::adapter(
                    "feishu",
                    format!("parse app message response failed: {err}; body={body_text}"),
                )
            })?;
        if body.code != 0 {
            return Err(ChannelError::adapter(
                "feishu",
                format!(
                    "send app message rejected: code={} msg={}",
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

    async fn refresh_tenant_access_token(&self) -> ChannelResult<String> {
        let token = self.fetch_tenant_access_token().await?;
        let access_token = token.access_token.clone();
        *self.tenant_access_token.lock().await = Some(token);
        Ok(access_token)
    }

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
}

#[async_trait]
impl ChannelAdapter for FeishuChannel {
    fn name(&self) -> &str {
        "feishu"
    }

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

    async fn send(&self, msg: OutboundMessage) -> ChannelResult<SendOutcome> {
        let text = msg.content.trim();
        if text.is_empty() {
            return Ok(SendOutcome::default());
        }

        let mut last_message_id: Option<String> = None;
        for chunk in split_text(text, FEISHU_TEXT_LIMIT) {
            if self.app_id.is_some() {
                let message_id = self.send_message_by_app(&msg.chat_id, &chunk).await?;
                if !message_id.is_empty() {
                    last_message_id = Some(message_id);
                }
            } else {
                self.send_message_by_webhook(&chunk).await?;
            }
        }
        Ok(SendOutcome {
            message_id: last_message_id,
        })
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    async fn update(&self, message_id: &str, msg: OutboundMessage) -> ChannelResult<()> {
        let text = msg.content.trim();
        if text.is_empty() {
            return Ok(());
        }
        if self.app_id.is_none() {
            let _ = self.send(msg).await?;
            return Ok(());
        }
        self.update_message_by_app(message_id, &msg.chat_id, text)
            .await
    }

    fn supports_stream_updates(&self) -> bool {
        self.app_id.is_some()
    }

    async fn begin_stream(&self, msg: &OutboundMessage) -> ChannelResult<Option<SendOutcome>> {
        if !self.stream_placeholder_enabled || self.app_id.is_none() {
            return Ok(None);
        }

        let message_id = self
            .send_message_by_app(&msg.chat_id, &self.stream_placeholder_text)
            .await?;
        Ok(Some(SendOutcome {
            message_id: if message_id.is_empty() {
                None
            } else {
                Some(message_id)
            },
        }))
    }
}

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

    match extract_inbound_message(&payload, &state.allow_from) {
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

fn extract_inbound_message(
    payload: &FeishuIncomingEnvelope,
    allow_from: &[String],
) -> ChannelResult<Option<InboundMessage>> {
    let event_type = payload
        .header
        .as_ref()
        .and_then(|h| h.event_type.as_deref())
        .unwrap_or_default();
    if event_type != "im.message.receive_v1" {
        return Ok(None);
    }

    let Some(event) = payload.event.as_ref() else {
        return Ok(None);
    };
    let Some(message) = event.message.as_ref() else {
        return Ok(None);
    };
    if message.message_type.as_deref() != Some("text") {
        return Ok(None);
    }

    let sender_id = event
        .sender
        .as_ref()
        .and_then(|s| s.sender_id.as_ref())
        .and_then(|s| {
            s.open_id
                .as_deref()
                .or(s.union_id.as_deref())
                .or(s.user_id.as_deref())
        })
        .ok_or_else(|| ChannelError::adapter("feishu", "missing sender id"))?
        .to_string();
    if !is_sender_allowed(allow_from, &sender_id) {
        return Ok(None);
    }

    let chat_id = message
        .chat_id
        .as_deref()
        .ok_or_else(|| ChannelError::adapter("feishu", "missing chat_id"))?
        .to_string();
    let message_id = message
        .message_id
        .as_deref()
        .ok_or_else(|| ChannelError::adapter("feishu", "missing message_id"))?
        .to_string();
    let content_json = message
        .content
        .as_deref()
        .ok_or_else(|| ChannelError::adapter("feishu", "missing content"))?;
    let content_value: serde_json::Value = serde_json::from_str(content_json)
        .map_err(|err| ChannelError::adapter("feishu", format!("invalid content json: {}", err)))?;
    let text = content_value
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .trim()
        .to_string();
    if text.is_empty() {
        return Ok(None);
    }

    Ok(Some(InboundMessage {
        channel: "feishu".to_string(),
        sender_id,
        chat_id,
        content: text.into(),
        timestamp: chrono::Utc::now(),
        media: Vec::new(),
        metadata: MessageMetadata {
            message_id: Some(MessageId::External(message_id)),
            stream_id: None,
        },
        session_key_override: None,
    }))
}

fn build_webhook_url(cfg: &GenericChannelConfig, api_base: &str) -> Option<String> {
    let webhook_or_key = extra_string(cfg, &["webhook", "webhookUrl", "url", "botKey"])?;
    if webhook_or_key.starts_with("http://") || webhook_or_key.starts_with("https://") {
        return Some(webhook_or_key);
    }
    Some(format!(
        "{}/open-apis/bot/v2/hook/{}",
        api_base.trim_end_matches('/'),
        webhook_or_key
    ))
}

fn build_signature(timestamp: &str, secret: &str) -> ChannelResult<String> {
    let string_to_sign = format!("{}\n{}", timestamp, secret);
    let mut mac = HmacSha256::new_from_slice(string_to_sign.as_bytes()).map_err(|err| {
        ChannelError::adapter("feishu", format!("failed to build signature key: {}", err))
    })?;
    mac.update(&[]);
    Ok(base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes()))
}

fn normalize_path(path: &str) -> String {
    if path.is_empty() || path == "/" {
        return FEISHU_CALLBACK_PATH_DEFAULT.to_string();
    }
    if path.starts_with('/') {
        return path.to_string();
    }
    format!("/{}", path)
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
        let safe_end = floor_char_boundary(&content, max_len);
        let cut = &content[..safe_end];
        let mut pos = cut.rfind('\n').unwrap_or(0);
        if pos == 0 {
            pos = cut.rfind(' ').unwrap_or(safe_end);
        }
        if pos == 0 {
            pos = safe_end;
        }
        chunks.push(content[..pos].to_string());
        content = content[pos..].trim_start().to_string();
    }
    chunks
}

fn serialize_text_content(text: &str) -> ChannelResult<String> {
    serde_json::to_string(&FeishuTextContent {
        text: text.to_string(),
    })
    .map_err(|err| ChannelError::adapter("feishu", format!("serialize content failed: {err}")))
}

fn floor_char_boundary(input: &str, max_len: usize) -> usize {
    let mut boundary = max_len.min(input.len());
    while boundary > 0 && !input.is_char_boundary(boundary) {
        boundary -= 1;
    }
    boundary
}

fn is_retryable_auth_send_error(err: &ChannelError) -> bool {
    let message = err.to_string().to_ascii_lowercase();
    message.contains("401")
        || message.contains("403")
        || message.contains("99991661")
        || message.contains("99991663")
        || message.contains("invalid tenant access token")
        || message.contains("access token")
}

fn is_success_response(body: &serde_json::Value) -> bool {
    if let Some(code) = body.get("code").and_then(|v| v.as_i64()) {
        return code == 0;
    }
    if let Some(code) = body.get("StatusCode").and_then(|v| v.as_i64()) {
        return code == 0;
    }
    true
}

fn error_message(body: &serde_json::Value) -> String {
    if let Some(v) = body
        .get("msg")
        .or_else(|| body.get("message"))
        .or_else(|| body.get("StatusMessage"))
        .and_then(|v| v.as_str())
    {
        return v.to_string();
    }
    body.to_string()
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
        let cfg = GenericChannelConfig::default();
        let out = FeishuChannel::new(cfg, MessageBus::new());
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
        let mut cfg = GenericChannelConfig::default();
        cfg.extra.insert("botKey".to_string(), json!("abc123"));
        cfg.allow_from = vec!["*".to_string()];
        let channel = FeishuChannel::new(cfg, MessageBus::new()).expect("feishu channel");
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
        let mut cfg = GenericChannelConfig::default();
        cfg.allow_from = vec!["*".to_string()];
        cfg.extra.insert("appId".to_string(), json!("demo"));
        cfg.extra.insert("appSecret".to_string(), json!("secret"));
        cfg.extra
            .insert("streamPlaceholderEnabled".to_string(), json!(true));
        cfg.extra
            .insert("streamPlaceholderText".to_string(), json!("处理中..."));

        let channel = FeishuChannel::new(cfg, MessageBus::new()).expect("feishu channel");
        assert!(channel.stream_placeholder_enabled);
        assert_eq!(channel.stream_placeholder_text, "处理中...");
    }

    #[tokio::test]
    async fn feishu_connectivity_check_with_env_credentials() {
        let Some(app_id) = env_var("FEISHU_TEST_APP_ID") else {
            return;
        };
        let Some(app_secret) = env_var("FEISHU_TEST_APP_SECRET") else {
            return;
        };

        let mut cfg = GenericChannelConfig {
            allow_from: vec!["*".to_string()],
            ..GenericChannelConfig::default()
        };
        cfg.extra.insert("appId".to_string(), json!(app_id));
        cfg.extra.insert("appSecret".to_string(), json!(app_secret));
        if let Some(api_base) = env_var("FEISHU_TEST_API_BASE") {
            cfg.extra.insert("apiBase".to_string(), json!(api_base));
        }

        let channel = FeishuChannel::new(cfg, MessageBus::new()).expect("feishu channel");
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
