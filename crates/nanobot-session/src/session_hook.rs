//! Session lifecycle hook implementations.
//!
//! This module provides concrete implementations of the [`SessionHook`] trait.
//! Currently includes:
//!
//! - [`LoggingHook`]: Logs session lifecycle events (create, save, consolidate,
//!   delete) via the `tracing` crate.
//!
//! [`SessionHook`]: crate::traits::SessionHook

use crate::SessionResult;
use async_trait::async_trait;
use tracing::{debug, info};

use super::traits::SessionHook;
use super::types::Session;

/// Logging target used by all events to keep log output scoped and filterable.
const TARGET: &str = "nanobot::session::hook";

/// A [`SessionHook`] that logs session lifecycle events.
///
/// Uses the `tracing` crate's `info!` and `debug!` macros with a scoped
/// target (`nanobot::session::hook`) so that session events can be filtered
/// independently from other log output.
///
/// Each log entry includes:
/// - A configurable `prefix` to distinguish between different channels or
///   session manager instances.
/// - The `session_key` for correlation.
/// - Event-specific metadata (message count, consolidation count, etc.).
///
/// # Example
///
/// ```ignore
/// let manager = SessionManager::new(store)
///     .add_hook(Box::new(LoggingHook::new("telegram")));
/// ```
pub struct LoggingHook {
    /// Distinguishes this hook instance in log output (e.g., channel name).
    prefix: String,
}

impl LoggingHook {
    /// Creates a new logging hook with the given prefix.
    ///
    /// # Arguments
    ///
    /// * `prefix` - A string prepended to log messages to identify the source
    ///   (e.g., `"telegram"`, `"cli"`, `"gateway"`).
    pub fn new(prefix: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
        }
    }
}

#[async_trait]
impl SessionHook for LoggingHook {
    /// Logs session creation at the `info` level.
    async fn on_create(&self, session: &Session) -> SessionResult<()> {
        info!(
            target: TARGET,
            prefix = %self.prefix,
            session_key = %session.key,
            "session created"
        );
        Ok(())
    }

    /// Logs pre-save state at the `debug` level, including message count.
    async fn on_before_save(&self, session: &mut Session) -> SessionResult<()> {
        debug!(
            target: TARGET,
            prefix = %self.prefix,
            session_key = %session.key,
            message_count = session.messages.len(),
            "before save"
        );
        Ok(())
    }

    /// Logs post-save at the `debug` level.
    async fn on_after_save(&self, session: &Session) -> SessionResult<()> {
        debug!(
            target: TARGET,
            prefix = %self.prefix,
            session_key = %session.key,
            "after save"
        );
        Ok(())
    }

    /// Logs consolidation at the `info` level, including how many messages
    /// were compressed.
    async fn on_consolidate(
        &self,
        session: &Session,
        messages_consolidated: usize,
    ) -> SessionResult<()> {
        info!(
            target: TARGET,
            prefix = %self.prefix,
            session_key = %session.key,
            messages_consolidated,
            "session consolidated"
        );
        Ok(())
    }

    /// Logs session deletion at the `info` level.
    async fn on_delete(&self, key: &str) -> SessionResult<()> {
        info!(
            target: TARGET,
            prefix = %self.prefix,
            session_key = %key,
            "session deleted"
        );
        Ok(())
    }
}
