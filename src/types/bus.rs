use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageMetadata {
    #[serde(default)]
    pub message_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboundMessage {
    pub channel: String,
    pub sender_id: String,
    pub chat_id: String,
    pub content: InboundContent,
    #[serde(default = "now_utc")]
    pub timestamp: DateTime<Utc>,
    #[serde(default)]
    pub media: Vec<String>,
    #[serde(default)]
    pub metadata: MessageMetadata,
    #[serde(default)]
    pub session_key_override: Option<String>,
}

impl InboundMessage {
    pub fn session_key(&self) -> String {
        self.session_key_override
            .clone()
            .unwrap_or_else(|| format!("{}:{}", self.channel, self.chat_id))
    }

    pub fn command(&self) -> Option<InboundCommand> {
        self.content.command()
    }

    pub fn content_text(&self) -> &str {
        self.content.as_text()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboundCommand {
    Help,
    Stop,
    New,
}

impl InboundCommand {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Help => "/help",
            Self::Stop => "/stop",
            Self::New => "/new",
        }
    }

    pub fn parse(input: &str) -> Option<Self> {
        match input.trim().to_ascii_lowercase().as_str() {
            "/help" => Some(Self::Help),
            "/stop" => Some(Self::Stop),
            "/new" => Some(Self::New),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum InboundContent {
    Text(String),
    Command(InboundCommand),
}

impl InboundContent {
    pub fn command(&self) -> Option<InboundCommand> {
        match self {
            Self::Text(_) => None,
            Self::Command(command) => Some(*command),
        }
    }

    pub fn as_text(&self) -> &str {
        match self {
            Self::Text(text) => text,
            Self::Command(command) => command.as_str(),
        }
    }
}

impl From<String> for InboundContent {
    fn from(value: String) -> Self {
        match InboundCommand::parse(&value) {
            Some(command) => Self::Command(command),
            None => Self::Text(value),
        }
    }
}

impl From<&str> for InboundContent {
    fn from(value: &str) -> Self {
        Self::from(value.to_string())
    }
}

impl From<InboundCommand> for InboundContent {
    fn from(command: InboundCommand) -> Self {
        Self::Command(command)
    }
}

impl From<InboundContent> for String {
    fn from(content: InboundContent) -> Self {
        match content {
            InboundContent::Text(text) => text,
            InboundContent::Command(command) => command.as_str().to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboundMessage {
    pub channel: String,
    pub chat_id: String,
    pub content: String,
    #[serde(default)]
    pub reply_to: Option<String>,
    #[serde(default)]
    pub media: Vec<String>,
    #[serde(default)]
    pub metadata: MessageMetadata,
}

fn now_utc() -> DateTime<Utc> {
    Utc::now()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inbound_content_parses_builtin_commands_case_insensitive() {
        let content: InboundContent = " /HeLp ".into();
        assert_eq!(content.command(), Some(InboundCommand::Help));
        assert_eq!(content.as_text(), "/help");
    }

    #[test]
    fn inbound_content_keeps_plain_text() {
        let content: InboundContent = "/help me".into();
        assert_eq!(content.command(), None);
        assert_eq!(content.as_text(), "/help me");
    }

    #[test]
    fn inbound_content_roundtrip_string() {
        let text: String = InboundContent::Command(InboundCommand::Stop).into();
        assert_eq!(text, "/stop");
    }
}
