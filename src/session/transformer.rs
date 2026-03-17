use crate::session::SessionResult;
use async_trait::async_trait;

use super::traits::HistoryTransformer;
use super::types::Session;
use crate::provider::ChatMessage;
/// Example: Transformer that filters out sensitive information.
pub struct SensitiveDataFilter {
    patterns: Vec<regex::Regex>,
    replacement: String,
}

impl SensitiveDataFilter {
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
    async fn transform(
        &self,
        messages: Vec<ChatMessage>,
        _session: &Session,
    ) -> SessionResult<Vec<ChatMessage>> {
        let mut transformed = Vec::with_capacity(messages.len());

        for mut msg in messages {
            if let Some(content) = msg.content {
                msg.content = Some(match content {
                    crate::provider::MessageContent::Text(text) => {
                        crate::provider::MessageContent::Text(self.filter_text(&text))
                    }
                    other => other,
                });
            }
            transformed.push(msg);
        }

        Ok(transformed)
    }
}

/// Example: Transformer that adds metadata annotations.
pub struct MetadataAnnotator {
    annotation: String,
}

impl MetadataAnnotator {
    pub fn new(annotation: impl Into<String>) -> Self {
        Self {
            annotation: annotation.into(),
        }
    }
}

#[async_trait]
impl HistoryTransformer for MetadataAnnotator {
    async fn transform(
        &self,
        mut messages: Vec<ChatMessage>,
        session: &Session,
    ) -> SessionResult<Vec<ChatMessage>> {
        // Add a system message at the beginning with metadata
        let metadata_msg = ChatMessage {
            role: crate::provider::MessageRole::System,
            content: Some(crate::provider::MessageContent::Text(format!(
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
    use crate::provider::{MessageContent, MessageRole};

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
