//! Trait definitions for session and memory management.
//!
//! This module provides a flexible, plugin-based architecture for managing
//! conversation sessions and memory. The trait system allows for:
//!
//! - Multiple storage backends (JSONL, database, cloud, etc.)
//! - Custom consolidation strategies
//! - Memory plugins (vector search, semantic retrieval, etc.)
//! - Context enrichment pipelines

use anyhow::Result;
use async_trait::async_trait;

use crate::provider::ChatMessage;
use crate::types::session::{Session, SessionEntry, SessionSummary};

/// Core trait for session storage and retrieval.
///
/// Implementations can use different backends: file system, database,
/// cloud storage, or in-memory caches.
#[async_trait]
pub trait SessionStore: Send + Sync {
    /// Retrieves a session by key, creating a new one if it doesn't exist.
    async fn get_or_create(&self, key: &str) -> Result<Session>;

    /// Saves a session to the underlying storage.
    async fn save(&self, session: &Session) -> Result<()>;

    /// Invalidates the cached version of a session (if caching is used).
    async fn invalidate(&self, key: &str);

    /// Lists all available sessions.
    async fn list_sessions(&self) -> Result<Vec<SessionSummary>>;

    /// Deletes a session permanently.
    async fn delete(&self, key: &str) -> Result<()>;
}

/// Trait for session consolidation (compression) strategies.
///
/// Different implementations can use different approaches:
/// - LLM-based summarization
/// - Rule-based compression
/// - Semantic clustering
/// - Importance scoring
#[async_trait]
pub trait ConsolidationStrategy: Send + Sync {
    /// Checks if a session should be consolidated.
    async fn should_consolidate(&self, session: &Session) -> bool;

    /// Consolidates a session, returning the consolidated version.
    ///
    /// The implementation should:
    /// 1. Analyze which messages to consolidate
    /// 2. Generate a summary or compressed representation
    /// 3. Replace old messages with the summary
    /// 4. Update the last_consolidated pointer
    async fn consolidate(&self, session: &mut Session) -> Result<bool>;
}

/// Trait for memory retrieval and context enrichment.
///
/// Memory providers can implement various strategies:
/// - Long-term memory files (MEMORY.md)
/// - Vector database semantic search
/// - Knowledge graphs
/// - External APIs
#[async_trait]
pub trait MemoryProvider: Send + Sync {
    /// Retrieves relevant memory context for a given query.
    ///
    /// # Arguments
    ///
    /// * `query` - The current user query or context
    /// * `session_key` - The session identifier for context-specific memory
    ///
    /// # Returns
    ///
    /// Returns a string containing relevant memory context to be injected
    /// into the system prompt.
    async fn get_context(&self, query: &str, session_key: &str) -> Result<String>;

    /// Stores new information into long-term memory.
    ///
    /// # Arguments
    ///
    /// * `content` - The content to store
    /// * `session_key` - The session identifier
    /// * `metadata` - Optional metadata for indexing/retrieval
    async fn store(
        &self,
        content: &str,
        session_key: &str,
        metadata: Option<&serde_json::Value>,
    ) -> Result<()>;

    /// Appends an entry to the history log.
    async fn append_history(&self, entry: &str) -> Result<()>;
}

/// Trait for session history transformation.
///
/// Transformers can modify the history before it's sent to the LLM:
/// - Filter sensitive information
/// - Inject additional context
/// - Rewrite messages for clarity
/// - Add metadata annotations
#[async_trait]
pub trait HistoryTransformer: Send + Sync {
    /// Transforms a list of chat messages.
    ///
    /// # Arguments
    ///
    /// * `messages` - The original message history
    /// * `session` - The full session for context
    ///
    /// # Returns
    ///
    /// Returns the transformed message list.
    async fn transform(
        &self,
        messages: Vec<ChatMessage>,
        session: &Session,
    ) -> Result<Vec<ChatMessage>>;
}

/// Trait for session lifecycle hooks.
///
/// Hooks allow plugins to react to session events:
/// - Session creation
/// - Message addition
/// - Consolidation
/// - Session deletion
#[async_trait]
pub trait SessionHook: Send + Sync {
    /// Called when a new session is created.
    async fn on_create(&self, session: &Session) -> Result<()> {
        let _ = session;
        Ok(())
    }

    /// Called before a session is saved.
    async fn on_before_save(&self, session: &mut Session) -> Result<()> {
        let _ = session;
        Ok(())
    }

    /// Called after a session is saved.
    async fn on_after_save(&self, session: &Session) -> Result<()> {
        let _ = session;
        Ok(())
    }

    /// Called when messages are added to a session.
    async fn on_messages_added(
        &self,
        session: &Session,
        new_messages: &[SessionEntry],
    ) -> Result<()> {
        let _ = (session, new_messages);
        Ok(())
    }

    /// Called when a session is consolidated.
    async fn on_consolidate(&self, session: &Session, messages_consolidated: usize) -> Result<()> {
        let _ = (session, messages_consolidated);
        Ok(())
    }

    /// Called when a session is deleted.
    async fn on_delete(&self, key: &str) -> Result<()> {
        let _ = key;
        Ok(())
    }
}
