use serde::{Deserialize, Serialize};

/// Telegram getUpdates response wrapper.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct TelegramUpdatesResponse {
    pub(crate) ok: bool,
    #[serde(default)]
    pub(crate) result: Vec<TelegramUpdate>,
}

/// Telegram update envelope.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct TelegramUpdate {
    pub(crate) update_id: i64,
    pub(crate) message: Option<TelegramMessage>,
}

/// Telegram message payload used by the channel adapter.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct TelegramMessage {
    pub(crate) message_id: i64,
    pub(crate) from: Option<TelegramUser>,
    pub(crate) chat: TelegramChat,
    pub(crate) text: Option<String>,
}

/// Telegram user identity fields needed for routing.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct TelegramUser {
    pub(crate) id: i64,
}

/// Telegram chat identity fields needed for routing.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct TelegramChat {
    pub(crate) id: i64,
}

/// Telegram sendMessage request body.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct TelegramSendMessage {
    pub(crate) chat_id: i64,
    pub(crate) text: String,
}
