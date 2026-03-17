use crate::session::SessionResult;
use async_trait::async_trait;

use super::memory_store::MemoryStore;
use super::traits::MemoryProvider;

/// Example: Memory provider that combines multiple sources.
pub struct CompositeMemoryProvider {
    providers: Vec<Box<dyn MemoryProvider>>,
}

impl CompositeMemoryProvider {
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
        }
    }

    pub fn add_provider(mut self, provider: Box<dyn MemoryProvider>) -> Self {
        self.providers.push(provider);
        self
    }
}

#[async_trait]
impl MemoryProvider for CompositeMemoryProvider {
    async fn get_context(&self, query: &str, session_key: &str) -> SessionResult<String> {
        let mut contexts = Vec::new();

        for provider in &self.providers {
            if let Ok(ctx) = provider.get_context(query, session_key).await {
                if !ctx.trim().is_empty() {
                    contexts.push(ctx);
                }
            }
        }

        Ok(contexts.join("\n\n---\n\n"))
    }

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

    async fn append_history(&self, entry: &str) -> SessionResult<()> {
        // Append to all providers
        for provider in &self.providers {
            provider.append_history(entry).await?;
        }
        Ok(())
    }
}

/// File-based memory provider adapter.
pub struct FileMemoryProvider {
    store: MemoryStore,
}

impl FileMemoryProvider {
    pub fn new(workspace: &std::path::Path) -> SessionResult<Self> {
        Ok(Self {
            store: MemoryStore::new(workspace)?,
        })
    }

    pub fn memory_dir(&self) -> &std::path::Path {
        self.store.memory_dir()
    }
}

#[async_trait]
impl MemoryProvider for FileMemoryProvider {
    async fn get_context(&self, _query: &str, _session_key: &str) -> SessionResult<String> {
        // Simple implementation: return long-term memory
        // Future: could use query for semantic search
        Ok(self.store.get_memory_context().await)
    }

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

    async fn append_history(&self, entry: &str) -> SessionResult<()> {
        self.store.append_history(entry).await
    }
}
