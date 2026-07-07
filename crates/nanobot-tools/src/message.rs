//! Message tool for sending outbound messages to channels.
//!
//! Provides the `message` tool that allows the LLM to send messages to
//! users. Messages are published to the [`MessageBus`] (if available) for
//! delivery by channel adapters.
//!
//! ## Per-turn tracking
//!
//! The tool tracks whether it sent a message in the current agent turn
//! via an `AtomicBool`. The agent loop uses this signal (`sent_in_turn`)
//! to decide whether the LLM needs to produce a visible response or
//! whether the tool's message delivery counts as the response.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use serde_json::json;

use crate::base::{Tool, ToolContext, ToolDefinition, parse_args, tool_definition_from_json};
use crate::error::{ToolError, ToolResult};
use nanobot_bus::{MessageBus, MessageId, MessageMetadata, OutboundMessage};
use nanobot_types::tools::MessageArgs;

// Tool descriptions
const MESSAGE_DESC: &str =
    "Send a message to the user. Use this when you want to communicate something.";
const MESSAGE_CONTENT_DESC: &str = "The message content to send";
const MESSAGE_CHANNEL_DESC: &str = "Optional: target channel (telegram, discord, etc.)";
const MESSAGE_CHAT_ID_DESC: &str = "Optional: target chat/user ID";
const MESSAGE_MEDIA_DESC: &str =
    "Optional: list of file paths to attach (images, audio, documents)";

/// Tool for sending outbound messages to users via configured channels.
///
/// If no [`MessageBus`] is available (e.g., in the CLI-only agent mode),
/// the tool still returns success but does not deliver the message
/// anywhere. This allows the agent loop to proceed when running without
/// a gateway.
pub struct MessageTool {
    /// The message bus for publishing outbound messages, or `None` if
    /// running in headless mode.
    bus: Option<MessageBus>,
    /// Tracks whether a message was sent in the current turn.
    sent_in_turn: AtomicBool,
}

impl MessageTool {
    /// Creates a new `MessageTool`.
    ///
    /// # Arguments
    ///
    /// * `bus` - Optional message bus. If `None`, the tool reports success
    ///   but does not actually deliver messages.
    pub fn new(bus: Option<MessageBus>) -> Self {
        Self {
            bus,
            sent_in_turn: AtomicBool::new(false),
        }
    }

    /// Returns the static tool definition (name: "message").
    ///
    /// Uses a `OnceLock` to cache the definition after first construction.
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

    /// Executes the message tool with strongly-typed arguments.
    ///
    /// Publishes the message to the bus and sets the `sent_in_turn` flag
    /// if the message is for the current conversation context.
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

        // Track whether we sent to the current conversation channel.
        // This is used by the agent loop to decide if the LLM's text
        // response is needed or if the tool message suffices.
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

    /// Resets the `sent_in_turn` flag at the start of each turn.
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
