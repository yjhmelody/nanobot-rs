//! Message types for the nanobot pub/sub message bus.
//!
//! This module defines the data structures that flow through the
//! [`MessageBus`]-based event system:
//!
//! - [`InboundMessage`] — messages arriving from external channels (CLI,
//!   Telegram, Feishu, etc.)
//! - [`OutboundMessage`] — messages emitted to external channels.
//! - [`InboundContent`] — message content that may be either plain text or
//!   a built-in control command.
//! - [`MessageId`] — typed identifier that distinguishes provider-generated
//!   IDs from internal sentinels (progress markers, tool hints).
//!
//! # Design
//!
//! - All message types are serialisable via `serde` with `camelCase` field
//!   names for compatibility with the Python codebase's JSON wire format.
//! - [`InboundContent`] uses a custom `From<String>` conversion that
//!   auto-detects control commands (e.g., `/stop`, `/help`), keeping the
//!   parsing logic in one place.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde::{Deserializer, Serializer};

use crate::SessionKey;

/// Message identifier used for routing and streaming.
///
/// Distinguishes between external (provider-assigned) message IDs and
/// internal sentinels used for progress streaming and tool hint routing.
///
/// # Variants
///
/// * `External(String)` — A provider-specific message ID (e.g., a Telegram
///   message ID or Feishu message ID). Used for threaded replies and updates.
/// * `Progress` — Internal sentinel indicating a progress/streaming update.
///   Serialised as `"__progress__"`.
/// * `ToolHint` — Internal sentinel indicating a tool execution hint.
///   Serialised as `"__tool_hint__"`.
///
/// # Custom serde
///
/// Serialises as a plain string via [`as_raw`](MessageId::as_raw) and
/// deserialises by recognising the built-in sentinel strings, delegating
/// other values to the `External` variant.
///
/// # Derive rationale
///
/// - `Clone`: message IDs are shared when fanning out to multiple handlers.
/// - `PartialEq + Eq`: used for comparison in routing logic.
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
    /// Raw string sentinel used to identify progress messages during serialisation.
    ///
    /// When a [`MessageMetadata`] carries this as the `message_id`, downstream
    /// adapters know not to treat it as a real provider message ID.
    pub const PROGRESS_TAG: &'static str = "__progress__";
    /// Raw string sentinel used to identify tool hint messages during serialisation.
    pub const TOOL_HINT_TAG: &'static str = "__tool_hint__";

    /// Parses a raw string into a `MessageId`, recognising the built-in sentinels.
    ///
    /// If the string matches [`PROGRESS_TAG`](Self::PROGRESS_TAG) or
    /// [`TOOL_HINT_TAG`](Self::TOOL_HINT_TAG), the corresponding variant is
    /// returned. Otherwise, the string is wrapped in [`External`](Self::External).
    pub fn from_raw(value: String) -> Self {
        match value.as_str() {
            Self::PROGRESS_TAG => Self::Progress,
            Self::TOOL_HINT_TAG => Self::ToolHint,
            _ => Self::External(value),
        }
    }

    /// Returns the raw string representation of this message ID.
    ///
    /// For [`External`](Self::External) variants, returns the inner string as-is.
    /// For sentinel variants, returns the corresponding `__...__` constant.
    pub fn as_raw(&self) -> &str {
        match self {
            Self::External(value) => value,
            Self::Progress => Self::PROGRESS_TAG,
            Self::ToolHint => Self::TOOL_HINT_TAG,
        }
    }

    /// Returns the external ID string if this is an `External` variant, otherwise `None`.
    ///
    /// Useful for extracting the provider-level message ID for threaded replies.
    pub fn external_id(&self) -> Option<&str> {
        match self {
            Self::External(value) => Some(value.as_str()),
            _ => None,
        }
    }

    /// Returns `true` if this is a progress marker.
    ///
    /// Progress markers are used for streaming intermediate results (e.g.,
    /// token-by-token model output).
    pub fn is_progress(&self) -> bool {
        matches!(self, Self::Progress)
    }

    /// Returns `true` if this is a tool hint marker.
    ///
    /// Tool hint markers signal that the message contains metadata about
    /// tool execution rather than actual conversation content.
    pub fn is_tool_hint(&self) -> bool {
        matches!(self, Self::ToolHint)
    }
}

