//! Legacy session manager - deprecated, use session_store.rs instead.
//!
//! This module is kept for backward compatibility during migration.
//! New code should use JsonlSessionStore from session_store.rs.

use std::path::Path;

use anyhow::Result;

pub use crate::types::session::{Session, SessionEntry, SessionSummary};

use super::session_store::JsonlSessionStore;

/// Legacy session manager - deprecated.
///
/// This is a compatibility wrapper around JsonlSessionStore.
/// Use JsonlSessionStore directly for new code.
#[deprecated(since = "0.1.0", note = "Use JsonlSessionStore from session_store module instead")]
pub struct LegacySessionManager {
    inner: JsonlSessionStore,
}

impl LegacySessionManager {
    /// Creates a new session manager with the specified workspace.
    ///
    /// # Deprecated
    ///
    /// Use `JsonlSessionStore::new()` instead.
    pub fn new(workspace: &Path) -> Result<Self> {
        Ok(Self {
            inner: JsonlSessionStore::new(workspace)?,
        })
    }

    pub async fn get_or_create(&self, key: &str) -> Result<Session> {
        use super::traits::SessionStore;
        self.inner.get_or_create(key).await
    }

    pub async fn save(&self, session: &Session) -> Result<()> {
        use super::traits::SessionStore;
        self.inner.save(session).await
    }

    pub async fn invalidate(&self, key: &str) {
        use super::traits::SessionStore;
        self.inner.invalidate(key).await
    }

    pub fn list_sessions(&self) -> Result<Vec<SessionSummary>> {
        use super::traits::SessionStore;
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.inner.list_sessions())
        })
    }

    pub(crate) fn session_path(&self, key: &str) -> std::path::PathBuf {
        self.inner.session_path(key)
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_history_drops_leading_non_user_messages() {
        use crate::provider::{MessageContent, MessageRole};

        let mut session = Session::new("cli:direct");
        session.messages = vec![
            SessionEntry {
                role: MessageRole::Assistant,
                content: Some(MessageContent::Text("preface".to_string())),
                timestamp: chrono::Utc::now().to_rfc3339(),
                tool_calls: None,
                tool_call_id: None,
                name: None,
                reasoning_content: None,
                thinking_blocks: None,
            },
            SessionEntry {
                role: MessageRole::User,
                content: Some(MessageContent::Text("question".to_string())),
                timestamp: chrono::Utc::now().to_rfc3339(),
                tool_calls: None,
                tool_call_id: None,
                name: None,
                reasoning_content: None,
                thinking_blocks: None,
            },
            SessionEntry {
                role: MessageRole::Assistant,
                content: Some(MessageContent::Text("answer".to_string())),
                timestamp: chrono::Utc::now().to_rfc3339(),
                tool_calls: None,
                tool_call_id: None,
                name: None,
                reasoning_content: None,
                thinking_blocks: None,
            },
        ];

        let history = session.get_history(10);
        assert_eq!(history.len(), 2);
        assert!(matches!(history[0].role, MessageRole::User));
        assert_eq!(history[0].content_as_text(), Some("question"));
        assert_eq!(history[1].content_as_text(), Some("answer"));
    }

    #[test]
    fn session_clear_resets_messages_and_consolidation() {
        let mut session = Session::new("test:clear");
        session.messages.push(SessionEntry {
            role: crate::provider::MessageRole::User,
            content: Some(crate::provider::MessageContent::Text("msg1".to_string())),
            timestamp: chrono::Utc::now().to_rfc3339(),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning_content: None,
            thinking_blocks: None,
        });
        session.last_consolidated = 2;

        session.clear();

        assert!(session.messages.is_empty());
        assert_eq!(session.last_consolidated, 0);
        assert_eq!(session.key, "test:clear");
    }

    #[test]
    fn get_history_respects_max_messages() {
        use crate::provider::{MessageContent, MessageRole};

        let mut session = Session::new("test:max");
        for i in 0..10 {
            session.messages.push(SessionEntry {
                role: if i % 2 == 0 {
                    MessageRole::User
                } else {
                    MessageRole::Assistant
                },
                content: Some(MessageContent::Text(format!("msg{}", i))),
                timestamp: chrono::Utc::now().to_rfc3339(),
                tool_calls: None,
                tool_call_id: None,
                name: None,
                reasoning_content: None,
                thinking_blocks: None,
            });
        }

        let history = session.get_history(3);
        assert!(history.len() <= 3);
    }

    #[test]
    fn get_history_handles_empty_session() {
        let session = Session::new("test:empty");
        let history = session.get_history(10);
        assert!(history.is_empty());
    }

    #[test]
    fn get_history_respects_last_consolidated() {
        use crate::provider::{MessageContent, MessageRole};

        let mut session = Session::new("test:consolidated");
        session.messages.push(SessionEntry {
            role: MessageRole::User,
            content: Some(MessageContent::Text("old1".to_string())),
            timestamp: chrono::Utc::now().to_rfc3339(),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning_content: None,
            thinking_blocks: None,
        });
        session.messages.push(SessionEntry {
            role: MessageRole::Assistant,
            content: Some(MessageContent::Text("old2".to_string())),
            timestamp: chrono::Utc::now().to_rfc3339(),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning_content: None,
            thinking_blocks: None,
        });
        session.last_consolidated = 2;

        session.messages.push(SessionEntry {
            role: MessageRole::User,
            content: Some(MessageContent::Text("new1".to_string())),
            timestamp: chrono::Utc::now().to_rfc3339(),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning_content: None,
            thinking_blocks: None,
        });
        session.messages.push(SessionEntry {
            role: MessageRole::Assistant,
            content: Some(MessageContent::Text("new2".to_string())),
            timestamp: chrono::Utc::now().to_rfc3339(),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning_content: None,
            thinking_blocks: None,
        });

        let history = session.get_history(10);
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].content_as_text(), Some("new1"));
        assert_eq!(history[1].content_as_text(), Some("new2"));
    }
}
