use std::path::PathBuf;

use anyhow::Result;
use chrono::Local;

use crate::agent::skills::SkillsLoader;
use crate::session::SessionManager;
use crate::types::provider::{
    AssistantToolCall, ChatMessage, ContentPart, MessageContent, MessageRole,
};

const IDENTITY_PROMPT_TEMPLATE: &str =
    "# nanobot :cat:\n\nYou are nanobot, a helpful AI assistant.\n\n## Runtime\nRust runtime\n\n## Workspace\nYour workspace is at: {workspace}\n- Long-term memory: {workspace}/memory/MEMORY.md\n- History log: {workspace}/memory/HISTORY.md\n- Custom skills: {workspace}/skills/{skill-name}/SKILL.md\n\n## nanobot Guidelines\n- State intent before tool calls, but NEVER predict or claim results before receiving them.\n- Before modifying a file, read it first. Do not assume files or directories exist.\n- After writing or editing a file, re-read it if accuracy matters.\n- If a tool call fails, analyze the error before retrying with a different approach.\n- Ask for clarification when the request is ambiguous.\n\nReply directly with text for conversations. Only use the 'message' tool to send to a specific chat channel.";
const SKILLS_SUMMARY_PREAMBLE: &str =
    "# Skills\n\nThe following skills extend your capabilities. To use a skill, read its SKILL.md file using the read_file tool.\nSkills with available=\"false\" need dependencies installed first - you can try installing them with apt/brew.\n\n";

// TODO: design a cache for context
#[derive(Debug, Clone)]
pub struct ContextBuilder {
    workspace: PathBuf,
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
    /// Returns an error if initialization fails.
    pub fn new(workspace: PathBuf) -> Result<Self> {
        let skills = SkillsLoader::new(&workspace);
        Ok(Self { workspace, skills })
    }

    /// Builds the system prompt for the agent.
    ///
    /// The system prompt includes:
    /// - Agent identity and role
    /// - Bootstrap files content (if available)
    /// - Memory context (from SessionManager)
    /// - Active skills (always-on skills)
    /// - Available skills summary
    ///
    /// # Arguments
    ///
    /// * `session_manager` - Session manager for retrieving memory context
    /// * `session_key` - Current session key for memory lookup
    ///
    /// # Returns
    ///
    /// Returns the complete system prompt as a string.
    pub async fn build_system_prompt(
        &self,
        session_manager: &SessionManager,
        session_key: &str,
    ) -> String {
        let mut parts = vec![self.identity_section()];

        // TODO: watch these files for changes and update context cache
        let bootstrap = self.load_bootstrap_files().await;
        if !bootstrap.trim().is_empty() {
            parts.push(bootstrap);
        }

        // Get memory context from SessionManager
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

    /// Builds the message history for the LLM request.
    ///
    /// This method constructs the complete message array including:
    /// - System prompt
    /// - Historical messages
    /// - Current user message with runtime context
    ///
    /// # Arguments
    ///
    /// * `session_manager` - Session manager for retrieving memory context
    /// * `session_key` - Current session key for memory lookup
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
        session_manager: &SessionManager,
        session_key: &str,
        history: impl IntoIterator<Item = ChatMessage>,
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
        IDENTITY_PROMPT_TEMPLATE.replace("{workspace}", &workspace)
    }

