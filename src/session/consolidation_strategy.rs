//! Adapter implementations for existing session and memory components.
//!
//! This module provides adapters that wrap the existing implementations
//! to conform to the new trait-based architecture.

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

use crate::provider::LLMProvider;
use crate::types::session::Session;

use super::consolidation::ConsolidationConfig;
use super::traits::ConsolidationStrategy;

/// LLM-based consolidation strategy adapter.
pub struct LlmConsolidationStrategy {
    provider: Arc<dyn LLMProvider>,
    model: String,
    config: ConsolidationConfig,
}

impl LlmConsolidationStrategy {
    pub fn new(provider: Arc<dyn LLMProvider>, model: String, config: ConsolidationConfig) -> Self {
        Self {
            provider,
            model,
            config,
        }
    }
}

#[async_trait]
impl ConsolidationStrategy for LlmConsolidationStrategy {
    async fn should_consolidate(&self, session: &Session) -> bool {
        let total_messages = session.messages.len();
        let unconsolidated_count = total_messages.saturating_sub(session.last_consolidated);
        unconsolidated_count >= self.config.min_messages
    }

    async fn consolidate(&self, session: &mut Session) -> Result<bool> {
        super::consolidation::consolidate_session(
            session,
            self.provider.as_ref(),
            &self.model,
            &self.config,
        )
        .await
    }
}
