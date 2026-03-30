use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use serde_json::json;

use nanobot_bus::{MessageBus, MessageId, MessageMetadata, OutboundMessage};
use crate::error::{ToolError, ToolResult};
use crate::base::{
    Tool, ToolContext, ToolDefinition, parse_args, tool_definition_from_json,
};
use nanobot_types::tools::MessageArgs;

// Tool descriptions
const MESSAGE_DESC: &str =
    "Send a message to the user. Use this when you want to communicate something.";
const MESSAGE_CONTENT_DESC: &str = "The message content to send";
const MESSAGE_CHANNEL_DESC: &str = "Optional: target channel (telegram, discord, etc.)";
const MESSAGE_CHAT_ID_DESC: &str = "Optional: target chat/user ID";
const MESSAGE_MEDIA_DESC: &str =
    "Optional: list of file paths to attach (images, audio, documents)";

pub struct MessageTool {
    bus: Option<MessageBus>,
    sent_in_turn: AtomicBool,
}

impl MessageTool {
    pub fn new(bus: Option<MessageBus>) -> Self {
        Self {
            bus,
            sent_in_turn: AtomicBool::new(false),
        }
    }

    pub fn definition() -> Arc<ToolDefinition> {
        static DEF: OnceLock<Arc<ToolDefinition>> = OnceLock::new();
        DEF.get_or_init(|| {
            Arc::new(tool_definition_from_json(json!({
                "type": "function",
                "function": {
                    "name": "message",
                    "description": MESSAGE_DESC,
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "content": {
                                "type": "string",
                                "description": MESSAGE_CONTENT_DESC
                            },
                            "channel": {
                                "type": "string",
                                "description": MESSAGE_CHANNEL_DESC
                            },
                            "chat_id": {
                                "type": "string",
                                "description": MESSAGE_CHAT_ID_DESC
                            },
                            "media": {
                                "type": "array",
                                "description": MESSAGE_MEDIA_DESC,
                                "items": {
                                    "type": "string"
                                }
                            }
                        },
                        "required": ["content"]
                    }
                }
            })))
        })
        .clone()
    }

    async fn execute_typed(&self, args: MessageArgs, ctx: &ToolContext) -> ToolResult<String> {
        let channel = args.channel.unwrap_or_else(|| ctx.channel.clone());
        let chat_id = args.chat_id.unwrap_or_else(|| ctx.chat_id.clone());
        let message_id = args
            .message_id
            .map(MessageId::External)
            .or_else(|| ctx.message_id.clone());

        if channel.trim().is_empty() || chat_id.trim().is_empty() {
            return Err(ToolError::execution(
                "message",
                anyhow::anyhow!("no target channel/chat specified"),
            ));
        }

        let media = args.media.unwrap_or_default();

        if let Some(bus) = &self.bus {
            let metadata = MessageMetadata {
                message_id,
                stream_id: None,
            };
            let msg = OutboundMessage {
                channel: channel.clone(),
                chat_id: chat_id.clone(),
                content: args.content,
                reply_to: None,
                media: media.clone(),
                metadata,
            };
            if let Err(e) = bus.publish_outbound(msg) {
                return Err(ToolError::execution(
                    "message",
                    anyhow::anyhow!("sending message: {}", e),
                ));
            }
        }

        if channel == ctx.channel && chat_id == ctx.chat_id {
            self.sent_in_turn.store(true, Ordering::SeqCst);
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

    fn definition(&self) -> Arc<ToolDefinition> {
        Self::definition()
    }

    async fn execute(&self, args_json: &str, ctx: &ToolContext) -> ToolResult<String> {
        let parsed = parse_args::<MessageArgs>(args_json)?;
        self.execute_typed(parsed, ctx).await
    }

    async fn start_turn(&self) -> ToolResult<()> {
        self.sent_in_turn.store(false, Ordering::SeqCst);
        Ok(())
    }

    async fn sent_in_turn(&self) -> ToolResult<bool> {
        Ok(self.sent_in_turn.load(Ordering::SeqCst))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nanobot_bus::MessageBus;
    use nanobot_bus::MessageId;
    use nanobot_types::SessionKey;

    #[tokio::test]
    async fn message_tool_sets_metadata_from_snake_case_fields() {
        let bus = MessageBus::new();
        let tool = MessageTool::new(Some(bus.clone()));

        let ctx = ToolContext {
            channel: "cli".to_string(),
            chat_id: "direct".to_string(),
            session_key: SessionKey::from("cli:direct"),
            message_id: Some(MessageId::External("orig-1".to_string())),
        };

        tool.start_turn().await.expect("start turn");

        // Subscribe before executing to ensure we can receive the message
        let mut rx = bus.subscribe_outbound();

        let out = tool
            .execute(
                r#"{"content":"hello","channel":"cli","chat_id":"direct","message_id":"msg-2"}"#,
                &ctx,
            )
            .await
            .expect("message tool execute");

        assert!(out.contains("Message sent to cli:direct"));
        assert!(tool.sent_in_turn().await.expect("sent_in_turn"));

        let emitted = rx.recv().await.expect("outbound message should exist");
        assert_eq!(emitted.content, "hello");
        assert_eq!(
            emitted.metadata.message_id,
            Some(MessageId::External("msg-2".to_string()))
        );
    }
}
