//! Session-related types for persistence and management.
//!
//! This module contains types that are specific to session storage and management:
//! - Session: The main session aggregate containing messages and metadata
//! - SessionEntry: Individual message entries stored in sessions
//! - SessionMetadata: Metadata associated with sessions
//! - SessionSummary: Summary information for listing sessions
//! - SessionMetadataLine: Internal type for JSONL metadata serialization

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use nanobot_provider::{AssistantToolCall, ChatMessage, MessageContent, MessageRole};

/// Arbitrary metadata attached to a session.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMetadata {
    /// User-defined tags for filtering and categorisation.
    #[serde(default)]
    pub tags: Vec<String>,
}

/// A single persisted message turn within a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionEntry {
    /// Role of the message author (user, assistant, tool, system).
    pub role: MessageRole,
    /// Optional text or structured content.
    #[serde(default)]
    pub content: Option<MessageContent>,
    /// RFC 3339 timestamp of when this entry was recorded.
    #[serde(default)]
    pub timestamp: String,
    /// Tool calls requested by the assistant in this turn, if any.
    #[serde(default)]
    pub tool_calls: Option<Vec<AssistantToolCall>>,
    /// Tool call ID used to correlate tool results back to a request.
    #[serde(default)]
    pub tool_call_id: Option<String>,
    /// Optional name override for the message author.
    #[serde(default)]
    pub name: Option<String>,
    /// Optional reasoning trace returned by extended-thinking providers.
    #[serde(default)]
    pub reasoning_content: Option<String>,
    /// Optional structured thinking blocks from extended-thinking providers.
    #[serde(default)]
    pub thinking_blocks: Option<Vec<String>>,
}

impl SessionEntry {
    /// Helper to extract text content from a session entry.
    pub fn content_as_text(&self) -> Option<&str> {
        self.content.as_ref().and_then(|c| c.as_text())
    }
}

/// A conversation session holding its full message history.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    /// Unique session key (`channel:chat_id`).
    pub key: String,
    /// Ordered list of persisted message turns.
    #[serde(default)]
    pub messages: Vec<SessionEntry>,
    /// When this session was first created.
    pub created_at: DateTime<Utc>,
    /// When this session was last modified.
    pub updated_at: DateTime<Utc>,
    /// Arbitrary metadata for this session.
    #[serde(default)]
    pub metadata: SessionMetadata,
    /// Index into `messages` marking the end of the last consolidation.
    #[serde(default)]
    pub last_consolidated: usize,
}

impl Session {
    /// Creates a new empty session with the given key.
    pub fn new(key: &str) -> Self {
        let now = Utc::now();
        Self {
            key: key.to_string(),
            messages: Vec::new(),
            created_at: now,
            updated_at: now,
            metadata: SessionMetadata::default(),
            last_consolidated: 0,
        }
    }

    /// Clears all messages and resets the consolidation pointer.
    pub fn clear(&mut self) {
        self.messages.clear();
        self.last_consolidated = 0;
        self.updated_at = Utc::now();
    }

    /// Returns up to `max_messages` unconsolidated messages as `ChatMessage` values,
    /// starting from the first user message in the window.
    pub fn get_history(&self, max_messages: usize) -> Vec<ChatMessage> {
        let unconsolidated = if self.last_consolidated <= self.messages.len() {
            &self.messages[self.last_consolidated..]
        } else {
            &[]
        };
        if unconsolidated.is_empty() || max_messages == 0 {
            return Vec::new();
        }

        let window_start = unconsolidated.len().saturating_sub(max_messages);
        let start = if let Some(rel_idx) = unconsolidated[window_start..]
            .iter()
            .position(|m| matches!(m.role, MessageRole::User))
        {
            window_start + rel_idx
        } else if let Some(prev_user_idx) = unconsolidated[..window_start]
            .iter()
            .rposition(|m| matches!(m.role, MessageRole::User))
        {
            prev_user_idx
        } else {
            return Vec::new();
        };

        unconsolidated[start..]
            .iter()
            .map(|m| ChatMessage {
                role: m.role,
                content: m.content.clone(),
                tool_calls: m.tool_calls.clone(),
                tool_call_id: m.tool_call_id.clone(),
                name: m.name.clone(),
                reasoning_content: m.reasoning_content.clone(),
                thinking_blocks: m.thinking_blocks.clone(),
            })
            .collect()
    }
}

/// Lightweight summary of a session used for listing.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    /// Session key.
    pub key: String,
    /// RFC 3339 timestamp of the last update, if known.
    pub updated_at: Option<String>,
    /// Filesystem path to the session JSONL file.
    pub path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsolidationOutcome {
    /// Consolidation is not configured.
    Disabled,
    /// Consolidation ran but found nothing to compress.
    Skipped,
    /// Consolidation completed and removed messages.
    Consolidated { removed: usize },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SessionMetadataLine {
    #[serde(rename = "_type")]
    pub(crate) line_type: String,
    pub(crate) key: String,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) updated_at: DateTime<Utc>,
    #[serde(default)]
    pub(crate) metadata: SessionMetadata,
    #[serde(default)]
    pub(crate) last_consolidated: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(role: MessageRole, text: &str) -> SessionEntry {
        SessionEntry {
            role,
            content: Some(MessageContent::Text(text.to_string())),
            timestamp: String::new(),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning_content: None,
            thinking_blocks: None,
        }
    }

    #[test]
    fn get_history_backtracks_to_previous_user_when_window_has_no_user() {
        let mut session = Session::new("cli:test");
        session.messages = vec![
            entry(MessageRole::User, "u1"),
            entry(MessageRole::Assistant, "a1"),
            entry(MessageRole::Tool, "t1"),
            entry(MessageRole::Assistant, "a2"),
        ];

        let history = session.get_history(2);
        assert!(!history.is_empty());
        assert!(matches!(history[0].role, MessageRole::User));
        assert_eq!(history.len(), 4);
    }

    #[test]
    fn get_history_returns_empty_when_no_user_exists() {
        let mut session = Session::new("cli:test");
        session.messages = vec![
            entry(MessageRole::Assistant, "a1"),
            entry(MessageRole::Tool, "t1"),
        ];

        let history = session.get_history(10);
        assert!(history.is_empty());
    }
}