    async fn load_bootstrap_files(&self) -> String {
        let mut parts = Vec::new();
        for file in Self::BOOTSTRAP_FILES {
            let path = self.workspace.join(file);
            if let Ok(content) = tokio::fs::read_to_string(&path).await {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_workspace(case: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "nanobot-rs-context-{}-{}",
            case,
            uuid::Uuid::new_v4()
        ))
    }

    #[test]
    fn build_runtime_context_includes_timestamp() {
        let ctx = ContextBuilder::build_runtime_context(None, None);
        assert!(ctx.contains("Current Time:"));
        assert!(ctx.contains(ContextBuilder::RUNTIME_CONTEXT_TAG));
    }

    #[test]
    fn build_runtime_context_includes_channel_and_chat_id() {
        let ctx = ContextBuilder::build_runtime_context(Some("telegram"), Some("123456"));
        assert!(ctx.contains("Channel: telegram"));
        assert!(ctx.contains("Chat ID: 123456"));
    }

    #[test]
    fn identity_section_includes_workspace_path() {
        let workspace = temp_workspace("identity");
        fs::create_dir_all(&workspace).expect("create workspace");

        let builder = ContextBuilder::new(workspace.clone()).expect("new builder");
        let identity = builder.identity_section();

        assert!(identity.contains("nanobot"));
        assert!(identity.contains(&workspace.display().to_string()));
        assert!(identity.contains("memory/MEMORY.md"));

        let _ = fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn load_bootstrap_files_reads_existing_files() {
        let workspace = temp_workspace("bootstrap");
        fs::create_dir_all(&workspace).expect("create workspace");
        fs::write(workspace.join("SOUL.md"), "# Soul\n\nBe helpful").expect("write soul");
        fs::write(workspace.join("USER.md"), "# User\n\nName: Alice").expect("write user");

        let builder = ContextBuilder::new(workspace.clone()).expect("new builder");
        let bootstrap = builder.load_bootstrap_files().await;

        assert!(bootstrap.contains("SOUL.md"));
        assert!(bootstrap.contains("Be helpful"));
        assert!(bootstrap.contains("USER.md"));
        assert!(bootstrap.contains("Name: Alice"));

        let _ = fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn load_bootstrap_files_skips_missing_files() {
        let workspace = temp_workspace("bootstrap-missing");
        fs::create_dir_all(&workspace).expect("create workspace");

        let builder = ContextBuilder::new(workspace.clone()).expect("new builder");
        let bootstrap = builder.load_bootstrap_files().await;

        // Should not fail, just return empty or partial content
        assert!(!bootstrap.contains("SOUL.md") || bootstrap.is_empty());

        let _ = fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn build_system_prompt_includes_identity() {
        use crate::session::{JsonlSessionStore, SessionManager};
        use tempfile::tempdir;

        let temp_dir = tempdir().expect("create temp dir");
        let workspace = temp_dir.path().to_path_buf();

        let builder = ContextBuilder::new(workspace.clone()).expect("new builder");
        let store = JsonlSessionStore::new(&workspace)
            .await
            .expect("create store");
        let session_manager = SessionManager::new(Box::new(store));

        let prompt = builder
            .build_system_prompt(&session_manager, "test-session")
            .await;

        assert!(prompt.contains("nanobot"));
        assert!(prompt.contains(&workspace.display().to_string()));
    }

    #[tokio::test]
    async fn build_messages_includes_system_and_user() {
        use crate::session::{JsonlSessionStore, SessionManager};
        use tempfile::tempdir;

        let temp_dir = tempdir().expect("create temp dir");
        let workspace = temp_dir.path().to_path_buf();

        let builder = ContextBuilder::new(workspace.clone()).expect("new builder");
        let store = JsonlSessionStore::new(&workspace)
            .await
            .expect("create store");
        let session_manager = SessionManager::new(Box::new(store));

        let messages = builder
            .build_messages(
                &session_manager,
                "test-session",
                vec![],
                "Hello",
                None,
                Some("cli"),
                Some("direct"),
            )
            .await;

        assert!(messages.len() >= 2);
        assert!(matches!(messages[0].role, MessageRole::System));
        assert!(matches!(messages[1].role, MessageRole::User));

        let user_content = messages[1].content_as_text().unwrap_or("");
        assert!(user_content.contains("Hello"));
        assert!(user_content.contains("Channel: cli"));
    }

    #[tokio::test]
    async fn build_messages_includes_history() {
        use crate::session::{JsonlSessionStore, SessionManager};
        use tempfile::tempdir;

        let temp_dir = tempdir().expect("create temp dir");
        let workspace = temp_dir.path().to_path_buf();

        let builder = ContextBuilder::new(workspace.clone()).expect("new builder");
        let store = JsonlSessionStore::new(&workspace)
            .await
            .expect("create store");
        let session_manager = SessionManager::new(Box::new(store));

        let history = vec![
            ChatMessage::user_text("Previous question"),
            ChatMessage::assistant(Some("Previous answer".to_string()), None, None, None),
        ];

        let messages = builder
            .build_messages(
                &session_manager,
                "test-session",
                history,
                "New question",
                None,
                None,
                None,
            )
            .await;

        // System + 2 history + 1 new user
        assert_eq!(messages.len(), 4);
        assert!(matches!(messages[0].role, MessageRole::System));
        assert!(matches!(messages[1].role, MessageRole::User));
        assert!(matches!(messages[2].role, MessageRole::Assistant));
        assert!(matches!(messages[3].role, MessageRole::User));
    }

    #[test]
    fn add_tool_result_appends_message() {
        let workspace = temp_workspace("tool-result");
        fs::create_dir_all(&workspace).expect("create workspace");

        let builder = ContextBuilder::new(workspace.clone()).expect("new builder");
        let mut messages = vec![];

        builder.add_tool_result(&mut messages, "call-123", "read_file", "file content");

        assert_eq!(messages.len(), 1);
        assert!(matches!(messages[0].role, MessageRole::Tool));
        assert_eq!(messages[0].tool_call_id.as_deref(), Some("call-123"));
        assert_eq!(messages[0].name.as_deref(), Some("read_file"));

        let _ = fs::remove_dir_all(workspace);
    }

    #[test]
    fn add_assistant_message_appends_message() {
        let workspace = temp_workspace("assistant-msg");
        fs::create_dir_all(&workspace).expect("create workspace");

        let builder = ContextBuilder::new(workspace.clone()).expect("new builder");
        let mut messages = vec![];

        builder.add_assistant_message(
            &mut messages,
            Some("Response text".to_string()),
            None,
            None,
            None,
        );

        assert_eq!(messages.len(), 1);
        assert!(matches!(messages[0].role, MessageRole::Assistant));
        assert_eq!(messages[0].content_as_text(), Some("Response text"));

        let _ = fs::remove_dir_all(workspace);
    }

    #[test]
    fn build_user_content_handles_text_only() {
        let workspace = temp_workspace("content-text");
        fs::create_dir_all(&workspace).expect("create workspace");

        let builder = ContextBuilder::new(workspace.clone()).expect("new builder");
        let content = builder.build_user_content("Hello world", None);

        assert_eq!(content.as_text(), Some("Hello world"));

        let _ = fs::remove_dir_all(workspace);
    }

    #[test]
    fn build_user_content_handles_media() {
        let workspace = temp_workspace("content-media");
        fs::create_dir_all(&workspace).expect("create workspace");
        let media_file = workspace.join("image.png");
        fs::write(&media_file, b"fake image").expect("write media");

        let builder = ContextBuilder::new(workspace.clone()).expect("new builder");
        let content = builder.build_user_content(
            "Check this image",
            Some(&[media_file.display().to_string()]),
        );

        match content {
            MessageContent::Parts(parts) => {
                assert!(parts.len() >= 2);
                assert!(parts.iter().any(|p| {
                    #[allow(irrefutable_let_patterns)]
                    if let ContentPart::Text { text } = p {
                        text.contains("image.png")
                    } else {
                        false
                    }
                }));
            }
            _ => panic!("Expected Parts content"),
        }

        let _ = fs::remove_dir_all(workspace);
    }
}
