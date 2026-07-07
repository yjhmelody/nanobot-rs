//! Composite session manager that orchestrates storage, consolidation, memory,
//! history transformation, and lifecycle hooks.
//!
//! [`SessionManager`] is the primary entry point for this crate. It composes the
//! five trait abstractions defined in [`traits`] into a single callable interface
//! that the agent loop interacts with.
//!
//! # Lifecycle
//!
//! ```ignore
//! let manager = SessionManager::new(store)
//!     .with_consolidation(strategy)
//!     .add_memory_provider(memory)
//!     .add_transformer(filter)
//!     .add_hook(logger);
//!
//! let mut session = manager.get_or_create("telegram:123").await?;
//! // ... add messages to session ...
//! manager.save(&mut session).await?;
//! ```
//!
//! # Concurrency
//!
//! The `SessionManager` itself is `Send + Sync` because all its component traits
//! are `Send + Sync`. However, concurrent access to the same session is managed
//! externally (by the agent loop) via per-session locks.
use super::ConsolidationConfig;
use super::SessionResult;
use super::consolidate_session;
use super::traits::*;
use super::types::{ConsolidationOutcome, Session, SessionSummary};
use nanobot_provider::ChatMessage;
use nanobot_provider::LLMProvider;
use std::sync::Arc;

/// Composite session manager that orchestrates multiple components.
///
/// This is the main interface that combines:
/// - Session storage
/// - Consolidation strategy
/// - Memory providers
/// - History transformers
/// - Lifecycle hooks
///
/// # Builder pattern
///
/// Constructed via [`SessionManager::new`] then configured with the builder
/// methods:
/// - [`with_consolidation`](SessionManager::with_consolidation)
/// - [`add_memory_provider`](SessionManager::add_memory_provider)
/// - [`add_transformer`](SessionManager::add_transformer)
/// - [`add_hook`](SessionManager::add_hook)
pub struct SessionManager {
    /// Underlying persistence backend (e.g. JSONL file store).
    store: Box<dyn SessionStore>,
    /// Optional consolidation strategy for compressing long sessions.
    consolidation: Option<Box<dyn ConsolidationStrategy>>,
    /// Whether to run consolidation automatically on `save`.
    auto_consolidation: bool,
    /// Ordered list of memory providers for context enrichment.
    memory_providers: Vec<Box<dyn MemoryProvider>>,
    /// Ordered list of history transformers applied to messages before LLM.
    transformers: Vec<Box<dyn HistoryTransformer>>,
    /// Ordered list of lifecycle hooks.
    hooks: Vec<Box<dyn SessionHook>>,
}

impl SessionManager {
    /// Creates a new session manager with the given store.
    ///
    /// Initially no consolidation, memory providers, transformers, or hooks
    /// are configured. Use the builder methods to add them.
    ///
    /// # Arguments
    ///
    /// * `store` - The persistence backend (e.g. `JsonlSessionStore` or
    ///   `InMemorySessionStore`).
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

    /// Sets the consolidation strategy for compressing long sessions.
    ///
    /// When configured, `save` will periodically summarise old messages to
    /// keep the session history within a reasonable size. See
    /// [`ConsolidationStrategy`] for details.
    ///
    /// [`ConsolidationStrategy`]: crate::traits::ConsolidationStrategy
    pub fn with_consolidation(mut self, strategy: Box<dyn ConsolidationStrategy>) -> Self {
        self.consolidation = Some(strategy);
        self
    }

    /// Enables or disables automatic consolidation on save.
    ///
    /// When enabled (the default), the session manager runs consolidation
    /// checks every time `save` is called. Disable this if you want to
    /// control consolidation manually via [`consolidate_now`].
    ///
    /// [`consolidate_now`]: SessionManager::consolidate_now
    pub fn with_auto_consolidation(mut self, enabled: bool) -> Self {
        self.auto_consolidation = enabled;
        self
    }

    /// Adds a memory provider for long-term context enrichment.
    ///
    /// Multiple providers can be added; they are queried in order and their
    /// results are joined together.
    ///
    /// # Arguments
    ///
    /// * `provider` - A `MemoryProvider` implementation (e.g. `FileMemoryProvider`
    ///   for MEMORY.md-based storage).
    ///
    /// [`FileMemoryProvider`]: crate::memory_provider::FileMemoryProvider
    pub fn add_memory_provider(mut self, provider: Box<dyn MemoryProvider>) -> Self {
        self.memory_providers.push(provider);
        self
    }

