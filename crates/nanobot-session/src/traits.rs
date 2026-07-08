//! Trait definitions for session and memory management.
//!
//! This module defines the five core traits that the session crate abstracts over.
//! All major components are accessed through these trait interfaces rather than
//! concrete types, following the crate's trait-first design principle.
//!
//! # Traits
//!
//! | Trait | Purpose | Default/example impls |
//! |-------|---------|----------------------|
//! | [`SessionStore`] | Persistence backend | `JsonlSessionStore`, `InMemorySessionStore` |
//! | [`ConsolidationStrategy`] | Session compression | `LlmConsolidationStrategy` |
//! | [`MemoryProvider`] | Long-term memory | `FileMemoryProvider`, `CompositeMemoryProvider` |
//! | [`HistoryTransformer`] | Pre-LLM message pipeline | `SensitiveDataFilter`, `MetadataAnnotator` |
//! | [`SessionHook`] | Lifecycle events | `LoggingHook` |
//!
//! # Contract
//!
//! All trait methods are `async` (via `#[async_trait]`), `Send + Sync`, and
//! return `SessionResult`. Implementors should be careful not to hold
//! `parking_lot::Mutex` guards across `.await` points (use `tokio::sync` instead).
use crate::SessionResult;
use async_trait::async_trait;

use super::types::{Session, SessionEntry, SessionSummary};
use nanobot_provider::ChatMessage;

/// Core trait for session storage and retrieval.
///
/// Implementations can use different backends: file system, database,
/// cloud storage, or in-memory caches.
///
/// # Contract for implementors
///
/// - `get_or_create` and `save` must be idempotent in the sense that saving
///   the same session twice should not duplicate data.
/// - `invalidate` should not fail even if the key is not cached.
/// - `list_sessions` should return sessions sorted by `updated_at` descending
///   (newest first) for a consistent user experience.
/// - `delete` should succeed silently if the session does not exist.
#[async_trait]
pub trait SessionStore: Send + Sync {
    /// Retrieves a session by key, creating a new one if it doesn't exist.
    ///
    /// # Implementation notes
    ///
    /// - Cache-check-first: if a cached session exists, return it immediately.
    /// - On cache miss, load from the backend. If the backend has no data,
    ///   return a fresh `Session::new(key)`.
    /// - The returned session SHOULD be cached before returning.
    async fn get_or_create(&self, key: &str) -> SessionResult<Session>;

    /// Saves a session to the underlying storage.
    ///
    /// # Implementation notes
    ///
    /// - Persist all `session.messages` and metadata atomically where possible.
    /// - Update the in-memory cache after a successful write.
    /// - This is a full-replacement save; the implementation should overwrite
    ///   the existing data for the given session key, not append.
    async fn save(&self, session: &Session) -> SessionResult<()>;

    /// Invalidates the cached version of a session (if caching is used).
    ///
    /// Called after external modifications to force a fresh load on the next
    /// `get_or_create` call. Should be a no-op if the key is not cached.
    async fn invalidate(&self, key: &str);

    /// Lists all available sessions.
    ///
    /// Returns a summary (key, timestamp, path) for each session. The list
    /// should be sorted by `updated_at` descending (newest first).
    async fn list_sessions(&self) -> SessionResult<Vec<SessionSummary>>;

    /// Deletes a session permanently.
    ///
    /// Removes the session from both the backend store and any in-memory cache.
    /// Should succeed silently if the session does not exist.
    async fn delete(&self, key: &str) -> SessionResult<()>;
}

/// Trait for session consolidation (compression) strategies.
///
/// Different implementations can use different approaches:
/// - LLM-based summarization
/// - Rule-based compression
/// - Semantic clustering
/// - Importance scoring
///
/// # Lifecycle
///
/// 1. The session manager calls `should_consolidate` to check whether compression
///    is needed (typically based on message count).
/// 2. If `true`, it calls `consolidate` which modifies the session in place,
///    replacing old messages with a concise summary and advancing
///    `last_consolidated`.
///
/// # Contract for implementors
///
/// - `consolidate` MUST update `session.last_consolidated` to reflect the new
///   boundary between summarised and unsummarised messages.
/// - `consolidate` SHOULD preserve the most recent messages (the "keep recent"
///   window) untouched so that the ongoing conversation is not summarised.
/// - The return value indicates whether any compression actually occurred.
#[async_trait]
pub trait ConsolidationStrategy: Send + Sync {
    /// Checks if a session should be consolidated.
    ///
    /// Typically compares the number of unconsolidated messages against a
    /// configurable threshold (`min_messages`). This is a lightweight check
    /// that should not perform I/O or LLM calls.
    async fn should_consolidate(&self, session: &Session) -> bool;

    /// Consolidates a session, returning the consolidated version.
    ///
    /// The implementation should:
    /// 1. Analyze which messages to consolidate
    /// 2. Generate a summary or compressed representation
    /// 3. Replace old messages with the summary
    /// 4. Update the last_consolidated pointer
    ///
    /// # Returns
    ///
    /// - `Ok(true)` if consolidation was performed and messages were removed.
    /// - `Ok(false)` if consolidation was skipped (e.g., no messages to compress).
    /// - `Err(e)` if the consolidation process failed.
    async fn consolidate(&self, session: &mut Session) -> SessionResult<bool>;
}

