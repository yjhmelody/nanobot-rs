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

use crate::provider::{AssistantToolCall, ChatMessage, MessageContent, MessageRole};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMetadata {
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionEntry {
    pub role: MessageRole,
    #[serde(default)]
    pub content: Option<MessageContent>,
    #[serde(default)]
    pub timestamp: String,
    #[serde(default)]
    pub tool_calls: Option<Vec<AssistantToolCall>>,
    #[serde(default)]
    pub tool_call_id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub reasoning_content: Option<String>,
    #[serde(default)]
    pub thinking_blocks: Option<Vec<String>>,
}

impl SessionEntry {
    /// Helper to extract text content from a session entry.
    pub fn content_as_text(&self) -> Option<&str> {
        self.content.as_ref().and_then(|c| c.as_text())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub key: String,
    #[serde(default)]
    pub messages: Vec<SessionEntry>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub metadata: SessionMetadata,
    #[serde(default)]
    pub last_consolidated: usize,
}

impl Session {
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

    pub fn clear(&mut self) {
        self.messages.clear();
        self.last_consolidated = 0;
        self.updated_at = Utc::now();
    }

    pub fn get_history(&self, max_messages: usize) -> Vec<ChatMessage> {
        let unconsolidated = if self.last_consolidated <= self.messages.len() {
            &self.messages[self.last_consolidated..]
        } else {
            &[]
        };

        let start = unconsolidated.len().saturating_sub(max_messages);
        let mut sliced: Vec<&SessionEntry> = unconsolidated[start..].iter().collect();

        if let Some(idx) = sliced
            .iter()
            .position(|m| matches!(m.role, MessageRole::User))
        {
            sliced = sliced[idx..].to_vec();
        }

        sliced
            .into_iter()
            .map(|m| ChatMessage {
                role: m.role.clone(),
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub key: String,
    pub updated_at: Option<String>,
    pub path: String,
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