    /// Adds a history transformer for pre-LLM message processing.
    ///
    /// Transformers are applied in registration order, with the output of
    /// one becoming the input of the next. Common uses include redacting
    /// sensitive data and injecting metadata annotations.
    ///
    /// # Arguments
    ///
    /// * `transformer` - A `HistoryTransformer` implementation.
    ///
    /// [`HistoryTransformer`]: crate::traits::HistoryTransformer
    pub fn add_transformer(mut self, transformer: Box<dyn HistoryTransformer>) -> Self {
        self.transformers.push(transformer);
        self
    }

    /// Adds a session lifecycle hook.
    ///
    /// Hooks are called in registration order during session operations.
    /// All methods on `SessionHook` have default no-op implementations,
    /// so you can override only the events you care about.
    ///
    /// # Arguments
    ///
    /// * `hook` - A `SessionHook` implementation (e.g. `LoggingHook` for
    ///   observability).
    ///
    /// [`SessionHook`]: crate::traits::SessionHook
    /// [`LoggingHook`]: crate::session_hook::LoggingHook
    pub fn add_hook(mut self, hook: Box<dyn SessionHook>) -> Self {
        self.hooks.push(hook);
        self
    }

    /// Gets or creates a session by key.
    ///
    /// If the session already exists (in the store's cache or on disk), it is
    /// returned. Otherwise, a new empty session is created.
    ///
    /// New sessions trigger `on_create` hooks.
    ///
    /// # Arguments
    ///
    /// * `key` - The session key, typically `"channel:chat_id"`.
    ///
    /// # Returns
    ///
    /// The existing or newly created `Session`.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying store fails to read.
    pub async fn get_or_create(&self, key: &str) -> SessionResult<Session> {
        let session = self.store.get_or_create(key).await?;

        if session.messages.is_empty() {
            for hook in &self.hooks {
                hook.on_create(&session).await?;
            }
        }

        Ok(session)
    }

