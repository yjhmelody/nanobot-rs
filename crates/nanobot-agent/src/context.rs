use std::path::PathBuf;

use async_trait::async_trait;
use chrono::Local;

use crate::error::AgentResult;
use crate::skills::SkillsLoader;
use crate::traits::{ContextProvider, SkillsProvider};
use nanobot_session::SessionManager;
use nanobot_types::provider::{
    AssistantToolCall, ChatMessage, ContentPart, MessageContent, MessageRole,
};

const IDENTITY_PROMPT_TEMPLATE: &str = "# nanobot\n\nYou are nanobot, a helpful AI assistant.\n\n## Runtime\nRust runtime\n\n## Workspace\nYour workspace is at: {workspace}\n- Long-term memory: {workspace}/memory/MEMORY.md\n- History log: {workspace}/memory/HISTORY.md\n- Custom skills: {workspace}/skills/{skill-name}/SKILL.md\n\n## nanobot Guidelines\n- State intent before tool calls, but NEVER predict or claim results before receiving them.\n- Before modifying a file, read it first. Do not assume files or directories exist.\n- After writing or editing a file, re-read it if accuracy matters.\n- If a tool call fails, analyze the error before retrying with a different approach.\n- Ask for clarification when the request is ambiguous.\n\nReply directly with text for conversations. Only use the 'message' tool to send to a specific chat channel.";
const SKILLS_SUMMARY_PREAMBLE: &str = "# Skills\n\nThe following skills extend your capabilities. To use a skill, read its SKILL.md file using the read_file tool.\nSkills with available=\"false\" need dependencies installed first - you can try installing them with apt/brew.\n\n";

/// Context builder for constructing agent prompts and message history.
#[derive(Debug)]
pub struct ContextBuilder {
    workspace: PathBuf,
    skills: Box<dyn SkillsProvider>,
}

