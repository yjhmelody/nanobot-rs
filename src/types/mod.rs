pub mod agent;
pub mod bus;
pub mod channels;
pub mod config;
pub mod cron;
pub mod heartbeat;
pub mod provider;
pub mod provider_anthropic;
pub mod provider_openai;
pub mod task;
pub mod tools;

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

    /// Returns true if the session key is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Default for SessionKey {
    fn default() -> Self {
        Self(String::new())
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

impl From<String> for SessionKey {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for SessionKey {
    fn from(s: &str) -> Self {
        Self(s.to_string())
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
    fn session_key_from_str_converts() {
        let key = SessionKey::from("cli:direct");
        assert_eq!(key.as_str(), "cli:direct");
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
}
