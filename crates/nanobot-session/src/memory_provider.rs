//! Memory provider implementations.
//!
//! This module provides concrete implementations of the [`MemoryProvider`] trait:
//!
//! - [`CompositeMemoryProvider`]: Combines multiple providers, querying all of
//!   them and concatenating their results.
//! - [`FileMemoryProvider`]: Adapter over [`MemoryStore`], reading from
//!   MEMORY.md and HISTORY.md files in the workspace.
//!
//! # Architecture
//!
//! The `CompositeMemoryProvider` follows the composite pattern, allowing
//! callers to layer different memory backends (file-based, vector DB, etc.)
//! without changing the consuming code.
//!
//! [`MemoryProvider`]: crate::traits::MemoryProvider
//! [`MemoryStore`]: crate::memory_store::MemoryStore

use crate::SessionResult;
use async_trait::async_trait;

use super::memory_store::MemoryStore;
use super::traits::MemoryProvider;

/// A memory provider that combines multiple child providers.
///
/// Queries fan out to all registered providers; their results are joined with
/// `"\n\n---\n\n"` separators. Store and append_history operations are
/// broadcast to all providers.
///
/// This is useful for composing, e.g., a file-based memory provider with a
/// future vector-database provider without changing the consumer.
pub struct CompositeMemoryProvider {
    providers: Vec<Box<dyn MemoryProvider>>,
}

impl CompositeMemoryProvider {
    /// Creates a new empty composite provider.
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
        }
    }

    /// Adds a child memory provider.
    ///
    /// # Arguments
    ///
    /// * `provider` - A `MemoryProvider` implementation to add to the set.
    pub fn add_provider(mut self, provider: Box<dyn MemoryProvider>) -> Self {
        self.providers.push(provider);
        self
    }
}

impl Default for CompositeMemoryProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MemoryProvider for CompositeMemoryProvider {
    /// Queries all child providers and joins non-empty results.
    async fn get_context(&self, query: &str, session_key: &str) -> SessionResult<String> {
        let mut contexts = Vec::new();

        for provider in &self.providers {
            if let Ok(ctx) = provider.get_context(query, session_key).await
                && !ctx.trim().is_empty()
            {
                contexts.push(ctx);
            }
        }

        Ok(contexts.join("\n\n---\n\n"))
    }

    /// Stores content to all child providers.
    async fn store(
        &self,
        content: &str,
        session_key: &str,
        metadata: Option<&serde_json::Value>,
    ) -> SessionResult<()> {
        // Store to all providers
        for provider in &self.providers {
            provider.store(content, session_key, metadata).await?;
        }
        Ok(())
    }

    /// Appends history to all child providers.
    async fn append_history(&self, entry: &str) -> SessionResult<()> {
        // Append to all providers
        for provider in &self.providers {
            provider.append_history(entry).await?;
        }
        Ok(())
    }
}

/// File-based memory provider adapter.
///
/// Wraps [`MemoryStore`] (which reads/writes MEMORY.md and HISTORY.md) behind
/// the [`MemoryProvider`] trait interface.
///
/// # Storage
///
/// - Long-term memory is stored in `{workspace}/memory/MEMORY.md`.
/// - History log is stored in `{workspace}/memory/HISTORY.md`.
///
/// # Future work
///
/// The current implementation simply overwrites long-term memory on `store`.
/// A future enhancement could merge or intelligently append new information
/// instead of replacing it wholesale.
pub struct FileMemoryProvider {
    store: MemoryStore,
}

impl FileMemoryProvider {
    /// Creates a new file-based memory provider in the given workspace.
    ///
    /// # Arguments
    ///
    /// * `workspace` - The workspace directory path. The `memory/` subdirectory
    ///   will be created if it does not exist.
    ///
    /// # Errors
    ///
    /// Returns an error if the memory directory cannot be created.
    pub fn new(workspace: &std::path::Path) -> SessionResult<Self> {
        Ok(Self {
            store: MemoryStore::new(workspace)?,
        })
    }

    /// Returns a reference to the memory directory path.
    pub fn memory_dir(&self) -> &std::path::Path {
        self.store.memory_dir()
    }
}

#[async_trait]
impl MemoryProvider for FileMemoryProvider {
    /// Retrieves query-aware context from `MEMORY.md`.
    ///
    /// Delegates to [`MemoryStore::get_memory_context_for_query`] with a
    /// maximum of 6 blocks.
    ///
    /// The `_session_key` parameter is currently unused; all sessions share
    /// the same long-term memory file.
    async fn get_context(&self, query: &str, _session_key: &str) -> SessionResult<String> {
        Ok(self.store.get_memory_context_for_query(query, 6).await)
    }

    /// Stores new content into long-term memory, overwriting existing content.
    ///
    /// Currently a simple overwrite. Future versions may merge or append
    /// intelligently.
    async fn store(
        &self,
        content: &str,
        _session_key: &str,
        _metadata: Option<&serde_json::Value>,
    ) -> SessionResult<()> {
        // Simple implementation: overwrite long-term memory
        // Future: could append or merge intelligently
        self.store.write_long_term(content).await
    }

    /// Appends a timestamped entry to the history log.
    async fn append_history(&self, entry: &str) -> SessionResult<()> {
        self.store.append_history(entry).await
    }
}
