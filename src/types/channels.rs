use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct TelegramUpdatesResponse {
    pub(crate) ok: bool,
    #[serde(default)]
    pub(crate) result: Vec<TelegramUpdate>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct TelegramUpdate {
    pub(crate) update_id: i64,
    pub(crate) message: Option<TelegramMessage>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct TelegramMessage {
    pub(crate) message_id: i64,
    pub(crate) from: Option<TelegramUser>,
    pub(crate) chat: TelegramChat,
    pub(crate) text: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct TelegramUser {
    pub(crate) id: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct TelegramChat {
    pub(crate) id: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct TelegramSendMessage {
    pub(crate) chat_id: i64,
    pub(crate) text: String,
}
