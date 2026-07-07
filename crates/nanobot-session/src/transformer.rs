//! History transformer implementations.
//!
//! This module provides concrete implementations of the [`HistoryTransformer`]
//! trait for pre-LLM message pipeline transforms:
//!
//! - [`SensitiveDataFilter`]: Redacts PII (email addresses, SSNs, credit card
//!   numbers) before messages reach the LLM.
//! - [`MetadataAnnotator`]: Inserts a system message at the start of the
//!   history with session metadata for context.
//!
//! # Pipeline order
//!
//! These transformers are applied in registration order by `SessionManager`.
//! Typically `SensitiveDataFilter` should come first (to redact before any
//! annotation is added), then `MetadataAnnotator` to prepend the context.
//!
//! [`HistoryTransformer`]: crate::traits::HistoryTransformer
//! [`SessionManager`]: crate::session_manager::SessionManager

use crate::SessionResult;
use async_trait::async_trait;

use super::traits::HistoryTransformer;
use super::types::Session;
use nanobot_provider::ChatMessage;

/// A [`HistoryTransformer`] that redacts sensitive information from messages.
///
/// Currently patches the following patterns with `[REDACTED]`:
/// - Email addresses
/// - US Social Security Numbers (SSN)
/// - Credit card numbers
///
/// # Limitations
///
/// - Regex-based matching may produce false positives or negatives. For
///   stronger guarantees, consider a dedicated PII detection service.
/// - Only text content is filtered; structured content (tool results) is
///   passed through unchanged.
///
/// # Example
///
/// ```ignore
/// let filter = SensitiveDataFilter::new()?;
/// let manager = SessionManager::new(store)
///     .add_transformer(Box::new(filter));
/// ```
pub struct SensitiveDataFilter {
    /// Compiled regex patterns for PII detection.
    patterns: Vec<regex::Regex>,
    /// Replacement string applied to all matches.
    replacement: String,
}

impl SensitiveDataFilter {
    /// Creates a new filter with default PII patterns.
    ///
    /// Patterns:
    /// - Email: `\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Z|a-z]{2,}\b`
    /// - SSN: `\b\d{3}-\d{2}-\d{4}\b`
    /// - Credit card: `\b\d{4}[- ]?\d{4}[- ]?\d{4}[- ]?\d{4}\b`
    ///
    /// # Errors
    ///
    /// Returns `SessionError::Regex` if any of the default patterns fail to
    /// compile (should not happen with the built-in patterns).
    pub fn new() -> SessionResult<Self> {
        Ok(Self {
            patterns: vec![
                regex::Regex::new(r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Z|a-z]{2,}\b")?, // Email
                regex::Regex::new(r"\b\d{3}-\d{2}-\d{4}\b")?,                               // SSN
                regex::Regex::new(r"\b\d{4}[- ]?\d{4}[- ]?\d{4}[- ]?\d{4}\b")?, // Credit card
            ],
            replacement: "[REDACTED]".to_string(),
        })
    }

    /// Applies all regex patterns to a text string.
    fn filter_text(&self, text: &str) -> String {
        let mut result = text.to_string();
        for pattern in &self.patterns {
            result = pattern
                .replace_all(&result, self.replacement.as_str())
                .to_string();
        }
        result
    }
}

#[async_trait]
impl HistoryTransformer for SensitiveDataFilter {
    /// Filters sensitive data from each message's text content.
    ///
    /// Non-text `MessageContent` variants (e.g., tool results) are passed
    /// through unchanged.
    async fn transform(
        &self,
        messages: Vec<ChatMessage>,
        _session: &Session,
    ) -> SessionResult<Vec<ChatMessage>> {
        let mut transformed = Vec::with_capacity(messages.len());

        for mut msg in messages {
            if let Some(content) = msg.content {
                msg.content = Some(match content {
                    nanobot_provider::MessageContent::Text(text) => {
                        nanobot_provider::MessageContent::Text(self.filter_text(&text))
                    }
                    other => other,
                });
            }
            transformed.push(msg);
        }

        Ok(transformed)
    }
}

/// A [`HistoryTransformer`] that prepends a system message with session
/// metadata.
///
/// The annotation includes the session key, total message count, and a
/// configurable custom string. This can be useful for debugging or for
/// providing session-level context to the LLM.
///
/// # Example
///
/// ```ignore
/// let annotator = MetadataAnnotator::new("production");
/// let manager = SessionManager::new(store)
///     .add_transformer(Box::new(annotator));
/// ```
pub struct MetadataAnnotator {
    /// Custom annotation string included in the metadata message.
    annotation: String,
}

impl MetadataAnnotator {
    /// Creates a new metadata annotator.
    ///
    /// # Arguments
    ///
    /// * `annotation` - A custom string included in the metadata message
    ///   (e.g., environment name, channel name).
    pub fn new(annotation: impl Into<String>) -> Self {
        Self {
            annotation: annotation.into(),
        }
    }
}

#[async_trait]
impl HistoryTransformer for MetadataAnnotator {
    /// Prepends a system message with session metadata.
    ///
    /// The injected message has the format:
    /// `[Session: <key> | Messages: <count> | <annotation>]`
    async fn transform(
        &self,
        mut messages: Vec<ChatMessage>,
        session: &Session,
    ) -> SessionResult<Vec<ChatMessage>> {
        // Add a system message at the beginning with metadata
        let metadata_msg = ChatMessage {
            role: nanobot_provider::MessageRole::System,
            content: Some(nanobot_provider::MessageContent::Text(format!(
                "[Session: {} | Messages: {} | {}]",
                session.key,
                session.messages.len(),
                self.annotation
            ))),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning_content: None,
            thinking_blocks: None,
        };

        messages.insert(0, metadata_msg);
        Ok(messages)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nanobot_provider::{MessageContent, MessageRole};

    #[tokio::test]
    async fn sensitive_data_filter_redacts_email() {
        let filter = SensitiveDataFilter::new().unwrap();
        let text = "Contact me at user@example.com for details";
        let filtered = filter.filter_text(text);
        assert!(filtered.contains("[REDACTED]"));
        assert!(!filtered.contains("user@example.com"));
    }

    #[tokio::test]
    async fn sensitive_data_filter_redacts_credit_card() {
        let filter = SensitiveDataFilter::new().unwrap();
        let text = "My card is 1234-5678-9012-3456";
        let filtered = filter.filter_text(text);
        assert!(filtered.contains("[REDACTED]"));
        assert!(!filtered.contains("1234-5678-9012-3456"));
    }

    #[tokio::test]
    async fn metadata_annotator_adds_system_message() {
        let annotator = MetadataAnnotator::new("test annotation");
        let session = Session::new("test:session");
        let messages = vec![ChatMessage {
            role: MessageRole::User,
            content: Some(MessageContent::Text("hello".to_string())),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning_content: None,
            thinking_blocks: None,
        }];

        let transformed = annotator.transform(messages, &session).await.unwrap();
        assert_eq!(transformed.len(), 2);
        assert!(matches!(transformed[0].role, MessageRole::System));
    }
}