    /// Saves a session with automatic consolidation and lifecycle hooks.
    ///
    /// This method runs the following pipeline:
    /// 1. Calls `on_before_save` hooks (mutable access for last-minute changes).
    /// 2. If auto-consolidation is enabled and the strategy deems it necessary,
    ///    consolidates old messages into a summary.
    /// 3. Persists the session to the underlying store.
    /// 4. Calls `on_after_save` hooks.
    ///
    /// # Arguments
    ///
    /// * `session` - The session to save. Modified in place if consolidation runs.
    ///
    /// # Errors
    ///
    /// Returns an error if any hook, consolidation step, or store write fails.
    pub async fn save(&self, session: &mut Session) -> SessionResult<()> {
        // Run before-save hooks
        for hook in &self.hooks {
            hook.on_before_save(session).await?;
        }

        // Try consolidation if configured
        if self.auto_consolidation
            && let Some(strategy) = &self.consolidation
            && strategy.should_consolidate(session).await
        {
            let messages_before = session.messages.len();
            if strategy.consolidate(session).await? {
                let messages_after = session.messages.len();
                let consolidated = messages_before.saturating_sub(messages_after);

                for hook in &self.hooks {
                    hook.on_consolidate(session, consolidated).await?;
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

    /// Saves a session using caller-provided consolidation settings.
    ///
    /// This variant allows the caller to override both the `enabled` flag and
    /// the consolidation config for a single save operation, bypassing the
    /// manager's defaults. This is useful when the consolidation decision is
    /// made externally (e.g., by the agent loop based on token usage).
    ///
    /// If the caller's config is not provided (or consolidation is disabled),
    /// the method falls back to the manager's auto-consolidation strategy.
    ///
    /// # Arguments
    ///
    /// * `session` - The session to save.
    /// * `provider` - LLM provider (required for LLM-based consolidation).
    /// * `model` - Model name for summarisation.
    /// * `config` - Optional consolidation configuration override.
    /// * `enabled` - Whether to run caller-specified consolidation.
    ///
    /// # Errors
    ///
    /// Returns an error if hooks, consolidation, or store write fails.
    pub async fn save_with_consolidation(
        &self,
        session: &mut Session,
        provider: &Arc<dyn LLMProvider>,
        model: &str,
        config: Option<&ConsolidationConfig>,
        enabled: bool,
    ) -> SessionResult<()> {
        for hook in &self.hooks {
            hook.on_before_save(session).await?;
        }

        if enabled && let Some(config) = config {
            let messages_before = session.messages.len();
            if consolidate_session(session, provider.as_ref(), model, config).await? {
                let messages_after = session.messages.len();
                let consolidated = messages_before.saturating_sub(messages_after);

                for hook in &self.hooks {
                    hook.on_consolidate(session, consolidated).await?;
                }
            }
        } else if self.auto_consolidation
            && let Some(strategy) = &self.consolidation
            && strategy.should_consolidate(session).await
        {
            let messages_before = session.messages.len();
            if strategy.consolidate(session).await? {
                let messages_after = session.messages.len();
                let consolidated = messages_before.saturating_sub(messages_after);

                for hook in &self.hooks {
                    hook.on_consolidate(session, consolidated).await?;
                }
            }
        }

        self.store.save(session).await?;

        for hook in &self.hooks {
            hook.on_after_save(session).await?;
        }

        Ok(())
    }

    /// Forces a consolidation pass for the given session using the
    /// manager-configured strategy.
    ///
    /// Unlike `save`, this method does NOT run the full save pipeline --
    /// it only consolidates and then persists. Hooks are still triggered.
    ///
    /// # Returns
    ///
    /// A `ConsolidationOutcome` indicating whether consolidation occurred.
    ///
    /// # Errors
    ///
    /// Returns an error if hooks or store write fails.
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

    /// Forces a consolidation pass using caller-provided config.
    ///
    /// Unlike `consolidate_now`, this method uses the provided LLM provider,
    /// model, and config rather than the manager-configured strategy. This is
    /// useful when consolidation parameters need to vary across calls.
    ///
    /// # Arguments
    ///
    /// * `session` - The session to consolidate.
    /// * `provider` - LLM provider for summary generation.
    /// * `model` - Model name for summarisation.
    /// * `config` - Consolidation configuration.
    ///
    /// # Returns
    ///
    /// A `ConsolidationOutcome` indicating whether consolidation occurred.
    ///
    /// # Errors
    ///
    /// Returns an error if hooks, LLM summarisation, or store write fails.
    pub async fn consolidate_now_with_config(
        &self,
        session: &mut Session,
        provider: &Arc<dyn LLMProvider>,
        model: &str,
        config: &ConsolidationConfig,
    ) -> SessionResult<ConsolidationOutcome> {
        for hook in &self.hooks {
            hook.on_before_save(session).await?;
        }

        let messages_before = session.messages.len();
        let consolidated = consolidate_session(session, provider.as_ref(), model, config).await?;
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

    /// Gets enriched context from all registered memory providers.
    ///
    /// Queries each `MemoryProvider` in order and concatenates non-empty
    /// results with double newlines.
    ///
    /// # Arguments
    ///
    /// * `query` - The current user query or context for relevance scoring.
    /// * `session_key` - The session identifier.
    ///
    /// # Returns
    ///
    /// A string of concatenated memory contexts, or an empty string if no
    /// provider returned relevant content.
    pub async fn get_memory_context(
        &self,
        query: &str,
        session_key: &str,
    ) -> SessionResult<String> {
        let mut contexts = Vec::new();

        for provider in &self.memory_providers {
            if let Ok(ctx) = provider.get_context(query, session_key).await
                && !ctx.trim().is_empty()
            {
                contexts.push(ctx);
            }
        }

        Ok(contexts.join("\n\n"))
    }

    /// Gets transformed conversation history for the LLM.
    ///
    /// Calls [`Session::get_history`] to extract the sliding window of
    /// unconsolidated messages, then runs each `HistoryTransformer` in order.
    ///
    /// # Arguments
    ///
    /// * `session` - The session to extract history from.
    /// * `max_messages` - Maximum number of messages to return (from the end).
    ///
    /// # Returns
    ///
    /// A `Vec<ChatMessage>` suitable for passing to an LLM provider.
    ///
    /// # Errors
    ///
    /// Returns an error if any transformer fails.
    ///
    /// [`Session::get_history`]: crate::types::Session::get_history
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

    /// Invalidates a session from the store's cache.
    ///
    /// The next `get_or_create` call for this key will bypass the cache and
    /// load fresh data from the backend.
    ///
    /// # Arguments
    ///
    /// * `key` - The session key to invalidate.
    pub async fn invalidate(&self, key: &str) {
        self.store.invalidate(key).await;
    }

    /// Lists all available sessions.
    ///
    /// Delegates to the underlying store. The returned list is typically
    /// sorted by `updated_at` descending (newest first).
    pub async fn list_sessions(&self) -> SessionResult<Vec<SessionSummary>> {
        self.store.list_sessions().await
    }

    /// Deletes a session permanently.
    ///
    /// Triggers `on_delete` hooks before removing from the store.
    ///
    /// # Arguments
    ///
    /// * `key` - The session key to delete.
    ///
    /// # Errors
    ///
    /// Propagates errors from hooks or the store. If the session does not
    /// exist, the implementation should succeed silently.
    pub async fn delete(&self, key: &str) -> SessionResult<()> {
        for hook in &self.hooks {
            hook.on_delete(key).await?;
        }

        self.store.delete(key).await
    }
}
