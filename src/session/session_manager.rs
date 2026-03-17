use super::traits::*;
use super::types::{ConsolidationOutcome, Session, SessionSummary};
use super::SessionResult;
use crate::provider::ChatMessage;

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
    auto_consolidation: bool,
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
            auto_consolidation: true,
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

    /// Enables or disables automatic consolidation on save.
    pub fn with_auto_consolidation(mut self, enabled: bool) -> Self {
        self.auto_consolidation = enabled;
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
    pub async fn get_or_create(&self, key: &str) -> SessionResult<Session> {
        let session = self.store.get_or_create(key).await?;

        if session.messages.is_empty() {
            for hook in &self.hooks {
                hook.on_create(&session).await?;
            }
        }

        Ok(session)
    }

    /// Saves a session with consolidation and hooks.
    pub async fn save(&self, session: &mut Session) -> SessionResult<()> {
        // Run before-save hooks
        for hook in &self.hooks {
            hook.on_before_save(session).await?;
        }

        // Try consolidation if configured
        if self.auto_consolidation
            && let Some(strategy) = &self.consolidation
        {
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

    /// Forces a consolidation pass for the given session.
    pub async fn consolidate_now(
        &self,
        session: &mut Session,
    ) -> SessionResult<ConsolidationOutcome> {
        let Some(strategy) = &self.consolidation else {
            return Ok(ConsolidationOutcome::Disabled);
        };

        for hook in &self.hooks {
            hook.on_before_save(session).await?;
        }

        let messages_before = session.messages.len();
        let consolidated = strategy.consolidate(session).await?;
        let outcome = if consolidated {
            let messages_after = session.messages.len();
            let removed = messages_before.saturating_sub(messages_after);

            for hook in &self.hooks {
                hook.on_consolidate(session, removed).await?;
            }

            ConsolidationOutcome::Consolidated { removed }
        } else {
            ConsolidationOutcome::Skipped
        };

        self.store.save(session).await?;

        for hook in &self.hooks {
            hook.on_after_save(session).await?;
        }

        Ok(outcome)
    }

    /// Gets enriched context from all memory providers.
    pub async fn get_memory_context(
        &self,
        query: &str,
        session_key: &str,
    ) -> SessionResult<String> {
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
    ) -> SessionResult<Vec<ChatMessage>> {
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
    pub async fn list_sessions(&self) -> SessionResult<Vec<SessionSummary>> {
        self.store.list_sessions().await
    }

    /// Deletes a session.
    pub async fn delete(&self, key: &str) -> SessionResult<()> {
        for hook in &self.hooks {
            hook.on_delete(key).await?;
        }

        self.store.delete(key).await
    }
}