// Custom serde implementation so `MessageId` round-trips through a plain
// JSON string.  The `#[serde(transparent)]` alternative would not work
// because we have custom logic for the sentinel values.
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
///
/// Carries channel-provided identifiers and streaming correlation info
/// alongside the message content. Not all channels populate every field.
///
/// # Serde notes
///
/// `#[serde(default)]` on each field means missing fields deserialise as
/// `None`/empty, maintaining forwards compatibility.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageMetadata {
    /// Optional per-message identifier from the channel adapter.
    #[serde(default)]
    pub message_id: Option<MessageId>,
    /// Optional stream identifier for correlating progressive updates.
    ///
    /// When a response spans multiple outbound messages (streaming), this
    /// field groups them under a common stream ID.
    #[serde(default)]
    pub stream_id: Option<String>,
}

/// A message received from an external channel, routed through the bus
/// to the agent for processing.
///
/// # Fields
///
/// * `channel` — Source channel name (e.g. `"cli"`, `"telegram"`, `"feishu"`).
/// * `sender_id` — Sender identifier from the channel (user ID).
/// * `chat_id` — Conversation or chat identifier within the channel.
/// * `content` — The incoming payload, either plain text or a control
///   command (e.g., `/stop`).
/// * `timestamp` — UTC timestamp of when the message was received.
/// * `media` — Optional list of media attachment paths or URLs.
/// * `metadata` — Channel-provided metadata (IDs, hints).
/// * `session_key_override` — When set, overrides the default session key
///   derived from `channel:chat_id`. Used for cross-session routing.
///
/// # Session key resolution
///
/// The effective session key is determined by
/// [`session_key`](InboundMessage::session_key): it uses
/// `session_key_override` if present, otherwise falls back to
/// `{channel}:{chat_id}`.
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
    /// Timestamp when the message was received (UTC).
    #[serde(default = "now_utc")]
    pub timestamp: DateTime<Utc>,
    /// Optional media attachments as paths or URLs.
    #[serde(default)]
    pub media: Vec<String>,
    /// Optional message metadata (IDs, hints).
    #[serde(default)]
    pub metadata: MessageMetadata,
    /// Override for session key routing.
    ///
    /// When `Some`, this key is used instead of the default `channel:chat_id`
    /// derivation. This enables scenarios like routing a message from one
    /// session into another.
    #[serde(default)]
    pub session_key_override: Option<SessionKey>,
}

impl InboundMessage {
    /// Returns the session key for this message.
    ///
    /// Uses `session_key_override` if set, otherwise derives the key from
    /// `channel:chat_id`. This is the canonical way to determine which
    /// session a message belongs to.
    pub fn session_key(&self) -> SessionKey {
        self.session_key_override
            .clone()
            .unwrap_or_else(|| SessionKey::new(&self.channel, &self.chat_id))
    }

    /// Returns the control command parsed from this message's content, if any.
    ///
    /// Handy shortcut for checking whether the message is a built-in command
    /// like `/stop` or `/help`.
    pub fn command(&self) -> Option<InboundCommand> {
        self.content.command()
    }

    /// Returns the text representation of this message's content.
    ///
    /// For text messages, this returns the original text. For command
    /// messages, this returns the command string (e.g., `"/help"`).
    pub fn content_text(&self) -> &str {
        self.content.as_text()
    }
}

