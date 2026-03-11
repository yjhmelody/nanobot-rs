use anyhow::Result;
use async_trait::async_trait;
use tracing::{debug, info};

use super::traits::SessionHook;
use super::types::Session;

const LOG_TARGET: &str = "session_hook";

/// Logging hook that tracks session lifecycle events.
pub struct LoggingHook {
    prefix: String,
}

impl LoggingHook {
    pub fn new(prefix: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
        }
    }
}

#[async_trait]
impl SessionHook for LoggingHook {
    async fn on_create(&self, session: &Session) -> Result<()> {
        info!(
            target: LOG_TARGET,
            prefix = %self.prefix,
            session_key = %session.key,
            "session created"
        );
        Ok(())
    }

    async fn on_before_save(&self, session: &mut Session) -> Result<()> {
        debug!(
            target: LOG_TARGET,
            prefix = %self.prefix,
            session_key = %session.key,
            message_count = session.messages.len(),
            "before save"
        );
        Ok(())
    }

    async fn on_after_save(&self, session: &Session) -> Result<()> {
        debug!(
            target: LOG_TARGET,
            prefix = %self.prefix,
            session_key = %session.key,
            "after save"
        );
        Ok(())
    }

    async fn on_consolidate(&self, session: &Session, messages_consolidated: usize) -> Result<()> {
        info!(
            target: LOG_TARGET,
            prefix = %self.prefix,
            session_key = %session.key,
            messages_consolidated,
            "session consolidated"
        );
        Ok(())
    }

    async fn on_delete(&self, key: &str) -> Result<()> {
        info!(
            target: LOG_TARGET,
            prefix = %self.prefix,
            session_key = %key,
            "session deleted"
        );
        Ok(())
    }
}
