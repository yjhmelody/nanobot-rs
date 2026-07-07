//! Context building for agent system prompts and message assembly.
//!
//! [`ContextBuilder`] is the default [`ContextProvider`] implementation. It
//! constructs the system prompt by composing:
//!
//! * **Identity section** — the agent's role, workspace, and behavioural
//!   guidelines (from `IDENTITY_PROMPT_TEMPLATE`).
//! * **Bootstrap files** — optional `AGENTS.md`, `SOUL.md`, `USER.md`,
//!   `TOOLS.md`, and `IDENTITY.md` files read from the workspace root.
//! * **Memory context** — relevant excerpts from the session's long-term
//!   memory (managed by the session manager).
//! * **Always-on skills** — skills with `always: true` injected directly.
//! * **Skills summary** — a condensed `<skills>` block listing all
//!   available skills (progressive disclosure).
//!
//! # Design Notes
//!
//! - Progressive disclosure is achieved by only injecting skill *content*
//!   for always-on skills; other skills are listed as a summary.
//! - The runtime context (time, channel, chat ID) is merged with the user
//!   message rather than the system prompt to keep the system prompt
//!   cacheable across turns.

use std::path::PathBuf;

use async_trait::async_trait;
use chrono::Local;

use crate::error::AgentResult;
use crate::skills::SkillsLoader;
use crate::traits::{ContextProvider, SkillsProvider};
use nanobot_session::SessionManager;
use nanobot_types::provider::{
    AssistantToolCall, ChatMessage, ContentPart, MessageContent, MessageRole, ThinkingBlock,
};

/// Identity prompt template injected at the top of every system prompt.
///
/// Contains the agent identity ("nanobot"), runtime language, workspace
/// paths, and behavioural guidelines. The `{workspace}` placeholder is
/// replaced at runtime.
const IDENTITY_PROMPT_TEMPLATE: &str = "# nanobot\n\nYou are nanobot, a helpful AI assistant.\n\n## Runtime\nRust runtime\n\n## Workspace\nYour workspace is at: {workspace}\n- Long-term memory: {workspace}/memory/MEMORY.md\n- History log: {workspace}/memory/HISTORY.md\n- Custom skills: {workspace}/skills/{skill-name}/SKILL.md\n\n## nanobot Guidelines\n- State intent before tool calls, but NEVER predict or claim results before receiving them.\n- Before modifying a file, read it first. Do not assume files or directories exist.\n- After writing or editing a file, re-read it if accuracy matters.\n- If a tool call fails, analyze the error before retrying with a different approach.\n- Ask for clarification when the request is ambiguous.\n\nReply directly with text for conversations. Only use the 'message' tool to send to a specific chat channel.";
/// Preamble text prepended to the skills summary to teach the agent how to
/// use skills via the `read_file` tool.
const SKILLS_SUMMARY_PREAMBLE: &str = "# Skills\n\nThe following skills extend your capabilities. To use a skill, read its SKILL.md file using the read_file tool.\nSkills with available=\"false\" need dependencies installed first - you can try installing them with apt/brew.\n\n";

/// Assembles the system prompt and message history for each LLM turn.
///
/// Uses a pluggable [`SkillsProvider`] (defaulting to [`SkillsLoader`]) so
/// that tests can substitute a mock.
#[derive(Debug)]
pub struct ContextBuilder {
    workspace: PathBuf,
    skills: Box<dyn SkillsProvider>,
}

