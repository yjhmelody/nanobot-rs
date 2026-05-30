use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde::{Deserializer, Serializer};

use crate::SessionKey;

/// Message identifier used for routing and streaming.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageId {
    /// Provider-specific message id for replies/updates.
    External(String),
    /// Internal progress marker.
    Progress,
    /// Internal tool hint marker.
    ToolHint,
}

impl MessageId {
    /// Raw string sentinel used to identify progress messages.
    pub const PROGRESS_TAG: &'static str = "__progress__";
    /// Raw string sentinel used to identify tool hint messages.
    pub const TOOL_HINT_TAG: &'static str = "__tool_hint__";

    /// Parses a raw string into a `MessageId`, recognising the built-in sentinels.
    pub fn from_raw(value: String) -> Self {
        match value.as_str() {
            Self::PROGRESS_TAG => Self::Progress,
            Self::TOOL_HINT_TAG => Self::ToolHint,
            _ => Self::External(value),
        }
    }

    /// Returns the raw string representation of this message ID.
    pub fn as_raw(&self) -> &str {
        match self {
            Self::External(value) => value,
            Self::Progress => Self::PROGRESS_TAG,
            Self::ToolHint => Self::TOOL_HINT_TAG,
        }
    }

    /// Returns the external ID string if this is an `External` variant, otherwise `None`.
    pub fn external_id(&self) -> Option<&str> {
        match self {
            Self::External(value) => Some(value.as_str()),
            _ => None,
        }
    }

    /// Returns `true` if this is a progress marker.
    pub fn is_progress(&self) -> bool {
        matches!(self, Self::Progress)
    }

    /// Returns `true` if this is a tool hint marker.
    pub fn is_tool_hint(&self) -> bool {
        matches!(self, Self::ToolHint)
    }
}

impl Serialize for MessageId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_raw())
    }
}

impl<'de> Deserialize<'de> for MessageId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Ok(Self::from_raw(value))
    }
}

/// Metadata attached to inbound/outbound messages.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageMetadata {
    #[serde(default)]
    /// Optional per-message identifier from the channel adapter.
    pub message_id: Option<MessageId>,
    #[serde(default)]
    /// Optional stream identifier for correlating progressive updates.
    pub stream_id: Option<String>,
}

/// Message received from an external channel into the bus.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboundMessage {
    /// Source channel name (e.g. `cli`, `telegram`).
    pub channel: String,
    /// Sender identifier from the channel.
    pub sender_id: String,
    /// Conversation or chat id within the channel.
    pub chat_id: String,
    /// Incoming content (plain text or command).
    pub content: InboundContent,
    #[serde(default = "now_utc")]
    /// Timestamp when the message was received (UTC).
    pub timestamp: DateTime<Utc>,
    #[serde(default)]
    /// Optional media attachments as paths or URLs.
    pub media: Vec<String>,
    #[serde(default)]
    /// Optional message metadata (IDs, hints).
    pub metadata: MessageMetadata,
    #[serde(default)]
    /// Override for session key routing.
    pub session_key_override: Option<SessionKey>,
}

impl InboundMessage {
    pub fn session_key(&self) -> SessionKey {
        self.session_key_override
            .clone()
            .unwrap_or_else(|| SessionKey::new(&self.channel, &self.chat_id))
    }

    pub fn command(&self) -> Option<InboundCommand> {
        self.content.command()
    }

    pub fn content_text(&self) -> &str {
        self.content.as_text()
    }
}

/// Built-in control commands encoded in inbound content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboundCommand {
    Help,
    Stop,
    Cancel,
    New,
    Compact,
}

impl InboundCommand {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Help => "/help",
            Self::Stop => "/stop",
            Self::Cancel => "/cancel",
            Self::New => "/new",
            Self::Compact => "/compact",
        }
    }

    pub fn parse(input: &str) -> Option<Self> {
        match input.trim().to_ascii_lowercase().as_str() {
            "/help" => Some(Self::Help),
            "/stop" => Some(Self::Stop),
            "/cancel" => Some(Self::Cancel),
            "/new" => Some(Self::New),
            "/compact" => Some(Self::Compact),
            _ => None,
        }
    }
}

/// Inbound content that can be plain text or a parsed command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum InboundContent {
    Text(String),
    Command(InboundCommand),
}

impl InboundContent {
    pub fn command(&self) -> Option<InboundCommand> {
        match self {
            Self::Text(_) => None,
            Self::Command(command) => Some(*command),
        }
    }

    pub fn as_text(&self) -> &str {
        match self {
            Self::Text(text) => text,
            Self::Command(command) => command.as_str(),
        }
    }
}

impl From<String> for InboundContent {
    fn from(value: String) -> Self {
        match InboundCommand::parse(&value) {
            Some(command) => Self::Command(command),
            None => Self::Text(value),
        }
    }
}

impl From<&str> for InboundContent {
    fn from(value: &str) -> Self {
        Self::from(value.to_string())
    }
}

impl From<InboundCommand> for InboundContent {
    fn from(command: InboundCommand) -> Self {
        Self::Command(command)
    }
}

impl From<InboundContent> for String {
    fn from(content: InboundContent) -> Self {
        match content {
            InboundContent::Text(text) => text,
            InboundContent::Command(command) => command.as_str().to_string(),
        }
    }
}

/// Message emitted by the bus to an outbound channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboundMessage {
    /// Target channel name.
    pub channel: String,
    /// Target chat id within the channel.
    pub chat_id: String,
    /// Outbound text content to deliver.
    pub content: String,
    #[serde(default)]
    /// Optional reply-to message id.
    pub reply_to: Option<String>,
    #[serde(default)]
    /// Optional media attachments as paths or URLs.
    pub media: Vec<String>,
    #[serde(default)]
    /// Optional message metadata (IDs, hints).
    pub metadata: MessageMetadata,
}

fn now_utc() -> DateTime<Utc> {
    Utc::now()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inbound_content_parses_builtin_commands_case_insensitive() {
        let content: InboundContent = " /HeLp ".into();
        assert_eq!(content.command(), Some(InboundCommand::Help));
        assert_eq!(content.as_text(), "/help");
    }

    #[test]
    fn inbound_content_keeps_plain_text() {
        let content: InboundContent = "/help me".into();
        assert_eq!(content.command(), None);
        assert_eq!(content.as_text(), "/help me");
    }

    #[test]
    fn inbound_content_roundtrip_string() {
        let text: String = InboundContent::Command(InboundCommand::Stop).into();
        assert_eq!(text, "/stop");
    }

    #[test]
    fn inbound_content_parses_compact_command() {
        let content: InboundContent = "/compact".into();
        assert_eq!(content.command(), Some(InboundCommand::Compact));
        assert_eq!(content.as_text(), "/compact");
    }

    #[test]
    fn inbound_content_parses_cancel_command() {
        let content: InboundContent = "/cancel".into();
        assert_eq!(content.command(), Some(InboundCommand::Cancel));
        assert_eq!(content.as_text(), "/cancel");
    }

    #[test]
    fn inbound_content_roundtrip_cancel() {
        let text: String = InboundContent::Command(InboundCommand::Cancel).into();
        assert_eq!(text, "/cancel");
    }
}
