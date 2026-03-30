use async_trait::async_trait;

use crate::skills::SkillInfo;
use crate::error::AgentResult;
use nanobot_session::SessionManager;
use nanobot_types::SessionKey;
use nanobot_types::provider::ChatMessage;

/// Trait for building context and messages for the agent.
#[async_trait]
pub trait ContextProvider: Send + Sync {
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
    async fn build_messages(
        &self,
        session_manager: &SessionManager,
        session_key: &str,
        history: Vec<ChatMessage>,
        current_message: &str,
        media: Option<&[String]>,
        channel: Option<&str>,
        chat_id: Option<&str>,
    ) -> Vec<ChatMessage>;
}

/// Trait for the main agent reasoning loop.
#[async_trait]
pub trait Agent: Send + Sync {
    /// Starts the inbound message loop, blocking until stopped.
    async fn run(self: std::sync::Arc<Self>);
    /// Signals the agent to stop processing new messages.
    async fn stop(&self);
    /// Stops the agent and shuts down MCP and provider connections.
    async fn shutdown(&self) {
        self.stop().await;
        self.close_mcp().await;
        self.close_provider().await;
    }
    /// Processes a single message directly, bypassing the bus, and returns the response text.
    async fn process_direct(
        &self,
        content: &str,
        session_key: &SessionKey,
        channel: &str,
        chat_id: &str,
    ) -> AgentResult<String>;
    /// Returns `true` if there are in-flight tasks for the given session.
    fn has_active_tasks(&self, session_key: &SessionKey) -> bool;
    /// Closes all MCP server connections.
    async fn close_mcp(&self);
    /// Closes the underlying LLM provider connection.
    async fn close_provider(&self);
}

/// Trait for managing skills with progressive disclosure.
#[async_trait]
pub trait SkillsProvider: Send + Sync + std::fmt::Debug {
    /// Lists available skills, optionally excluding those whose requirements are unmet.
    async fn list_skills(&self, filter_unavailable: bool) -> Vec<SkillInfo>;
    /// Loads the full content of a skill by name, returning `None` if not found.
    async fn load_skill(&self, name: &str) -> Option<String>;
    /// Returns the content of all skills marked `always: true`.
    async fn get_always_skills(&self) -> Vec<String>;
    /// Loads and concatenates the content of the named skills.
    async fn load_skills_for_context(&self, skill_names: &[String]) -> String;
    /// Builds a condensed summary of all available skills for injection into the system prompt.
    async fn build_skills_summary(&self) -> String;
}
