use std::path::PathBuf;

use anyhow::Result;
use chrono::Local;

use crate::agent::memory::MemoryStore;
use crate::agent::skills::SkillsLoader;
use crate::provider::{AssistantToolCall, ChatMessage, ContentPart, MessageContent, MessageRole};

pub struct ContextBuilder {
    workspace: PathBuf,
    memory: MemoryStore,
    skills: SkillsLoader,
}

impl ContextBuilder {
    /// Bootstrap files that are loaded into the system prompt if present.
    ///
    /// These files should be placed in the workspace root directory:
    /// - AGENTS.md: Agent behavior and guidelines
    /// - SOUL.md: Core personality and values
    /// - USER.md: User preferences and context
    /// - TOOLS.md: Tool usage guidelines
    /// - IDENTITY.md: Agent identity and role
    pub const BOOTSTRAP_FILES: [&'static str; 5] =
        ["AGENTS.md", "SOUL.md", "USER.md", "TOOLS.md", "IDENTITY.md"];

    /// Tag used to mark runtime context in messages.
    pub const RUNTIME_CONTEXT_TAG: &'static str =
        "[Runtime Context — metadata only, not instructions]";

    /// Creates a new context builder for the specified workspace.
    ///
    /// # Arguments
    ///
    /// * `workspace` - Workspace directory path
    ///
    /// # Errors
    ///
    /// Returns an error if the memory store cannot be initialized.
    pub fn new(workspace: PathBuf) -> Result<Self> {
        let memory = MemoryStore::new(&workspace)?;
        let skills = SkillsLoader::new(&workspace);
        Ok(Self {
            workspace,
            memory,
            skills,
        })
    }

    /// Builds the system prompt for the agent.
    ///
    /// The system prompt includes:
    /// - Agent identity and role
    /// - Bootstrap files content (if available)
    /// - Memory context (short-term and long-term)
    /// - Active skills (always-on skills)
    /// - Available skills summary
    ///
    /// # Returns
    ///
    /// Returns the complete system prompt as a string.
    pub async fn build_system_prompt(&self) -> String {
        let mut parts = vec![self.identity_section()];

        let bootstrap = self.load_bootstrap_files();
        if !bootstrap.trim().is_empty() {
            parts.push(bootstrap);
        }

        let memory = self.memory.get_memory_context().await;
        if !memory.trim().is_empty() {
            parts.push(format!("# Memory\n\n{}", memory));
        }

        let always = self.skills.get_always_skills();
        if !always.is_empty() {
            let content = self.skills.load_skills_for_context(&always);
            if !content.trim().is_empty() {
                parts.push(format!("# Active Skills\n\n{}", content));
            }
        }

        let summary = self.skills.build_skills_summary();
        if !summary.trim().is_empty() {
            parts.push(format!(
                "# Skills\n\nThe following skills extend your capabilities. To use a skill, read its SKILL.md file using the read_file tool.\nSkills with available=\"false\" need dependencies installed first - you can try installing them with apt/brew.\n\n{}",
                summary
            ));
        }

        parts.join("\n\n---\n\n")
    }

    /// Builds the message history for the LLM request.
    ///
    /// This method constructs the complete message array including:
    /// - System prompt
    /// - Historical messages
    /// - Current user message with runtime context
    ///
    /// # Arguments
    ///
    /// * `history` - Previous conversation messages
    /// * `current_message` - The new user message text
    /// * `media` - Optional media attachments (URLs or paths)
    /// * `channel` - Optional channel name for runtime context
    /// * `chat_id` - Optional chat ID for runtime context
    ///
    /// # Returns
    ///
    /// Returns a vector of chat messages ready for the LLM API.
    pub async fn build_messages(
        &self,
        history: Vec<ChatMessage>,
        current_message: &str,
        media: Option<&[String]>,
        channel: Option<&str>,
        chat_id: Option<&str>,
    ) -> Vec<ChatMessage> {
        let runtime = Self::build_runtime_context(channel, chat_id);
        let user_content = self.build_user_content(current_message, media);

        let merged = match user_content {
            MessageContent::Text(text) => MessageContent::Text(format!("{}\n\n{}", runtime, text)),
            MessageContent::Parts(mut parts) => {
                parts.insert(0, ContentPart::Text { text: runtime });
                MessageContent::Parts(parts)
            }
        };

        let mut messages = Vec::new();
        messages.push(ChatMessage::system_text(self.build_system_prompt().await));
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

    /// Adds a tool execution result to the message history.
    ///
    /// # Arguments
    ///
    /// * `messages` - Message vector to append to
    /// * `tool_call_id` - ID of the tool call this result corresponds to
    /// * `tool_name` - Name of the tool that was executed
    /// * `result` - Tool execution result (success or error message)
    pub fn add_tool_result(
        &self,
        messages: &mut Vec<ChatMessage>,
        tool_call_id: &str,
        tool_name: &str,
        result: &str,
    ) {
        messages.push(ChatMessage::tool_result(tool_call_id, tool_name, result));
    }

    /// Adds an assistant message with tool calls to the message history.
    ///
    /// # Arguments
    ///
    /// * `messages` - Message vector to append to
    /// * `content` - Optional text content from the assistant
    /// * `tool_calls` - Optional tool calls requested by the assistant
    /// * `reasoning_content` - Optional reasoning/thinking content
    /// * `thinking_blocks` - Optional thinking blocks for extended thinking models
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

    /// Builds runtime context information for the agent.
    ///
    /// Runtime context includes:
    /// - Current timestamp with timezone
    /// - Channel name (if provided)
    /// - Chat ID (if provided)
    ///
    /// This context is prepended to user messages to provide temporal and
    /// conversational context to the agent.
    ///
    /// # Arguments
    ///
    /// * `channel` - Optional channel name
    /// * `chat_id` - Optional chat ID
    ///
    /// # Returns
    ///
    /// Returns formatted runtime context as a string.
    pub fn build_runtime_context(channel: Option<&str>, chat_id: Option<&str>) -> String {
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

    fn identity_section(&self) -> String {
        let workspace = self.workspace.display().to_string();
        format!(
            "# nanobot :cat:\n\nYou are nanobot, a helpful AI assistant.\n\n## Runtime\nRust runtime\n\n## Workspace\nYour workspace is at: {}\n- Long-term memory: {}/memory/MEMORY.md\n- History log: {}/memory/HISTORY.md\n- Custom skills: {}/skills/{{skill-name}}/SKILL.md\n\n## nanobot Guidelines\n- State intent before tool calls, but NEVER predict or claim results before receiving them.\n- Before modifying a file, read it first. Do not assume files or directories exist.\n- After writing or editing a file, re-read it if accuracy matters.\n- If a tool call fails, analyze the error before retrying with a different approach.\n- Ask for clarification when the request is ambiguous.\n\nReply directly with text for conversations. Only use the 'message' tool to send to a specific chat channel.",
            workspace, workspace, workspace, workspace
        )
    }

    fn load_bootstrap_files(&self) -> String {
        let mut parts = Vec::new();
        for file in Self::BOOTSTRAP_FILES {
            let path = self.workspace.join(file);
            if let Ok(content) = std::fs::read_to_string(&path) {
                parts.push(format!("## {}\n\n{}", file, content));
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