/// Trait for memory retrieval and context enrichment.
///
/// Memory providers can implement various strategies:
/// - Long-term memory files (MEMORY.md)
/// - Vector database semantic search
/// - Knowledge graphs
/// - External APIs
///
/// # Contract for implementors
///
/// - `get_context` should return an empty string (not an error) when no relevant
///   memory is found. Errors should be reserved for actual failures (I/O, network).
/// - `store` should not block on expensive re-indexing; the write should be
///   acknowledged eagerly and any background processing deferred.
/// - `append_history` is an append-only operation; implementors should not
///   deduplicate or re-order entries.
#[async_trait]
pub trait MemoryProvider: Send + Sync {
    /// Retrieves relevant memory context for a given query.
    ///
    /// # Arguments
    ///
    /// * `query` - The current user query or context.
    /// * `session_key` - The session identifier for context-specific memory.
    ///
    /// # Returns
    ///
    /// Returns a string containing relevant memory context to be injected
    /// into the system prompt. Returns an empty string if no relevant memory
    /// is found (not an error).
    async fn get_context(&self, query: &str, session_key: &str) -> SessionResult<String>;

    /// Stores new information into long-term memory.
    ///
    /// # Arguments
    ///
    /// * `content` - The content to store.
    /// * `session_key` - The session identifier.
    /// * `metadata` - Optional metadata for indexing/retrieval.
    async fn store(
        &self,
        content: &str,
        session_key: &str,
        metadata: Option<&serde_json::Value>,
    ) -> SessionResult<()>;

    /// Appends an entry to the history log.
    ///
    /// This is an append-only operation. Entries are typically timestamped so
    /// that the log serves as an audit trail of significant events.
    async fn append_history(&self, entry: &str) -> SessionResult<()>;
}

/// Trait for session history transformation.
///
/// Transformers can modify the history before it's sent to the LLM:
/// - Filter sensitive information
/// - Inject additional context
/// - Rewrite messages for clarity
/// - Add metadata annotations
///
/// # Pipeline
///
/// Transformers are applied in order (the output of one becomes the input of
/// the next). This allows composing multiple lightweight transformers instead
/// of building one monolithic filter.
///
/// # Contract for implementors
///
/// - Should not mutate the `session` reference -- it is provided for context
///   only.
/// - Should preserve the ordering and role of messages unless explicitly
///   documented.
/// - Returns `Ok(transformed)` on success; if a transformer encounters a
///   non-recoverable error it should return `Err(e)`.
#[async_trait]
pub trait HistoryTransformer: Send + Sync {
    /// Transforms a list of chat messages.
    ///
    /// # Arguments
    ///
    /// * `messages` - The original message history.
    /// * `session` - The full session for context (read-only).
    ///
    /// # Returns
    ///
    /// Returns the transformed message list.
    async fn transform(
        &self,
        messages: Vec<ChatMessage>,
        session: &Session,
    ) -> SessionResult<Vec<ChatMessage>>;
}

/// Trait for session lifecycle hooks.
///
/// Hooks allow plugins to react to session events:
/// - Session creation
/// - Message addition
/// - Consolidation
/// - Session deletion
///
/// All methods have default no-op implementations so implementors only need
/// to override the events they care about.
///
/// # Contract for implementors
///
/// - Hooks should not panic. Errors are propagated to the caller (the session
///   manager) which may decide to abort the operation.
/// - Hooks should be lightweight and avoid blocking I/O, as they run inside
///   the save/get_or_create/delete path.
/// - The `session` reference passed to hooks is read-only for `on_create`,
///   `on_after_save`, `on_messages_added`, and `on_consolidate`. Only
///   `on_before_save` receives a mutable reference for last-minute changes.
#[async_trait]
pub trait SessionHook: Send + Sync {
    /// Called when a new session is created.
    async fn on_create(&self, session: &Session) -> SessionResult<()> {
        let _ = session;
        Ok(())
    }

    /// Called before a session is saved.
    async fn on_before_save(&self, session: &mut Session) -> SessionResult<()> {
        let _ = session;
        Ok(())
    }

    /// Called after a session is saved.
    async fn on_after_save(&self, session: &Session) -> SessionResult<()> {
        let _ = session;
        Ok(())
    }

    /// Called when messages are added to a session.
    async fn on_messages_added(
        &self,
        session: &Session,
        new_messages: &[SessionEntry],
    ) -> SessionResult<()> {
        let _ = (session, new_messages);
        Ok(())
    }

    /// Called when a session is consolidated.
    async fn on_consolidate(
        &self,
        session: &Session,
        messages_consolidated: usize,
    ) -> SessionResult<()> {
        let _ = (session, messages_consolidated);
        Ok(())
    }

    /// Called when a session is deleted.
    async fn on_delete(&self, key: &str) -> SessionResult<()> {
        let _ = key;
        Ok(())
    }
}
