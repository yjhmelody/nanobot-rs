use anyhow::Result;

use super::traits::*;
use crate::provider::ChatMessage;
use crate::types::session::{Session, SessionSummary};

/// Composite session manager that orchestrates multiple components.
///
/// This is the main interface that combines:
/// - Session storage
/// - Consolidation strategy
/// - Memory providers
/// - History transformers
/// - Lifecycle hooks
pub struct SessionManager {
    store: Box<dyn SessionStore>,
    consolidation: Option<Box<dyn ConsolidationStrategy>>,
    memory_providers: Vec<Box<dyn MemoryProvider>>,
    transformers: Vec<Box<dyn HistoryTransformer>>,
    hooks: Vec<Box<dyn SessionHook>>,
}

impl SessionManager {
    /// Creates a new session manager with the given store.
    pub fn new(store: Box<dyn SessionStore>) -> Self {
        Self {
            store,
            consolidation: None,
            memory_providers: Vec::new(),
            transformers: Vec::new(),
            hooks: Vec::new(),
        }
    }

    /// Sets the consolidation strategy.
    pub fn with_consolidation(mut self, strategy: Box<dyn ConsolidationStrategy>) -> Self {
        self.consolidation = Some(strategy);
        self
    }

    /// Adds a memory provider.
    pub fn add_memory_provider(mut self, provider: Box<dyn MemoryProvider>) -> Self {
        self.memory_providers.push(provider);
        self
    }

    /// Adds a history transformer.
    pub fn add_transformer(mut self, transformer: Box<dyn HistoryTransformer>) -> Self {
        self.transformers.push(transformer);
        self
    }

    /// Adds a session hook.
    pub fn add_hook(mut self, hook: Box<dyn SessionHook>) -> Self {
        self.hooks.push(hook);
        self
    }

    /// Gets or creates a session.
    pub async fn get_or_create(&self, key: &str) -> Result<Session> {
        let session = self.store.get_or_create(key).await?;

        if session.messages.is_empty() {
            for hook in &self.hooks {
                hook.on_create(&session).await?;
            }
        }

        Ok(session)
    }

    /// Saves a session with consolidation and hooks.
    pub async fn save(&self, session: &mut Session) -> Result<()> {
        // Run before-save hooks
        for hook in &self.hooks {
            hook.on_before_save(session).await?;
        }

        // Try consolidation if configured
        if let Some(strategy) = &self.consolidation {
            if strategy.should_consolidate(session).await {
                let messages_before = session.messages.len();
                if strategy.consolidate(session).await? {
                    let messages_after = session.messages.len();
                    let consolidated = messages_before.saturating_sub(messages_after);

                    for hook in &self.hooks {
                        hook.on_consolidate(session, consolidated).await?;
                    }
                }
            }
        }

        // Save to store
        self.store.save(session).await?;

        // Run after-save hooks
        for hook in &self.hooks {
            hook.on_after_save(session).await?;
        }

        Ok(())
    }

    /// Gets enriched context from all memory providers.
    pub async fn get_memory_context(&self, query: &str, session_key: &str) -> Result<String> {
        let mut contexts = Vec::new();

        for provider in &self.memory_providers {
            if let Ok(ctx) = provider.get_context(query, session_key).await {
                if !ctx.trim().is_empty() {
                    contexts.push(ctx);
                }
            }
        }

        Ok(contexts.join("\n\n"))
    }

    /// Gets transformed history.
    pub async fn get_history(
        &self,
        session: &Session,
        max_messages: usize,
    ) -> Result<Vec<ChatMessage>> {
        let mut history = session.get_history(max_messages);

        for transformer in &self.transformers {
            history = transformer.transform(history, session).await?;
        }

        Ok(history)
    }

    /// Invalidates a session from cache.
    pub async fn invalidate(&self, key: &str) {
        self.store.invalidate(key).await;
    }

    /// Lists all sessions.
    pub async fn list_sessions(&self) -> Result<Vec<SessionSummary>> {
        self.store.list_sessions().await
    }

    /// Deletes a session.
    pub async fn delete(&self, key: &str) -> Result<()> {
        for hook in &self.hooks {
            hook.on_delete(key).await?;
        }

        self.store.delete(key).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    #[test]
    fn session_manager_builder_pattern_works() {
        struct DummyStore;

        #[async_trait]
        impl SessionStore for DummyStore {
            async fn get_or_create(&self, _key: &str) -> Result<Session> {
                Ok(Session::new("test"))
            }
            async fn save(&self, _session: &Session) -> Result<()> {
                Ok(())
            }
            async fn invalidate(&self, _key: &str) {}
            async fn list_sessions(&self) -> Result<Vec<SessionSummary>> {
                Ok(Vec::new())
            }
            async fn delete(&self, _key: &str) -> Result<()> {
                Ok(())
            }
        }

        let manager = SessionManager::new(Box::new(DummyStore));
        assert!(manager.consolidation.is_none());
        assert!(manager.memory_providers.is_empty());
        assert!(manager.transformers.is_empty());
        assert!(manager.hooks.is_empty());
    }
}
