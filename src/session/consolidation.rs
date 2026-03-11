//! Session message consolidation (compression) functionality.
//!
//! This module provides automatic compression of old session messages to reduce
//! memory usage and context window consumption while preserving conversation continuity.

use anyhow::Result;
use tracing::{debug, info};

use crate::provider::{ChatMessage, LLMProvider, MessageContent, MessageRole};
use crate::types::session::{Session, SessionEntry};

/// Configuration for session consolidation behavior.
#[derive(Debug, Clone)]
pub struct ConsolidationConfig {
    /// Minimum number of messages before consolidation is triggered
    pub min_messages: usize,
    /// Number of recent messages to keep unconsolidated
    pub keep_recent: usize,
    /// Maximum tokens for the consolidation summary request
    pub max_tokens: i32,
}

impl Default for ConsolidationConfig {
    fn default() -> Self {
        Self {
            min_messages: 20,
            keep_recent: 10,
            max_tokens: 1000,
        }
    }
}

/// Consolidates (compresses) old messages in a session by generating a summary.
///
/// # Process
///
/// 1. Check if consolidation is needed (enough messages accumulated)
/// 2. Extract messages to consolidate (old messages before keep_recent window)
/// 3. Generate summary using LLM
/// 4. Replace old messages with a single summary message
/// 5. Update last_consolidated pointer
///
/// # Arguments
///
/// * `session` - The session to consolidate (will be modified in place)
/// * `provider` - LLM provider for generating summaries
/// * `model` - Model name to use for summarization
/// * `config` - Consolidation configuration
///
/// # Returns
///
/// Returns `Ok(true)` if consolidation was performed, `Ok(false)` if skipped.
pub async fn consolidate_session(
    session: &mut Session,
    provider: &dyn LLMProvider,
    model: &str,
    config: &ConsolidationConfig,
) -> Result<bool> {
    let total_messages = session.messages.len();
    let unconsolidated_count = total_messages.saturating_sub(session.last_consolidated);

    // Skip if not enough messages to consolidate
    if unconsolidated_count < config.min_messages {
        debug!(
            session_key = %session.key,
            unconsolidated = unconsolidated_count,
            min_required = config.min_messages,
            "skipping consolidation: not enough messages"
        );
        return Ok(false);
    }

    // Calculate consolidation range
    let consolidate_end = total_messages.saturating_sub(config.keep_recent);
    if consolidate_end <= session.last_consolidated {
        debug!(
            session_key = %session.key,
            "skipping consolidation: no messages to consolidate after keeping recent"
        );
        return Ok(false);
    }

    let messages_to_consolidate = &session.messages[session.last_consolidated..consolidate_end];
    if messages_to_consolidate.is_empty() {
        return Ok(false);
    }

    info!(
        session_key = %session.key,
        consolidating = messages_to_consolidate.len(),
        from = session.last_consolidated,
        to = consolidate_end,
        "consolidating session messages"
    );

    // Generate summary
    let summary = generate_summary(provider, model, messages_to_consolidate, config).await?;

    // Create summary entry
    let summary_entry = SessionEntry {
        role: MessageRole::System,
        content: Some(MessageContent::Text(format!(
            "[Consolidated summary of {} previous messages]\n\n{}",
            messages_to_consolidate.len(),
            summary
        ))),
        timestamp: chrono::Utc::now().to_rfc3339(),
        tool_calls: None,
        tool_call_id: None,
        name: None,
        reasoning_content: None,
        thinking_blocks: None,
    };

    // Replace old messages with summary
    let mut new_messages = Vec::with_capacity(1 + (total_messages - consolidate_end));
    new_messages.push(summary_entry);
    new_messages.extend_from_slice(&session.messages[consolidate_end..]);

    session.messages = new_messages;
    session.last_consolidated = 1; // Summary is at index 0
    session.updated_at = chrono::Utc::now();

    info!(
        session_key = %session.key,
        new_message_count = session.messages.len(),
        "session consolidation completed"
    );

    Ok(true)
}

/// Generates a summary of messages using the LLM provider.
async fn generate_summary(
    provider: &dyn LLMProvider,
    model: &str,
    messages: &[SessionEntry],
    config: &ConsolidationConfig,
) -> Result<String> {
    let conversation_text = format_messages_for_summary(messages);

    let summary_prompt = format!(
        "Summarize the following conversation history concisely. \
        Focus on key topics discussed, important decisions made, and relevant context. \
        Keep the summary under 500 words.\n\n{}",
        conversation_text
    );

    let chat_messages = vec![ChatMessage {
        role: MessageRole::User,
        content: Some(MessageContent::Text(summary_prompt)),
        tool_calls: None,
        tool_call_id: None,
        name: None,
        reasoning_content: None,
        thinking_blocks: None,
    }];

    let request = crate::provider::ChatRequest {
        session_key: None,
        messages: chat_messages,
        tools: None,
        model: Some(model.to_string()),
        max_tokens: config.max_tokens,
        temperature: 0.3,
        reasoning_effort: None,
    };

    let response = provider.chat(request).await;

    let summary = response
        .content
        .unwrap_or_else(|| "Unable to generate summary.".to_string());

    Ok(summary)
}

/// Formats messages into a readable text format for summarization.
fn format_messages_for_summary(messages: &[SessionEntry]) -> String {
    let mut text = String::new();

    for msg in messages {
        let role_label = match msg.role {
            MessageRole::User => "User",
            MessageRole::Assistant => "Assistant",
            MessageRole::System => "System",
            MessageRole::Tool => "Tool",
        };

        if let Some(content) = msg.content_as_text() {
            text.push_str(&format!("{}: {}\n\n", role_label, content));
        }
    }

    text
}

impl SessionEntry {
    /// Helper to extract text content from a session entry.
    fn content_as_text(&self) -> Option<&str> {
        self.content.as_ref().and_then(|c| c.as_text())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::MessageRole;
    use chrono::Utc;

    fn create_test_entry(role: MessageRole, text: &str) -> SessionEntry {
        SessionEntry {
            role,
            content: Some(MessageContent::Text(text.to_string())),
            timestamp: Utc::now().to_rfc3339(),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning_content: None,
            thinking_blocks: None,
        }
    }

    #[test]
    fn format_messages_for_summary_works() {
        let messages = vec![
            create_test_entry(MessageRole::User, "Hello"),
            create_test_entry(MessageRole::Assistant, "Hi there!"),
            create_test_entry(MessageRole::User, "How are you?"),
        ];

        let formatted = format_messages_for_summary(&messages);

        assert!(formatted.contains("User: Hello"));
        assert!(formatted.contains("Assistant: Hi there!"));
        assert!(formatted.contains("User: How are you?"));
    }

    #[test]
    fn consolidation_config_default_values() {
        let config = ConsolidationConfig::default();
        assert_eq!(config.min_messages, 20);
        assert_eq!(config.keep_recent, 10);
        assert_eq!(config.max_tokens, 1000);
    }
}