/// Built-in control commands that can be embedded in inbound message content.
///
/// These commands are parsed from the leading text of an [`InboundMessage`]
/// and trigger special behaviour in the agent (e.g., starting a new session,
/// stopping the current turn).
///
/// # Variants
///
/// | Variant | Command | Effect |
/// |---------|---------|--------|
/// | `Help` | `/help` | Show help text |
/// | `Stop` | `/stop` | Stop the current agent turn |
/// | `Cancel` | `/cancel` | Cancel an in-progress operation |
/// | `New` | `/new` | Start a fresh session |
/// | `Compact` | `/compact` | Compact/truncate session history |
///
/// # Derive rationale
///
/// - `Clone + Copy`: small enum passed by value in routing logic.
/// - `PartialEq + Eq`: compared in match branches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboundCommand {
    Help,
    Stop,
    Cancel,
    New,
    Compact,
}

impl InboundCommand {
    /// Returns the string representation for this command, including the
    /// leading `/` prefix.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Help => "/help",
            Self::Stop => "/stop",
            Self::Cancel => "/cancel",
            Self::New => "/new",
            Self::Compact => "/compact",
        }
    }

    /// Attempts to parse an input string as a control command.
    ///
    /// The input is trimmed and case-normalised to lowercase before matching,
    /// so `" /HeLp "` yields [`Help`](InboundCommand::Help).
    ///
    /// Returns `None` if the string does not match any known command.
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

/// Inbound content that is either plain text or a parsed control command.
///
/// The custom `From<String>` / `Into<String>` serde bridge auto-detects
/// commands during deserialisation, so that e.g. `"/stop"` in JSON becomes
/// [`Command(Stop)`](InboundContent::Command) rather than `"Text("/stop")"`.
///
/// # Conversion logic
///
/// `From<String>` first tries [`InboundCommand::parse`]. If it matches, the
/// result is [`Command`](InboundContent::Command). Otherwise, the string is
/// stored as [`Text`](InboundContent::Text).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum InboundContent {
    /// Plain text content (no recognised command prefix).
    Text(String),
    /// A recognised built-in control command.
    Command(InboundCommand),
}

impl InboundContent {
    /// Returns the parsed command, if any.
    pub fn command(&self) -> Option<InboundCommand> {
        match self {
            Self::Text(_) => None,
            Self::Command(command) => Some(*command),
        }
    }

    /// Returns the text representation of this content.
    ///
    /// For text content, returns the original text. For commands, returns
    /// the command string (e.g., `"/help"`). This always produces a non-empty
    /// string.
    pub fn as_text(&self) -> &str {
        match self {
            Self::Text(text) => text,
            Self::Command(command) => command.as_str(),
        }
    }
}

/// Converts a `String` into [`InboundContent`], auto-detecting commands.
///
/// If the string matches a known command pattern (e.g., `"/stop"`), the
/// [`Command`](InboundContent::Command) variant is returned. Otherwise, the
/// string is wrapped in [`Text`](InboundContent::Text).
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

/// A message emitted by the bus to an external channel adapter for delivery.
///
/// Created by the agent after processing an [`InboundMessage`] and sent
/// back through the bus to the appropriate channel adapter's outbound
/// handler.
///
/// # Fields
///
/// * `channel` — Target channel name (e.g. `"telegram"`).
/// * `chat_id` — Target chat/conversation identifier within the channel.
/// * `content` — Outbound text content to deliver to the user.
/// * `reply_to` — Optional message ID to reply to (for threaded replies).
/// * `media` — Optional list of media attachment paths or URLs.
/// * `metadata` — Channel-specific metadata (e.g., stream correlation ID).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboundMessage {
    /// Target channel name.
    pub channel: String,
    /// Target chat id within the channel.
    pub chat_id: String,
    /// Outbound text content to deliver.
    pub content: String,
    /// Optional reply-to message id.
    #[serde(default)]
    pub reply_to: Option<String>,
    /// Optional media attachments as paths or URLs.
    #[serde(default)]
    pub media: Vec<String>,
    /// Optional message metadata (IDs, hints).
    #[serde(default)]
    pub metadata: MessageMetadata,
}

/// Returns the current UTC time. Used as a `serde(default)` for timestamp fields.
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
