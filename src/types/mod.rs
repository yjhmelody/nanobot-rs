use serde::{Deserialize, Serialize};
use std::fmt;

/// Newtype wrapper for session keys.
///
/// A session key uniquely identifies a conversation session, typically
/// formatted as "channel:chat_id" (e.g., "telegram:123456").
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionKey(String);

impl SessionKey {
    /// Creates a new session key from channel and chat_id.
    pub fn new(channel: impl Into<String>, chat_id: impl Into<String>) -> Self {
        Self(format!("{}:{}", channel.into(), chat_id.into()))
    }

    /// Creates a session key from a raw string.
    pub fn from_string(s: String) -> Self {
        Self(s)
    }

    /// Returns the session key as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the session key and returns the inner string.
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl fmt::Display for SessionKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl AsRef<str> for SessionKey {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Newtype wrapper for chat IDs.
///
/// A chat ID identifies a specific conversation within a channel.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ChatId(String);

impl ChatId {
    /// Creates a new chat ID.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Returns the chat ID as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the chat ID and returns the inner string.
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl fmt::Display for ChatId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl AsRef<str> for ChatId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Newtype wrapper for channel names.
///
/// A channel name identifies the communication channel (e.g., "telegram", "cli").
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ChannelName(String);

impl ChannelName {
    /// Creates a new channel name.
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    /// Returns the channel name as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the channel name and returns the inner string.
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl fmt::Display for ChannelName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl AsRef<str> for ChannelName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_key_new_formats_correctly() {
        let key = SessionKey::new("telegram", "123456");
        assert_eq!(key.as_str(), "telegram:123456");
    }

    #[test]
    fn session_key_from_string_preserves_value() {
        let key = SessionKey::from_string("cli:direct".to_string());
        assert_eq!(key.as_str(), "cli:direct");
    }

    #[test]
    fn session_key_display_formats_correctly() {
        let key = SessionKey::new("telegram", "123456");
        assert_eq!(format!("{}", key), "telegram:123456");
    }

    #[test]
    fn session_key_as_ref_returns_str() {
        let key = SessionKey::new("telegram", "123456");
        let s: &str = key.as_ref();
        assert_eq!(s, "telegram:123456");
    }

    #[test]
    fn session_key_into_inner_consumes() {
        let key = SessionKey::new("telegram", "123456");
        let inner = key.into_inner();
        assert_eq!(inner, "telegram:123456");
    }

    #[test]
    fn chat_id_new_creates_correctly() {
        let id = ChatId::new("123456");
        assert_eq!(id.as_str(), "123456");
    }

    #[test]
    fn chat_id_display_formats_correctly() {
        let id = ChatId::new("123456");
        assert_eq!(format!("{}", id), "123456");
    }

    #[test]
    fn channel_name_new_creates_correctly() {
        let name = ChannelName::new("telegram");
        assert_eq!(name.as_str(), "telegram");
    }

    #[test]
    fn channel_name_display_formats_correctly() {
        let name = ChannelName::new("telegram");
        assert_eq!(format!("{}", name), "telegram");
    }

    #[test]
    fn session_key_serialization_is_transparent() {
        let key = SessionKey::new("telegram", "123456");
        let json = serde_json::to_string(&key).unwrap();
        assert_eq!(json, "\"telegram:123456\"");
    }

    #[test]
    fn session_key_deserialization_is_transparent() {
        let json = "\"telegram:123456\"";
        let key: SessionKey = serde_json::from_str(json).unwrap();
        assert_eq!(key.as_str(), "telegram:123456");
    }

    #[test]
    fn chat_id_serialization_is_transparent() {
        let id = ChatId::new("123456");
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"123456\"");
    }

    #[test]
    fn channel_name_serialization_is_transparent() {
        let name = ChannelName::new("telegram");
        let json = serde_json::to_string(&name).unwrap();
        assert_eq!(json, "\"telegram\"");
    }
}
