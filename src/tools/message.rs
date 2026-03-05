use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::bus::MessageBus;
use crate::bus::events::{MessageMetadata, OutboundMessage};
use crate::error::{NanobotError, Result};
use crate::tools::base::{JsonSchema, Tool, ToolContext, ToolDefinition, parse_args, schema_props};

#[derive(Debug, Deserialize)]
pub(crate) struct MessageArgs {
    content: String,
    channel: Option<String>,
    #[serde(alias = "chatId")]
    chat_id: Option<String>,
    #[serde(alias = "messageId")]
    message_id: Option<String>,
    media: Option<Vec<String>>,
}

pub struct MessageTool {
    bus: Option<Arc<MessageBus>>,
    sent_in_turn: Mutex<bool>,
}

impl MessageTool {
    pub fn new(bus: Option<Arc<MessageBus>>) -> Self {
        Self {
            bus,
            sent_in_turn: Mutex::new(false),
        }
    }

    pub fn definition() -> ToolDefinition {
        ToolDefinition::function(
            "message",
            "Send a message to the user. Use this when you want to communicate something.",
            JsonSchema::object(
                schema_props([
                    (
                        "content",
                        JsonSchema::string(Some("The message content to send")),
                    ),
                    (
                        "channel",
                        JsonSchema::string(Some(
                            "Optional: target channel (telegram, discord, etc.)",
                        )),
                    ),
                    (
                        "chat_id",
                        JsonSchema::string(Some("Optional: target chat/user ID")),
                    ),
                    (
                        "media",
                        JsonSchema::array(
                            JsonSchema::string(None),
                            Some(
                                "Optional: list of file paths to attach (images, audio, documents)",
                            ),
                        ),
                    ),
                ]),
                vec!["content"],
            ),
        )
    }

    async fn execute_typed(&self, args: MessageArgs, ctx: &ToolContext) -> Result<String> {
        let channel = args.channel.unwrap_or_else(|| ctx.channel.clone());
        let chat_id = args.chat_id.unwrap_or_else(|| ctx.chat_id.clone());
        let message_id = args.message_id.or_else(|| ctx.message_id.clone());

        if channel.trim().is_empty() || chat_id.trim().is_empty() {
            return Err(NanobotError::tool_execution("message", anyhow::anyhow!("no target channel/chat specified")));
        }

        let media = args.media.unwrap_or_default();

        if let Some(bus) = &self.bus {
            let metadata = MessageMetadata { message_id };
            let msg = OutboundMessage {
                channel: channel.clone(),
                chat_id: chat_id.clone(),
                content: args.content,
                reply_to: None,
                media: media.clone(),
                metadata,
            };
            if let Err(e) = bus.publish_outbound(msg) {
                return Err(NanobotError::tool_execution("message", anyhow::anyhow!("sending message: {}", e)));
            }
        }

        if channel == ctx.channel && chat_id == ctx.chat_id {
            *self.sent_in_turn.lock().await = true;
        }

        let info = if media.is_empty() {
            String::new()
        } else {
            format!(" with {} attachments", media.len())
        };
        Ok(format!("Message sent to {}:{}{}", channel, chat_id, info))
    }
}

#[async_trait]
impl Tool for MessageTool {
    fn name(&self) -> &str {
        "message"
    }

    fn definition(&self) -> ToolDefinition {
        Self::definition()
    }

    async fn execute(&self, args_json: &str, ctx: &ToolContext) -> Result<String> {
        let parsed = parse_args::<MessageArgs>(args_json)?;
        self.execute_typed(parsed, ctx).await
    }

    async fn start_turn(&self) -> Result<()> {
        *self.sent_in_turn.lock().await = false;
        Ok(())
    }

    async fn sent_in_turn(&self) -> Result<bool> {
        Ok(*self.sent_in_turn.lock().await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::MessageBus;

    #[tokio::test]
    async fn message_tool_accepts_camel_case_fields_and_sets_metadata() {
        let bus = Arc::new(MessageBus::new());
        let tool = MessageTool::new(Some(bus.clone()));

        let ctx = ToolContext {
            channel: "cli".to_string(),
            chat_id: "direct".to_string(),
            session_key: "cli:direct".to_string(),
            message_id: Some("orig-1".to_string()),
        };

        tool.start_turn().await.expect("start turn");

        // Subscribe before executing to ensure we can receive the message
        let mut rx = bus.subscribe_outbound();

        let out = tool
            .execute(
                r#"{"content":"hello","channel":"cli","chatId":"direct","messageId":"msg-2"}"#,
                &ctx,
            )
            .await
            .expect("message tool execute");

        assert!(out.contains("Message sent to cli:direct"));
        assert!(tool.sent_in_turn().await.expect("sent_in_turn"));

        let emitted = rx.recv().await.expect("outbound message should exist");
        assert_eq!(emitted.content, "hello");
        assert_eq!(emitted.metadata.message_id.as_deref(), Some("msg-2"));
    }
}