impl ContextBuilder {
    /// Workspace files that are loaded as bootstrap context (in order).
    ///
    /// Each file is a Markdown document that adds system-level context.
    pub const BOOTSTRAP_FILES: [&'static str; 5] =
        ["AGENTS.md", "SOUL.md", "USER.md", "TOOLS.md", "IDENTITY.md"];

    /// Marker tag applied to runtime metadata to distinguish it from
    /// instructional content.
    pub const RUNTIME_CONTEXT_TAG: &'static str =
        "[Runtime Context \u{2014} metadata only, not instructions]";

    /// Creates a `ContextBuilder` using the default [`SkillsLoader`] for the
    /// given workspace.
    pub fn new(workspace: PathBuf) -> AgentResult<Self> {
        let skills = Box::new(SkillsLoader::new(&workspace));
        Ok(Self { workspace, skills })
    }

    /// Creates a `ContextBuilder` with a custom [`SkillsProvider`].
    ///
    /// Useful for injecting a mock during testing.
    pub fn with_skills_provider(workspace: PathBuf, skills: Box<dyn SkillsProvider>) -> Self {
        Self { workspace, skills }
    }

    /// Builds the complete system prompt for the current turn.
    ///
    /// Composes identity, bootstrap files, memory, always-on skills, and
    /// the skills summary into a single system-prompt string.
    ///
    /// # Arguments
    ///
    /// * `session_manager` — for retrieving memory context.
    /// * `session_key` — identifies the conversation session.
    /// * `current_message` — the user's latest message, used to inform
    ///   memory retrieval.
    pub async fn build_system_prompt(
        &self,
        session_manager: &SessionManager,
        session_key: &str,
        current_message: &str,
    ) -> String {
        let mut parts = vec![self.identity_section()];

        let bootstrap = self.load_bootstrap_files().await;
        if !bootstrap.trim().is_empty() {
            parts.push(bootstrap);
        }

        let memory = session_manager
            .get_memory_context(current_message, session_key)
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

    /// Appends a tool-result message to the conversation history.
    pub fn add_tool_result(
        &self,
        messages: &mut Vec<ChatMessage>,
        tool_call_id: &str,
        tool_name: &str,
        result: &str,
    ) {
        messages.push(ChatMessage::tool_result(tool_call_id, tool_name, result));
    }

    /// Appends an assistant message (with optional content, tool calls,
    /// reasoning, and thinking blocks) to the conversation history.
    pub fn add_assistant_message(
        &self,
        messages: &mut Vec<ChatMessage>,
        content: Option<String>,
        tool_calls: Option<Vec<AssistantToolCall>>,
        reasoning_content: Option<String>,
        thinking_blocks: Option<Vec<ThinkingBlock>>,
    ) {
        messages.push(ChatMessage::assistant(
            content,
            tool_calls,
            reasoning_content,
            thinking_blocks,
        ));
    }

    /// Builds a runtime-context string with the current timestamp and
    /// channel/chat-id metadata.
    ///
    /// The returned string is tagged with [`RUNTIME_CONTEXT_TAG`] so that
    /// the LLM can identify it as non-instructional metadata.
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

        format!("{}\n{}", Self::RUNTIME_CONTEXT_TAG, lines.join("\n"))
    }

    /// Returns the identity section with the workspace path substituted in.
    fn identity_section(&self) -> String {
        IDENTITY_PROMPT_TEMPLATE.replace("{workspace}", &self.workspace.display().to_string())
    }

    /// Reads all bootstrap files that exist on disk and concatenates them.
    async fn load_bootstrap_files(&self) -> String {
        let mut parts = Vec::new();
        for file in &Self::BOOTSTRAP_FILES {
            let path = self.workspace.join(file);
            if path.is_file()
                && let Ok(content) = tokio::fs::read_to_string(&path).await
            {
                parts.push(format!("## {}\n\n{}", file, content));
            }
        }
        parts.join("\n\n")
    }

    /// Builds the user message content, optionally attaching media
    /// references as content parts.
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
    /// Builds the full message history for the LLM request.
    ///
    /// Returns: `[system_prompt, ...history, user_message]` where the user
    /// message includes runtime context merged with the user's text.
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
            MessageContent::Text(text) => MessageContent::Text(format!("{}\n\n{}", runtime, text)),
            MessageContent::Parts(mut parts) => {
                parts.insert(0, ContentPart::Text { text: runtime });
                MessageContent::Parts(parts)
            }
        };

        let mut messages = Vec::new();
        messages.push(ChatMessage::system_text(
            self.build_system_prompt(session_manager, session_key, current_message)
                .await,
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
        std::env::temp_dir().join(format!("nanobot-context-{}-{}", case, uuid::Uuid::new_v4()))
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