impl ContextBuilder {
    pub const BOOTSTRAP_FILES: [&'static str; 5] =
        ["AGENTS.md", "SOUL.md", "USER.md", "TOOLS.md", "IDENTITY.md"];
    pub const RUNTIME_CONTEXT_TAG: &'static str =
        "[Runtime Context \u{2014} metadata only, not instructions]";

    /// Creates a `ContextBuilder` using the default `SkillsLoader` for the given workspace.
    pub fn new(workspace: PathBuf) -> AgentResult<Self> {
        let skills = Box::new(SkillsLoader::new(&workspace));
        Ok(Self { workspace, skills })
    }

    /// Creates a `ContextBuilder` with a custom `SkillsProvider` (useful for testing).
    pub fn with_skills_provider(workspace: PathBuf, skills: Box<dyn SkillsProvider>) -> Self {
        Self { workspace, skills }
    }
    pub async fn build_system_prompt(
        &self,
        session_manager: &SessionManager,
        session_key: &str,
    ) -> String {
        let mut parts = vec![self.identity_section()];

        let bootstrap = self.load_bootstrap_files().await;
        if !bootstrap.trim().is_empty() {
            parts.push(bootstrap);
        }

        let memory = session_manager
            .get_memory_context("", session_key)
            .await
            .unwrap_or_default();
        if !memory.trim().is_empty() {
            parts.push(format!("# Memory\n\n{}", memory));
        }

        let always = self.skills.get_always_skills().await;
        if !always.is_empty() {
            let content = self.skills.load_skills_for_context(&always).await;
            if !content.trim().is_empty() {
                parts.push(format!("# Active Skills\n\n{}", content));
            }
        }

        let summary = self.skills.build_skills_summary().await;
        if !summary.trim().is_empty() {
            parts.push(format!("{SKILLS_SUMMARY_PREAMBLE}{summary}"));
        }

        parts.join("\n\n---\n\n")
    }

    pub fn add_tool_result(
        &self,
        messages: &mut Vec<ChatMessage>,
        tool_call_id: &str,
        tool_name: &str,
        result: &str,
    ) {
        messages.push(ChatMessage::tool_result(tool_call_id, tool_name, result));
    }

    pub fn add_assistant_message(
        &self,
        messages: &mut Vec<ChatMessage>,
        content: Option<String>,
        tool_calls: Option<Vec<AssistantToolCall>>,
        reasoning_content: Option<String>,
        thinking_blocks: Option<Vec<String>>,
    ) {
        messages.push(ChatMessage::assistant(
            content,
            tool_calls,
            reasoning_content,
            thinking_blocks,
        ));
    }
    pub fn build_runtime_context(&self, channel: Option<&str>, chat_id: Option<&str>) -> String {
        let now = Local::now();
        let mut lines = vec![format!(
            "Current Time: {} ({})",
            now.format("%Y-%m-%d %H:%M (%A)"),
            now.offset()
        )];

        if let (Some(c), Some(id)) = (channel, chat_id) {
            lines.push(format!("Channel: {}", c));
            lines.push(format!("Chat ID: {}", id));
        }

        format!(
            "{}
{}",
            Self::RUNTIME_CONTEXT_TAG,
            lines.join("\n")
        )
    }

    fn identity_section(&self) -> String {
        IDENTITY_PROMPT_TEMPLATE.replace("{workspace}", &self.workspace.display().to_string())
    }

    async fn load_bootstrap_files(&self) -> String {
        let mut parts = Vec::new();
        for file in &Self::BOOTSTRAP_FILES {
            let path = self.workspace.join(file);
            if path.is_file() {
                if let Ok(content) = tokio::fs::read_to_string(&path).await {
                    parts.push(format!("## {}\n\n{}", file, content));
                }
            }
        }
        parts.join("\n\n")
    }

    fn build_user_content(&self, text: &str, media: Option<&[String]>) -> MessageContent {
        let Some(media) = media else {
            return MessageContent::Text(text.to_string());
        };

        let mut items = Vec::new();
        for path in media {
            let p = std::path::Path::new(path);
            if p.is_file() {
                items.push(ContentPart::Text {
                    text: format!("[media: {}]", path),
                });
            }
        }
        if items.is_empty() {
            MessageContent::Text(text.to_string())
        } else {
            items.push(ContentPart::Text {
                text: text.to_string(),
            });
            MessageContent::Parts(items)
        }
    }
}

#[async_trait]
impl ContextProvider for ContextBuilder {
    async fn build_messages(
        &self,
        session_manager: &SessionManager,
        session_key: &str,
        history: Vec<ChatMessage>,
        current_message: &str,
        media: Option<&[String]>,
        channel: Option<&str>,
        chat_id: Option<&str>,
    ) -> Vec<ChatMessage> {
        let runtime = self.build_runtime_context(channel, chat_id);
        let user_content = self.build_user_content(current_message, media);

        let merged = match user_content {
            MessageContent::Text(text) => MessageContent::Text(format!(
                "{}

{}",
                runtime, text
            )),
            MessageContent::Parts(mut parts) => {
                parts.insert(0, ContentPart::Text { text: runtime });
                MessageContent::Parts(parts)
            }
        };

        let mut messages = Vec::new();
        messages.push(ChatMessage::system_text(
            self.build_system_prompt(session_manager, session_key).await,
        ));
        messages.extend(history);
        messages.push(ChatMessage {
            role: MessageRole::User,
            content: Some(merged),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning_content: None,
            thinking_blocks: None,
        });
        messages
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_workspace(case: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "nanobot-rs-context-{}-{}",
            case,
            uuid::Uuid::new_v4()
        ))
    }

    #[test]
    fn build_runtime_context_includes_timestamp() {
        let workspace = temp_workspace("runtime-ts");
        let builder = ContextBuilder::new(workspace).unwrap();
        let ctx = builder.build_runtime_context(None, None);
        assert!(ctx.contains("Current Time:"));
        assert!(ctx.contains(ContextBuilder::RUNTIME_CONTEXT_TAG));
    }
}
