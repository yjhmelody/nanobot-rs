use anyhow::Result;
use async_trait::async_trait;

use super::traits::MemoryProvider;
use crate::agent::memory::MemoryStore;

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
    async fn get_context(&self, query: &str, session_key: &str) -> Result<String> {
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
    ) -> Result<()> {
        // Store to all providers
        for provider in &self.providers {
            provider.store(content, session_key, metadata).await?;
        }
        Ok(())
    }

    async fn append_history(&self, entry: &str) -> Result<()> {
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
    pub fn new(workspace: &std::path::Path) -> Result<Self> {
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
    async fn get_context(&self, _query: &str, _session_key: &str) -> Result<String> {
        // Simple implementation: return long-term memory
        // Future: could use query for semantic search
        Ok(self.store.get_memory_context().await)
    }

    async fn store(
        &self,
        content: &str,
        _session_key: &str,
        _metadata: Option<&serde_json::Value>,
    ) -> Result<()> {
        // Simple implementation: overwrite long-term memory
        // Future: could append or merge intelligently
        self.store.write_long_term(content).await
    }

    async fn append_history(&self, entry: &str) -> Result<()> {
        self.store.append_history(entry).await
    }
}
